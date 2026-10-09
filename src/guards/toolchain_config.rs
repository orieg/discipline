//! `toolchain-config`: a change cannot quietly lower the compiler, linter, type-checker,
//! test-runner or coverage bar it is judged by.
//!
//! `config-integrity` guards `discipline.toml`; this gate does the same for the files that
//! configure the rest of the toolchain. Each recognised file is loaded, on the base side
//! and the head side, into one generic tree (TOML, YAML, JSON with comments and INI all
//! land in `serde_json::Value`), and a rule table says which key paths loosen in which
//! direction. A configuration written as code (`eslint.config.js`, `jest.config.ts`) has no
//! tree to compare: a change to it is reported as not analysed, never passed in silence.

mod rules;

pub use super::config_delta::{classify_with, diff_trees, load, Classified, LAX_FLAG_PREFIXES};
pub use rules::RULES;

use super::config_delta::{self, AbsentSide, ReadOrder, TreeGate};
use super::{build_flags, Context, GateOutcome};
use crate::config::Severity;
use crate::tokens;
use anyhow::Result;
use rules::{EXECUTABLE, INHERITANCE};
use serde_json::Value;

/// Whether `path` is one of the rule files (by basename, or `dir/basename` for the dotted
/// directories), or an executable configuration.
pub fn classify(path: &str) -> Option<Classified> {
    classify_with(path, RULES, EXECUTABLE)
}

/// `(key, what was gained)` for every inheritance key of `name` whose head side names an
/// entry the base side did not.
pub fn inherited_changes(name: &str, base: &Value, head: &Value) -> Vec<(String, String)> {
    config_delta::gained_entries(INHERITANCE, name, base, head)
}

/// A build file (`Makefile`, `CMakeLists.txt`, `setup.py`, `build.rs`): compiler warning
/// flags it gained that lower the bar, and strict ones it lost
/// ([`build_flags`]). A deleted file is not judged: removing a build file is a change of
/// build system, not a lowered flag.
fn build_file(
    ctx: &Context,
    severity: Severity,
    out: &mut GateOutcome,
    file: &crate::gitctx::ChangedFile,
    kind: build_flags::BuildKind,
) -> Result<()> {
    const GATE: &str = "toolchain-config";
    let Some(head_src) = ctx.git.head_content(&file.path)? else {
        return Ok(());
    };
    let base_src = ctx.git.base_content(&file.old_path)?;
    let read = |src: &str| build_flags::extract(kind, src);
    let head = read(&head_src);
    let base = match &base_src {
        Some(src) => read(src),
        None => Ok(Vec::new()),
    };
    let (base, head) = match (base, head) {
        (Ok(base), Ok(head)) => (base, head),
        (Err(build_flags::ExtractError::GrammarAbsent), _)
        | (_, Err(build_flags::ExtractError::GrammarAbsent)) => {
            out.notes.push(format!(
                "`{}` was not read: this build of discipline lacks the grammar it needs",
                file.path
            ));
            return Ok(());
        }
        _ => {
            out.examined += 1;
            out.push(
                severity,
                &crate::findings::TOOLCHAIN_CONFIG_UNREADABLE,
                Some(&file.path),
                None,
                format!(
                    "`{}` could not be parsed on one side, so its warning flags could not be checked.",
                    file.path
                ),
                "Fix the file so it parses.",
            );
            return Ok(());
        }
    };
    out.examined += 1;
    for change in build_flags::judge(&base, &head) {
        let lift = |subject: &str| {
            ctx.find_override(
                GATE,
                &crate::findings::TOOLCHAIN_CONFIG_WEAKENED,
                tokens::ALLOW_TOOLCHAIN_WEAKENING,
                subject,
            )
        };
        if let Some(ov) = lift(&change.flag)
            .or_else(|| lift(&change.context))
            .or_else(|| lift(&file.path))
        {
            out.overrides.push(ov);
            continue;
        }
        let (what, remedy) = if change.gained {
            (
                "gained",
                "Drop it, or justify it on its own line in the PR body or a commit message",
            )
        } else {
            (
                "lost",
                "Restore it, or justify the loss on its own line in the PR body or a commit message",
            )
        };
        out.push(
            ctx.overridable(severity),
            &crate::findings::TOOLCHAIN_CONFIG_WEAKENED,
            Some(&file.path),
            change.line,
            format!(
                "`{}` {what} `{}` in `{}`; the compiler warning bar is lower.",
                change.context, change.flag, file.path
            ),
            &format!(
                "{remedy}: `allow-toolchain-weakening: {} <reason>`.",
                change.flag
            ),
        );
        out.anchor_last(format!("{} {}", change.context, change.flag));
    }
    Ok(())
}

/// A changed file the rule table does not read: a build file is judged on its warning
/// flags ([`build_file`]).
fn other_file(
    ctx: &Context,
    severity: Severity,
    out: &mut GateOutcome,
    file: &crate::gitctx::ChangedFile,
) -> Result<()> {
    if let Some(kind) = build_flags::classify_build(&file.path) {
        build_file(ctx, severity, out, file, kind)?;
    }
    Ok(())
}

fn executable_message(path: &str) -> String {
    format!(
        "`{path}` is configuration written as code; whether the change loosens it cannot be read from a diff."
    )
}

fn unreadable_message(path: &str, _side: &str) -> String {
    format!("`{path}` could not be parsed on one side, so its weakening could not be checked.")
}

/// What this gate hands the shared loop ([`config_delta::run`]).
const TREE_GATE: TreeGate = TreeGate {
    id: "toolchain-config",
    directive: tokens::ALLOW_TOOLCHAIN_WEAKENING,
    classify,
    changed: &crate::findings::TOOLCHAIN_CONFIG_WEAKENED,
    not_analysed: &crate::findings::TOOLCHAIN_CHANGE_NOT_ANALYSED,
    unreadable: &crate::findings::TOOLCHAIN_CONFIG_UNREADABLE,
    executable_message,
    unreadable_message,
    // A file that appears has no bar to lower; one that disappears is reported: its
    // settings no longer apply.
    absent_side: AbsentSide::NotCompared {
        deleted: &crate::findings::TOOLCHAIN_CONFIG_DELETED,
    },
    read_order: ReadOrder::HeadThenBase,
    inherited: Some(inherited_changes),
    other_file: Some(other_file),
    nothing_examined: "no toolchain configuration files modified in this diff",
};

pub fn toolchain_config(ctx: &Context) -> Result<GateOutcome> {
    config_delta::run(ctx, &ctx.config.gates.toolchain_config, &TREE_GATE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(name: &str, base: &str, head: &str) -> Vec<(String, String)> {
        let Some(Classified::Data { name, rules }) = classify(name) else {
            panic!("{name} is not a data config");
        };
        let b = load(&name, base).expect("base parses");
        let h = load(&name, head).expect("head parses");
        diff_trees(&b, &h, &rules)
            .into_iter()
            .map(|w| (w.key, w.what))
            .collect()
    }

    #[test]
    fn tsconfig_with_comments_strict_off_and_exclude_grown() {
        let base = "{\n  // strict everywhere\n  \"compilerOptions\": { \"strict\": true, \"skipLibCheck\": false, },\n  \"exclude\": [\"dist\"],\n}\n";
        let head = "{\"compilerOptions\": {\"strict\": false, \"skipLibCheck\": true}, \"exclude\": [\"dist\", \"src/legacy\"]}";
        assert_eq!(
            diff("tsconfig.json", base, head),
            vec![
                (
                    "compilerOptions.strict".to_string(),
                    "switched off".to_string()
                ),
                (
                    "compilerOptions.skipLibCheck".to_string(),
                    "switched on".to_string()
                ),
                (
                    "exclude".to_string(),
                    "gained 1 entr(y/ies): `src/legacy`".to_string()
                ),
            ]
        );
        // The reverse tightens; removing `strict: true` loosens.
        assert!(diff("tsconfig.build.json", head, base).is_empty());
        assert_eq!(
            diff("tsconfig.json", base, "{\"compilerOptions\": {}}")[0].1,
            "removed (was on)"
        );
    }

    #[test]
    fn pyproject_ruff_mypy_pytest_and_coverage() {
        let base = "[tool.ruff.lint]\nselect = [\"E\", \"F\", \"B\"]\nignore = []\n\
            [tool.mypy]\nstrict = true\n\
            [tool.pytest.ini_options]\naddopts = \"--strict-markers -p no:cacheprovider\"\nxfail_strict = true\n\
            [tool.coverage.report]\nfail_under = 90\n";
        let head = "[tool.ruff.lint]\nselect = [\"E\", \"F\"]\nignore = [\"E501\"]\n\
            [tool.ruff.lint.per-file-ignores]\n\"tests/*\" = [\"S101\"]\n\
            [tool.mypy]\nstrict = false\nignore_missing_imports = true\n\
            [tool.pytest.ini_options]\naddopts = \"-p no:cacheprovider --reruns 3\"\nxfail_strict = false\n\
            [tool.coverage.report]\nfail_under = 60\n";
        let keys: Vec<String> = diff("pyproject.toml", base, head)
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(
            keys,
            vec![
                "tool.ruff.lint.select",
                "tool.ruff.lint.ignore",
                "tool.ruff.lint.per-file-ignores.tests/*",
                "tool.mypy.strict",
                "tool.mypy.ignore_missing_imports",
                "tool.pytest.ini_options.addopts",
                "tool.pytest.ini_options.xfail_strict",
                "tool.coverage.report.fail_under",
            ]
        );
        let found = diff("pyproject.toml", base, head);
        let addopts = &found
            .iter()
            .find(|(k, _)| k.ends_with("addopts"))
            .unwrap()
            .1;
        assert_eq!(addopts, "lost `--strict-markers`; gained `--reruns`");
        // A version bump elsewhere in the file is silent.
        assert!(diff(
            "pyproject.toml",
            base,
            &format!("{base}[project]\nversion = \"2\"\n")
        )
        .is_empty());
    }

    #[test]
    fn ini_files_mypy_pytest_coveragerc_and_setup_cfg() {
        let base = "[mypy]\nstrict = True\nexclude = build/\n\n[tool:pytest]\naddopts = --strict-config\n\n[flake8]\nmax-line-length = 88\nignore =\n    E203\n";
        let head = "[mypy]\nstrict = False\nexclude = build/, vendor/\n\n[mypy-thirdparty.*]\nignore_errors = True\n\n[tool:pytest]\naddopts = -q\n\n[flake8]\nmax-line-length = 120\nignore =\n    E203\n    W503\n";
        let keys: Vec<String> = diff("setup.cfg", base, head)
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(
            keys,
            vec![
                "mypy.strict",
                "mypy.exclude",
                "mypy-thirdparty.*.ignore_errors",
                "tool:pytest.addopts",
                "flake8.ignore",
                "flake8.max-line-length",
            ]
        );
        assert_eq!(
            diff(
                ".coveragerc",
                "[report]\nfail_under = 85\n",
                "[report]\nfail_under = 70\n"
            ),
            vec![(
                "report.fail_under".to_string(),
                "lowered from 85 to 70".to_string()
            )]
        );
    }

    #[test]
    fn cargo_lints_rustflags_nextest_and_eslint_levels() {
        assert_eq!(
            diff(
                "Cargo.toml",
                "[lints.rust]\nunsafe_code = \"forbid\"\n[lints.clippy]\nunwrap_used = { level = \"deny\", priority = 1 }\nall = \"warn\"\n",
                "[lints.rust]\nunsafe_code = \"warn\"\n[lints.clippy]\nunwrap_used = \"allow\"\nall = \"deny\"\n",
            ),
            vec![
                ("lints.clippy.unwrap_used".to_string(), "lowered from error to off".to_string()),
                ("lints.rust.unsafe_code".to_string(), "lowered from error to warn".to_string()),
            ]
        );
        assert_eq!(
            diff(
                ".cargo/config.toml",
                "[build]\nrustflags = [\"-D\", \"warnings\", \"-C\", \"target-cpu=native\"]\n",
                "[build]\nrustflags = [\"-C\", \"target-cpu=native\", \"-A\", \"dead_code\"]\n",
            ),
            vec![(
                "build.rustflags".to_string(),
                "lost `-D warnings`; gained `-A`".to_string()
            )]
        );
        assert_eq!(
            diff(
                ".config/nextest.toml",
                "[profile.ci]\nfail-fast = true\n",
                "[profile.ci]\nretries = 3\nfail-fast = false\n"
            ),
            vec![
                (
                    "profile.ci.retries".to_string(),
                    "raised from 0 to 3".to_string()
                ),
                (
                    "profile.ci.fail-fast".to_string(),
                    "switched off".to_string()
                ),
            ]
        );
        assert_eq!(
            diff(
                ".eslintrc.json",
                "{\"rules\": {\"no-unused-vars\": \"error\", \"eqeqeq\": [2, \"always\"], \"semi\": 1}}",
                "{\"rules\": {\"no-unused-vars\": \"warn\", \"eqeqeq\": [\"off\"], \"semi\": 2}, \"ignorePatterns\": [\"legacy/\"]}",
            ),
            vec![
                ("rules.eqeqeq".to_string(), "lowered from error to off".to_string()),
                ("rules.no-unused-vars".to_string(), "lowered from error to warn".to_string()),
                ("ignorePatterns".to_string(), "gained 1 entr(y/ies): `legacy/`".to_string()),
            ]
        );
    }

    #[test]
    fn golangci_codecov_jest_phpstan_and_phpunit() {
        assert_eq!(
            diff(
                ".golangci.yml",
                "linters:\n  enable: [govet, errcheck]\nissues:\n  exclude-rules: []\n",
                "linters:\n  enable: [govet]\n  disable: [errcheck]\nissues:\n  exclude-rules:\n    - path: _test\\.go\n      linters: [gosec]\n",
            )
            .into_iter()
            .map(|(k, _)| k)
            .collect::<Vec<_>>(),
            vec!["linters.enable", "linters.disable", "issues.exclude-rules"]
        );
        assert_eq!(
            diff(
                "codecov.yml",
                "coverage:\n  status:\n    project:\n      default:\n        target: 80%\n        threshold: 1%\n",
                "coverage:\n  status:\n    project:\n      default:\n        target: 60%\n        threshold: 5%\n",
            ),
            vec![
                ("coverage.status.project.default.target".to_string(), "lowered from 80 to 60".to_string()),
                ("coverage.status.project.default.threshold".to_string(), "raised from 1 to 5".to_string()),
            ]
        );
        assert_eq!(
            diff(
                "package.json",
                "{\"name\": \"a\", \"jest\": {\"coverageThreshold\": {\"global\": {\"lines\": 90}}}}",
                "{\"name\": \"a\", \"version\": \"2\", \"jest\": {\"coverageThreshold\": {\"global\": {\"lines\": 50}}}}",
            ),
            vec![("jest.coverageThreshold.global.lines".to_string(), "lowered from 90 to 50".to_string())]
        );
        assert_eq!(
            diff(
                "phpstan.neon",
                "parameters:\n  level: 8\n  ignoreErrors: []\n",
                "parameters:\n  level: 5\n  ignoreErrors:\n    - '#Call to undefined#'\n"
            )
            .into_iter()
            .map(|(k, _)| k)
            .collect::<Vec<_>>(),
            vec!["parameters.level", "parameters.ignoreErrors"]
        );
        assert_eq!(
            diff(
                "phpunit.xml.dist",
                "<?xml version=\"1.0\"?>\n<phpunit bootstrap=\"vendor/autoload.php\" failOnWarning=\"true\">\n</phpunit>\n",
                "<?xml version=\"1.0\"?>\n<phpunit bootstrap=\"vendor/autoload.php\" failOnWarning=\"false\">\n</phpunit>\n",
            ),
            vec![("phpunit.failOnWarning".to_string(), "switched off".to_string())]
        );
    }

    #[test]
    fn classification_covers_executables_nested_paths_and_ignores_the_rest() {
        assert!(matches!(
            classify("eslint.config.js"),
            Some(Classified::Executable)
        ));
        assert!(matches!(
            classify("web/jest.config.ts"),
            Some(Classified::Executable)
        ));
        assert!(matches!(
            classify("crates/a/.cargo/config.toml"),
            Some(Classified::Data { .. })
        ));
        assert!(matches!(
            classify("tsconfig.app.json"),
            Some(Classified::Data { .. })
        ));
        assert!(classify("src/main.rs").is_none());
        assert!(classify("docs/config.toml").is_none());
        assert!(classify("tsconfig.json.bak").is_none());
        assert!(load("tsconfig.json", "{ nope").is_none());
    }

    #[test]
    fn clippy_toml_thresholds_are_caps_and_lists_go_their_way() {
        let got = diff(
            "clippy.toml",
            "too-many-arguments-threshold = 7\ncognitive-complexity-threshold = 25\ndisallowed-methods = [\"std::env::set_var\"]\nallowed-scripts = [\"Latin\"]\nallow-unwrap-in-tests = false\n",
            "too-many-arguments-threshold = 12\ncognitive-complexity-threshold = 25\ndisallowed-methods = []\nallowed-scripts = [\"Latin\", \"Cyrillic\"]\nallow-unwrap-in-tests = true\n",
        );
        let mut keys: Vec<&str> = got.iter().map(|(k, _)| k.as_str()).collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "allow-unwrap-in-tests",
                "allowed-scripts",
                "disallowed-methods",
                "too-many-arguments-threshold",
            ],
            "{got:?}"
        );
        // Lowering a threshold is a tightening.
        assert!(diff(
            "clippy.toml",
            "too-many-lines-threshold = 100\n",
            "too-many-lines-threshold = 50\n"
        )
        .is_empty());
    }

    #[test]
    fn a_gained_or_swapped_inheritance_is_named_not_passed() {
        let b = load(
            "tsconfig.json",
            "{\"extends\": \"@tsconfig/strictest/tsconfig.json\"}",
        )
        .unwrap();
        let h = load(
            "tsconfig.json",
            "{\"extends\": \"@tsconfig/recommended/tsconfig.json\"}",
        )
        .unwrap();
        assert_eq!(
            inherited_changes("tsconfig.json", &b, &h),
            vec![(
                "extends".to_string(),
                "`@tsconfig/recommended/tsconfig.json`".to_string()
            )]
        );
        assert!(inherited_changes("tsconfig.json", &h, &h).is_empty());
        let b = load(
            ".eslintrc.json",
            "{\"extends\": [\"eslint:recommended\"], \"plugins\": []}",
        )
        .unwrap();
        let h = load(
            ".eslintrc.json",
            "{\"extends\": [\"eslint:recommended\", \"plugin:x/lax\"], \"plugins\": [\"x\"]}",
        )
        .unwrap();
        let got = inherited_changes(".eslintrc.json", &b, &h);
        assert_eq!(got.len(), 2, "{got:?}");
        // Losing an entry is `Shrunk`'s finding, not this one.
        assert!(inherited_changes(".eslintrc.json", &h, &b).is_empty());
    }

    #[test]
    fn package_json_is_strictly_parsed_as_json() {
        assert!(load("package.json", "{\"name\": \"a\",}\n").is_none());
        assert!(load("package.json", "{\n  // comment\n  \"name\": \"a\"\n}\n").is_none());
        assert!(load("package.json", "{\"name\": \"a\"}\n").is_some());
        // Relative / nested paths are also strictly parsed.
        assert!(load("packages/sub/package.json", "{\"name\": \"a\",}\n").is_none());
        assert!(load("packages/sub/package.json", "{\"name\": \"a\"}\n").is_some());
        // Control: tsconfig.json allows comments and trailing commas.
        assert!(load("tsconfig.json", "{\"name\": \"a\",}\n").is_some());
    }
}
