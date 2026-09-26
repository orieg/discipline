//! `toolchain-config`: a change cannot quietly lower the compiler, linter, type-checker,
//! test-runner or coverage bar it is judged by.
//!
//! `config-integrity` guards `discipline.toml`; this gate does the same for the files that
//! configure the rest of the toolchain. Each recognised file is loaded, on the base side
//! and the head side, into one generic tree (TOML, YAML, JSON with comments and INI all
//! land in `serde_json::Value`), and a rule table says which key paths loosen in which
//! direction. A configuration written as code (`eslint.config.js`, `jest.config.ts`) has no
//! tree to compare: a change to it is reported as not analysed, never passed in silence.

use super::{Context, GateOutcome, PathFilter};
use crate::config::{GateSettings, Severity};
use crate::gitctx::ChangeKind;
use crate::tokens;
use anyhow::Result;
use serde_json::Value;

/// How a key path moves the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Judge {
    /// List: a gained entry loosens; so does the list appearing where there was none.
    Grown,
    /// List: a lost entry loosens; so does the list disappearing. Appearing does not.
    Shrunk,
    /// Number: a smaller value loosens; so does removal.
    Floor,
    /// Number: a larger value loosens; appearing counts as rising from zero.
    Cap,
    /// Boolean: `true` loosens; appearing as `true` counts.
    LooserWhenTrue,
    /// Boolean: `false` loosens; `true` disappearing counts.
    LooserWhenFalse,
    /// Lint level (`off`/`allow` < `warn` < `error`/`deny`/`forbid`): lowering loosens.
    LintLevel,
    /// Command-line flags (string or list): losing a strict flag or gaining a lax one
    /// loosens.
    Flags,
}

/// One rule: files it applies to (basename or `dir/basename` glob), the key path inside
/// the tree (`*` matches one segment), and the direction.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    pub files: &'static [&'static str],
    pub path: &'static str,
    pub judge: Judge,
}

const TSCONFIG: &[&str] = &["tsconfig.json", "tsconfig.*.json", "jsconfig.json"];
const RUFF: &[&str] = &["ruff.toml", ".ruff.toml"];
const PYPROJECT: &[&str] = &["pyproject.toml"];
const MYPY_INI: &[&str] = &["mypy.ini", ".mypy.ini", "setup.cfg"];
const PYTEST_INI: &[&str] = &["pytest.ini", "tox.ini", "setup.cfg"];
const COVERAGERC: &[&str] = &[".coveragerc"];
const FLAKE8: &[&str] = &[".flake8", "setup.cfg", "tox.ini"];
const CARGO: &[&str] = &["Cargo.toml"];
const CARGO_CONFIG: &[&str] = &[".cargo/config.toml", ".cargo/config"];
const ESLINTRC: &[&str] = &[
    ".eslintrc",
    ".eslintrc.json",
    ".eslintrc.yml",
    ".eslintrc.yaml",
];
const GOLANGCI: &[&str] = &[
    ".golangci.yml",
    ".golangci.yaml",
    ".golangci.toml",
    ".golangci.json",
];
const JEST: &[&str] = &["jest.config.json", "package.json"];
const CODECOV: &[&str] = &["codecov.yml", ".codecov.yml"];
const NEXTEST: &[&str] = &[".config/nextest.toml"];
const PHPSTAN: &[&str] = &["phpstan.neon", "phpstan.neon.dist", "phpstan.dist.neon"];
const PHPUNIT: &[&str] = &["phpunit.xml", "phpunit.xml.dist"];
const CLIPPY: &[&str] = &["clippy.toml", ".clippy.toml"];

/// Configurations written as code: read for a change, not for a delta.
const EXECUTABLE: &[&str] = &[
    "eslint.config.js",
    "eslint.config.mjs",
    "eslint.config.cjs",
    "eslint.config.ts",
    ".eslintrc.js",
    ".eslintrc.cjs",
    "jest.config.js",
    "jest.config.ts",
    "jest.config.mjs",
    "jest.config.cjs",
    "vitest.config.js",
    "vitest.config.ts",
    "vitest.config.mjs",
    "vitest.config.mts",
    "conftest.py",
];

pub const RULES: &[Rule] = &[
    // clippy.toml: every threshold is a cap, allow-lists grow, disallow-lists shrink.
    Rule {
        files: CLIPPY,
        path: "too-many-arguments-threshold",
        judge: Judge::Cap,
    },
    Rule {
        files: CLIPPY,
        path: "cognitive-complexity-threshold",
        judge: Judge::Cap,
    },
    Rule {
        files: CLIPPY,
        path: "type-complexity-threshold",
        judge: Judge::Cap,
    },
    Rule {
        files: CLIPPY,
        path: "too-many-lines-threshold",
        judge: Judge::Cap,
    },
    Rule {
        files: CLIPPY,
        path: "enum-variant-size-threshold",
        judge: Judge::Cap,
    },
    Rule {
        files: CLIPPY,
        path: "array-size-threshold",
        judge: Judge::Cap,
    },
    Rule {
        files: CLIPPY,
        path: "vec-box-size-threshold",
        judge: Judge::Cap,
    },
    Rule {
        files: CLIPPY,
        path: "max-fn-params-bools",
        judge: Judge::Cap,
    },
    Rule {
        files: CLIPPY,
        path: "max-struct-bools",
        judge: Judge::Cap,
    },
    Rule {
        files: CLIPPY,
        path: "single-char-binding-names-threshold",
        judge: Judge::Cap,
    },
    Rule {
        files: CLIPPY,
        path: "trivial-copy-size-limit",
        judge: Judge::Cap,
    },
    Rule {
        files: CLIPPY,
        path: "large-error-threshold",
        judge: Judge::Cap,
    },
    Rule {
        files: CLIPPY,
        path: "stack-size-threshold",
        judge: Judge::Cap,
    },
    Rule {
        files: CLIPPY,
        path: "max-trait-bounds",
        judge: Judge::Cap,
    },
    Rule {
        files: CLIPPY,
        path: "max-include-file-size",
        judge: Judge::Cap,
    },
    Rule {
        files: CLIPPY,
        path: "excessive-nesting-threshold",
        judge: Judge::Cap,
    },
    Rule {
        files: CLIPPY,
        path: "too-large-for-stack",
        judge: Judge::Cap,
    },
    Rule {
        files: CLIPPY,
        path: "unnecessary-box-size",
        judge: Judge::Cap,
    },
    Rule {
        files: CLIPPY,
        path: "literal-representation-threshold",
        judge: Judge::Cap,
    },
    Rule {
        files: CLIPPY,
        path: "min-ident-chars-threshold",
        judge: Judge::Floor,
    },
    Rule {
        files: CLIPPY,
        path: "enum-variant-name-threshold",
        judge: Judge::Floor,
    },
    Rule {
        files: CLIPPY,
        path: "struct-field-name-threshold",
        judge: Judge::Floor,
    },
    Rule {
        files: CLIPPY,
        path: "unreadable-literal-lint-fractions",
        judge: Judge::Floor,
    },
    Rule {
        files: CLIPPY,
        path: "allowed-idents-below-min-chars",
        judge: Judge::Grown,
    },
    Rule {
        files: CLIPPY,
        path: "allowed-duplicate-crates",
        judge: Judge::Grown,
    },
    Rule {
        files: CLIPPY,
        path: "allowed-prefixes",
        judge: Judge::Grown,
    },
    Rule {
        files: CLIPPY,
        path: "allowed-scripts",
        judge: Judge::Grown,
    },
    Rule {
        files: CLIPPY,
        path: "allowed-wildcard-imports",
        judge: Judge::Grown,
    },
    Rule {
        files: CLIPPY,
        path: "allowed-dotfiles",
        judge: Judge::Grown,
    },
    Rule {
        files: CLIPPY,
        path: "ignore-interior-mutability",
        judge: Judge::Grown,
    },
    Rule {
        files: CLIPPY,
        path: "arithmetic-side-effects-allowed",
        judge: Judge::Grown,
    },
    Rule {
        files: CLIPPY,
        path: "arithmetic-side-effects-allowed-binary",
        judge: Judge::Grown,
    },
    Rule {
        files: CLIPPY,
        path: "arithmetic-side-effects-allowed-unary",
        judge: Judge::Grown,
    },
    Rule {
        files: CLIPPY,
        path: "allow-renamed-params-for",
        judge: Judge::Grown,
    },
    Rule {
        files: CLIPPY,
        path: "doc-valid-idents",
        judge: Judge::Grown,
    },
    Rule {
        files: CLIPPY,
        path: "disallowed-methods",
        judge: Judge::Shrunk,
    },
    Rule {
        files: CLIPPY,
        path: "disallowed-types",
        judge: Judge::Shrunk,
    },
    Rule {
        files: CLIPPY,
        path: "disallowed-macros",
        judge: Judge::Shrunk,
    },
    Rule {
        files: CLIPPY,
        path: "disallowed-names",
        judge: Judge::Shrunk,
    },
    Rule {
        files: CLIPPY,
        path: "await-holding-invalid-types",
        judge: Judge::Shrunk,
    },
    Rule {
        files: CLIPPY,
        path: "allow-unwrap-in-tests",
        judge: Judge::LooserWhenTrue,
    },
    Rule {
        files: CLIPPY,
        path: "allow-expect-in-tests",
        judge: Judge::LooserWhenTrue,
    },
    Rule {
        files: CLIPPY,
        path: "allow-dbg-in-tests",
        judge: Judge::LooserWhenTrue,
    },
    Rule {
        files: CLIPPY,
        path: "allow-print-in-tests",
        judge: Judge::LooserWhenTrue,
    },
    Rule {
        files: CLIPPY,
        path: "allow-panic-in-tests",
        judge: Judge::LooserWhenTrue,
    },
    Rule {
        files: CLIPPY,
        path: "allow-mixed-uninlined-format-args",
        judge: Judge::LooserWhenTrue,
    },
    Rule {
        files: CLIPPY,
        path: "allow-one-hash-in-raw-strings",
        judge: Judge::LooserWhenTrue,
    },
    Rule {
        files: CLIPPY,
        path: "allow-private-module-inception",
        judge: Judge::LooserWhenTrue,
    },
    Rule {
        files: CLIPPY,
        path: "allow-useless-vec-in-tests",
        judge: Judge::LooserWhenTrue,
    },
    Rule {
        files: CLIPPY,
        path: "allow-comparison-to-zero",
        judge: Judge::LooserWhenTrue,
    },
    Rule {
        files: CLIPPY,
        path: "allow-indexing-slicing-in-tests",
        judge: Judge::LooserWhenTrue,
    },
    Rule {
        files: CLIPPY,
        path: "allow-unwrap-in-consts",
        judge: Judge::LooserWhenTrue,
    },
    Rule {
        files: CLIPPY,
        path: "allow-exact-repetitions",
        judge: Judge::LooserWhenTrue,
    },
    // TypeScript
    Rule {
        files: TSCONFIG,
        path: "compilerOptions.strict",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: TSCONFIG,
        path: "compilerOptions.noImplicitAny",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: TSCONFIG,
        path: "compilerOptions.strictNullChecks",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: TSCONFIG,
        path: "compilerOptions.strictFunctionTypes",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: TSCONFIG,
        path: "compilerOptions.noUnusedLocals",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: TSCONFIG,
        path: "compilerOptions.noUnusedParameters",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: TSCONFIG,
        path: "compilerOptions.noImplicitReturns",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: TSCONFIG,
        path: "compilerOptions.noUncheckedIndexedAccess",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: TSCONFIG,
        path: "compilerOptions.noFallthroughCasesInSwitch",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: TSCONFIG,
        path: "compilerOptions.skipLibCheck",
        judge: Judge::LooserWhenTrue,
    },
    Rule {
        files: TSCONFIG,
        path: "compilerOptions.allowJs",
        judge: Judge::LooserWhenTrue,
    },
    Rule {
        files: TSCONFIG,
        path: "exclude",
        judge: Judge::Grown,
    },
    Rule {
        files: TSCONFIG,
        path: "include",
        judge: Judge::Shrunk,
    },
    // Ruff (standalone and pyproject)
    Rule {
        files: RUFF,
        path: "select",
        judge: Judge::Shrunk,
    },
    Rule {
        files: RUFF,
        path: "extend-select",
        judge: Judge::Shrunk,
    },
    Rule {
        files: RUFF,
        path: "ignore",
        judge: Judge::Grown,
    },
    Rule {
        files: RUFF,
        path: "extend-ignore",
        judge: Judge::Grown,
    },
    Rule {
        files: RUFF,
        path: "exclude",
        judge: Judge::Grown,
    },
    Rule {
        files: RUFF,
        path: "extend-exclude",
        judge: Judge::Grown,
    },
    Rule {
        files: RUFF,
        path: "per-file-ignores.*",
        judge: Judge::Grown,
    },
    Rule {
        files: RUFF,
        path: "lint.select",
        judge: Judge::Shrunk,
    },
    Rule {
        files: RUFF,
        path: "lint.extend-select",
        judge: Judge::Shrunk,
    },
    Rule {
        files: RUFF,
        path: "lint.ignore",
        judge: Judge::Grown,
    },
    Rule {
        files: RUFF,
        path: "lint.extend-ignore",
        judge: Judge::Grown,
    },
    Rule {
        files: RUFF,
        path: "lint.per-file-ignores.*",
        judge: Judge::Grown,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.ruff.select",
        judge: Judge::Shrunk,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.ruff.extend-select",
        judge: Judge::Shrunk,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.ruff.ignore",
        judge: Judge::Grown,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.ruff.extend-ignore",
        judge: Judge::Grown,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.ruff.exclude",
        judge: Judge::Grown,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.ruff.extend-exclude",
        judge: Judge::Grown,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.ruff.per-file-ignores.*",
        judge: Judge::Grown,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.ruff.lint.select",
        judge: Judge::Shrunk,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.ruff.lint.extend-select",
        judge: Judge::Shrunk,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.ruff.lint.ignore",
        judge: Judge::Grown,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.ruff.lint.extend-ignore",
        judge: Judge::Grown,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.ruff.lint.per-file-ignores.*",
        judge: Judge::Grown,
    },
    // mypy (pyproject and INI)
    Rule {
        files: PYPROJECT,
        path: "tool.mypy.strict",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.mypy.disallow_untyped_defs",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.mypy.disallow_any_generics",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.mypy.warn_return_any",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.mypy.warn_unused_ignores",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.mypy.ignore_missing_imports",
        judge: Judge::LooserWhenTrue,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.mypy.ignore_errors",
        judge: Judge::LooserWhenTrue,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.mypy.disable_error_code",
        judge: Judge::Grown,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.mypy.exclude",
        judge: Judge::Grown,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.mypy.overrides",
        judge: Judge::Grown,
    },
    Rule {
        files: MYPY_INI,
        path: "mypy.strict",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: MYPY_INI,
        path: "mypy.disallow_untyped_defs",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: MYPY_INI,
        path: "mypy.warn_unused_ignores",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: MYPY_INI,
        path: "mypy.ignore_missing_imports",
        judge: Judge::LooserWhenTrue,
    },
    Rule {
        files: MYPY_INI,
        path: "mypy.ignore_errors",
        judge: Judge::LooserWhenTrue,
    },
    Rule {
        files: MYPY_INI,
        path: "mypy.disable_error_code",
        judge: Judge::Grown,
    },
    Rule {
        files: MYPY_INI,
        path: "mypy.exclude",
        judge: Judge::Grown,
    },
    Rule {
        files: MYPY_INI,
        path: "mypy-*.ignore_errors",
        judge: Judge::LooserWhenTrue,
    },
    Rule {
        files: MYPY_INI,
        path: "mypy-*.ignore_missing_imports",
        judge: Judge::LooserWhenTrue,
    },
    // pytest
    Rule {
        files: PYPROJECT,
        path: "tool.pytest.ini_options.addopts",
        judge: Judge::Flags,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.pytest.ini_options.xfail_strict",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.pytest.ini_options.filterwarnings",
        judge: Judge::Shrunk,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.pytest.ini_options.testpaths",
        judge: Judge::Shrunk,
    },
    Rule {
        files: PYTEST_INI,
        path: "pytest.addopts",
        judge: Judge::Flags,
    },
    Rule {
        files: PYTEST_INI,
        path: "pytest.xfail_strict",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: PYTEST_INI,
        path: "pytest.testpaths",
        judge: Judge::Shrunk,
    },
    Rule {
        files: PYTEST_INI,
        path: "tool:pytest.addopts",
        judge: Judge::Flags,
    },
    Rule {
        files: PYTEST_INI,
        path: "tool:pytest.xfail_strict",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: PYTEST_INI,
        path: "tool:pytest.testpaths",
        judge: Judge::Shrunk,
    },
    // coverage.py
    Rule {
        files: PYPROJECT,
        path: "tool.coverage.report.fail_under",
        judge: Judge::Floor,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.coverage.report.omit",
        judge: Judge::Grown,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.coverage.report.exclude_lines",
        judge: Judge::Grown,
    },
    Rule {
        files: PYPROJECT,
        path: "tool.coverage.run.omit",
        judge: Judge::Grown,
    },
    Rule {
        files: COVERAGERC,
        path: "report.fail_under",
        judge: Judge::Floor,
    },
    Rule {
        files: COVERAGERC,
        path: "report.omit",
        judge: Judge::Grown,
    },
    Rule {
        files: COVERAGERC,
        path: "report.exclude_lines",
        judge: Judge::Grown,
    },
    Rule {
        files: COVERAGERC,
        path: "run.omit",
        judge: Judge::Grown,
    },
    // flake8
    Rule {
        files: FLAKE8,
        path: "flake8.ignore",
        judge: Judge::Grown,
    },
    Rule {
        files: FLAKE8,
        path: "flake8.extend-ignore",
        judge: Judge::Grown,
    },
    Rule {
        files: FLAKE8,
        path: "flake8.exclude",
        judge: Judge::Grown,
    },
    Rule {
        files: FLAKE8,
        path: "flake8.per-file-ignores",
        judge: Judge::Grown,
    },
    Rule {
        files: FLAKE8,
        path: "flake8.max-line-length",
        judge: Judge::Cap,
    },
    Rule {
        files: FLAKE8,
        path: "flake8.max-complexity",
        judge: Judge::Cap,
    },
    // Rust
    Rule {
        files: CARGO,
        path: "lints.*.*",
        judge: Judge::LintLevel,
    },
    Rule {
        files: CARGO,
        path: "workspace.lints.*.*",
        judge: Judge::LintLevel,
    },
    Rule {
        files: CARGO_CONFIG,
        path: "build.rustflags",
        judge: Judge::Flags,
    },
    Rule {
        files: CARGO_CONFIG,
        path: "build.rustdocflags",
        judge: Judge::Flags,
    },
    Rule {
        files: CARGO_CONFIG,
        path: "target.*.rustflags",
        judge: Judge::Flags,
    },
    Rule {
        files: NEXTEST,
        path: "profile.*.retries",
        judge: Judge::Cap,
    },
    Rule {
        files: NEXTEST,
        path: "profile.*.retries.count",
        judge: Judge::Cap,
    },
    Rule {
        files: NEXTEST,
        path: "profile.*.fail-fast",
        judge: Judge::LooserWhenFalse,
    },
    // ESLint (data forms)
    Rule {
        files: ESLINTRC,
        path: "rules.*",
        judge: Judge::LintLevel,
    },
    Rule {
        files: ESLINTRC,
        path: "overrides.*.rules.*",
        judge: Judge::LintLevel,
    },
    Rule {
        files: ESLINTRC,
        path: "ignorePatterns",
        judge: Judge::Grown,
    },
    Rule {
        files: ESLINTRC,
        path: "extends",
        judge: Judge::Shrunk,
    },
    // golangci-lint
    Rule {
        files: GOLANGCI,
        path: "linters.enable",
        judge: Judge::Shrunk,
    },
    Rule {
        files: GOLANGCI,
        path: "linters.disable",
        judge: Judge::Grown,
    },
    Rule {
        files: GOLANGCI,
        path: "linters.enable-all",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: GOLANGCI,
        path: "linters.disable-all",
        judge: Judge::LooserWhenTrue,
    },
    Rule {
        files: GOLANGCI,
        path: "issues.exclude",
        judge: Judge::Grown,
    },
    Rule {
        files: GOLANGCI,
        path: "issues.exclude-rules",
        judge: Judge::Grown,
    },
    Rule {
        files: GOLANGCI,
        path: "issues.exclude-dirs",
        judge: Judge::Grown,
    },
    Rule {
        files: GOLANGCI,
        path: "issues.exclude-files",
        judge: Judge::Grown,
    },
    Rule {
        files: GOLANGCI,
        path: "issues.max-issues-per-linter",
        judge: Judge::Cap,
    },
    Rule {
        files: GOLANGCI,
        path: "issues.max-same-issues",
        judge: Judge::Cap,
    },
    Rule {
        files: GOLANGCI,
        path: "run.skip-dirs",
        judge: Judge::Grown,
    },
    Rule {
        files: GOLANGCI,
        path: "run.skip-files",
        judge: Judge::Grown,
    },
    Rule {
        files: GOLANGCI,
        path: "linters.exclusions.rules",
        judge: Judge::Grown,
    },
    Rule {
        files: GOLANGCI,
        path: "linters.exclusions.paths",
        judge: Judge::Grown,
    },
    // Coverage thresholds and retries (JS)
    Rule {
        files: JEST,
        path: "coverageThreshold.global.*",
        judge: Judge::Floor,
    },
    Rule {
        files: JEST,
        path: "coverageThreshold.*.*",
        judge: Judge::Floor,
    },
    Rule {
        files: JEST,
        path: "testPathIgnorePatterns",
        judge: Judge::Grown,
    },
    Rule {
        files: JEST,
        path: "coveragePathIgnorePatterns",
        judge: Judge::Grown,
    },
    Rule {
        files: JEST,
        path: "jest.coverageThreshold.global.*",
        judge: Judge::Floor,
    },
    Rule {
        files: JEST,
        path: "jest.coverageThreshold.*.*",
        judge: Judge::Floor,
    },
    Rule {
        files: JEST,
        path: "jest.testPathIgnorePatterns",
        judge: Judge::Grown,
    },
    Rule {
        files: JEST,
        path: "jest.coveragePathIgnorePatterns",
        judge: Judge::Grown,
    },
    Rule {
        files: CODECOV,
        path: "coverage.status.project.*.target",
        judge: Judge::Floor,
    },
    Rule {
        files: CODECOV,
        path: "coverage.status.patch.*.target",
        judge: Judge::Floor,
    },
    Rule {
        files: CODECOV,
        path: "coverage.status.project.*.threshold",
        judge: Judge::Cap,
    },
    Rule {
        files: CODECOV,
        path: "coverage.status.patch.*.threshold",
        judge: Judge::Cap,
    },
    Rule {
        files: CODECOV,
        path: "coverage.status.project",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: CODECOV,
        path: "coverage.status.patch",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: CODECOV,
        path: "ignore",
        judge: Judge::Grown,
    },
    Rule {
        files: CODECOV,
        path: "codecov.require_ci_to_pass",
        judge: Judge::LooserWhenFalse,
    },
    // PHP
    Rule {
        files: PHPSTAN,
        path: "parameters.level",
        judge: Judge::Floor,
    },
    Rule {
        files: PHPSTAN,
        path: "parameters.ignoreErrors",
        judge: Judge::Grown,
    },
    Rule {
        files: PHPSTAN,
        path: "parameters.excludePaths",
        judge: Judge::Grown,
    },
    Rule {
        files: PHPSTAN,
        path: "parameters.excludePaths.analyse",
        judge: Judge::Grown,
    },
    Rule {
        files: PHPSTAN,
        path: "parameters.reportUnmatchedIgnoredErrors",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: PHPUNIT,
        path: "phpunit.failOnWarning",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: PHPUNIT,
        path: "phpunit.failOnRisky",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: PHPUNIT,
        path: "phpunit.failOnIncomplete",
        judge: Judge::LooserWhenFalse,
    },
    Rule {
        files: PHPUNIT,
        path: "phpunit.failOnSkipped",
        judge: Judge::LooserWhenFalse,
    },
];

/// Flags whose loss loosens the run.
const STRICT_FLAGS: &[&str] = &[
    "-Dwarnings",
    "-D",
    "--deny",
    "-F",
    "--forbid",
    "-Werror",
    "--strict-markers",
    "--strict-config",
    "--strict",
    "-W",
    "--locked",
    "--frozen",
    "--frozen-lockfile",
    "--require-hashes",
    "--cov-fail-under",
    "-Wall",
    "-Wextra",
    "-Wpedantic",
];
/// Flags whose gain loosens the run.
const LAX_FLAGS: &[&str] = &[
    "--reruns",
    "--reruns-delay",
    "--ignore",
    "--ignore-glob",
    "--deselect",
    "--no-cov",
    "--continue-on-collection-errors",
    "--runxfail",
    "-Awarnings",
    "-A",
    "--allow",
    "--cap-lints",
    "-w",
    "--no-verify",
    "--no-strict",
];

/// Whether `path` is one of the rule files (by basename, or `dir/basename` for the dotted
/// directories), or an executable configuration.
pub fn classify(path: &str) -> Option<Classified> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let tail2 = {
        let mut it = path.rsplitn(3, '/');
        let a = it.next().unwrap_or("");
        match it.next() {
            Some(b) => format!("{b}/{a}"),
            None => a.to_string(),
        }
    };
    if EXECUTABLE.contains(&name) {
        return Some(Classified::Executable);
    }
    let matches = |pat: &str| {
        if let Some(prefix) = pat.strip_suffix(".*.json") {
            name.starts_with(&format!("{prefix}."))
                && name.ends_with(".json")
                && name.len() > prefix.len() + 6
        } else if pat.contains('/') {
            tail2 == pat
        } else {
            name == pat
        }
    };
    let rules: Vec<&'static Rule> = RULES
        .iter()
        .filter(|r| r.files.iter().any(|f| matches(f)))
        .collect();
    if rules.is_empty() {
        return None;
    }
    Some(Classified::Data {
        name: name.to_string(),
        rules,
    })
}

#[derive(Debug)]
pub enum Classified {
    Executable,
    Data {
        name: String,
        rules: Vec<&'static Rule>,
    },
}

/// Strip `//` and `/* */` comments and trailing commas outside strings (JSON with comments,
/// as `tsconfig.json` and `.eslintrc` allow).
fn strip_jsonc(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let b: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < b.len() {
                out.push(b[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
                i += 1;
            }
            '/' if b.get(i + 1) == Some(&'/') => {
                while i < b.len() && b[i] != '\n' {
                    i += 1;
                }
            }
            '/' if b.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            ',' => {
                // Drop the comma when the next non-space char closes a container.
                let mut j = i + 1;
                while j < b.len() && b[j].is_whitespace() {
                    j += 1;
                }
                if !(j < b.len() && (b[j] == '}' || b[j] == ']')) {
                    out.push(c);
                }
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// A minimal INI reader: `[section]`, `key = value` / `key: value`, `;`/`#` comments,
/// indented continuation lines joined into a list. Values stay strings; lists are split
/// on newlines and commas.
fn parse_ini(src: &str) -> Value {
    let mut root = serde_json::Map::new();
    let mut section = String::new();
    let mut last_key: Option<String> = None;
    for raw in src.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with([';', '#']) {
            continue;
        }
        if let Some(name) = line
            .trim()
            .strip_prefix('[')
            .and_then(|l| l.strip_suffix(']'))
        {
            section = name.trim().to_string();
            root.entry(section.clone())
                .or_insert(Value::Object(Default::default()));
            last_key = None;
            continue;
        }
        let is_continuation = raw.starts_with([' ', '\t']);
        let table = root
            .entry(section.clone())
            .or_insert(Value::Object(Default::default()))
            .as_object_mut()
            .expect("sections are objects");
        if is_continuation {
            if let Some(k) = &last_key {
                let existing = table.get(k).cloned().unwrap_or(Value::Null);
                let mut items: Vec<Value> = match existing {
                    Value::Array(a) => a,
                    Value::String(s) if s.is_empty() => Vec::new(),
                    Value::String(s) => vec![Value::String(s)],
                    _ => Vec::new(),
                };
                items.push(Value::String(line.trim().to_string()));
                table.insert(k.clone(), Value::Array(items));
            }
            continue;
        }
        let Some((k, v)) = line.split_once(['=', ':']) else {
            continue;
        };
        let key = k.trim().to_string();
        let value = v.trim();
        let parsed = if value.contains(',') {
            Value::Array(
                value
                    .split(',')
                    .map(|x| Value::String(x.trim().to_string()))
                    .filter(|x| x.as_str() != Some(""))
                    .collect(),
            )
        } else {
            Value::String(value.to_string())
        };
        table.insert(key.clone(), parsed);
        last_key = Some(key);
    }
    Value::Object(root)
}

/// A minimal XML reader for PHPUnit: root element attributes only.
fn parse_phpunit_xml(src: &str) -> Option<Value> {
    let start = src.find("<phpunit")?;
    let end = src[start..].find('>')? + start;
    let tag = &src[start + "<phpunit".len()..end];
    let mut attrs = serde_json::Map::new();
    let mut rest = tag;
    while let Some(eq) = rest.find('=') {
        let key = rest[..eq]
            .trim()
            .rsplit(char::is_whitespace)
            .next()?
            .to_string();
        let after = rest[eq + 1..].trim_start();
        let quote = after.chars().next()?;
        let close = after[1..].find(quote)? + 1;
        attrs.insert(key, Value::String(after[1..close].to_string()));
        rest = &after[close + 1..];
    }
    let mut root = serde_json::Map::new();
    root.insert("phpunit".to_string(), Value::Object(attrs));
    Some(Value::Object(root))
}

/// Load a configuration file into one generic tree. `None` = not parseable as its format.
pub fn load(name: &str, content: &str) -> Option<Value> {
    if name.ends_with(".toml") || name == ".cargo/config" || name == "config" {
        let v: toml::Value = toml::from_str(content).ok()?;
        serde_json::to_value(v).ok()
    } else if name.ends_with(".json") || name == ".eslintrc" {
        serde_json::from_str(&strip_jsonc(content)).ok()
    } else if name.ends_with(".yml") || name.ends_with(".yaml") {
        let v: serde_yaml::Value = serde_yaml::from_str(content).ok()?;
        serde_json::to_value(v).ok()
    } else if name.ends_with(".neon") {
        // NEON is YAML-shaped for the keys this gate reads.
        let v: serde_yaml::Value = serde_yaml::from_str(content).ok()?;
        serde_json::to_value(v).ok()
    } else if name.ends_with(".xml") || name.ends_with(".xml.dist") {
        parse_phpunit_xml(content)
    } else {
        Some(parse_ini(content))
    }
}

/// Whether one pattern segment (`*`, `mypy-*`, or a literal) matches a key. A key may
/// itself contain dots (`mypy-thirdparty.*`): keys are never split.
fn segment_matches(pattern: &str, key: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == key;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !key.starts_with(first) || !key.ends_with(last) || key.len() < first.len() + last.len() {
        return false;
    }
    let mut rest = &key[first.len()..key.len() - last.len()];
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(i) => rest = &rest[i + mid.len()..],
            None => return false,
        }
    }
    true
}

/// All (path, value) pairs in `tree` matching a dotted pattern where `*` is one segment.
fn matching<'a>(tree: &'a Value, pattern: &str) -> Vec<(String, &'a Value)> {
    fn walk<'a>(
        v: &'a Value,
        segs: &[&str],
        prefix: Vec<String>,
        out: &mut Vec<(String, &'a Value)>,
    ) {
        let Some((first, rest)) = segs.split_first() else {
            out.push((prefix.join("."), v));
            return;
        };
        let Some(map) = v.as_object() else { return };
        for (k, child) in map {
            if segment_matches(first, k) {
                let mut p = prefix.clone();
                p.push(k.clone());
                walk(child, rest, p, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(
        tree,
        &pattern.split('.').collect::<Vec<_>>(),
        Vec::new(),
        &mut out,
    );
    out
}

fn as_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "1" => Some(true),
            "false" | "no" | "off" | "0" => Some(false),
            _ => None,
        },
        Value::Number(n) => n.as_i64().map(|i| i != 0),
        _ => None,
    }
}

fn as_num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().trim_end_matches('%').parse().ok(),
        _ => None,
    }
}

/// Entries of a list value; a scalar is a one-entry list, a map is its keys.
fn as_list(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a
            .iter()
            .map(|x| match x {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect(),
        Value::Null => Vec::new(),
        Value::Object(m) => m.keys().cloned().collect(),
        Value::String(s) => s
            .split([',', '\n'])
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect(),
        other => vec![other.to_string()],
    }
}

fn lint_level(v: &Value) -> Option<u8> {
    let s = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Array(a) => return a.first().and_then(lint_level),
        Value::Object(m) => return m.get("level").and_then(lint_level),
        _ => return None,
    };
    match s.trim().to_ascii_lowercase().as_str() {
        "off" | "0" | "allow" => Some(0),
        "warn" | "warning" | "1" => Some(1),
        "error" | "2" | "deny" | "forbid" => Some(2),
        _ => None,
    }
}

fn flag_tokens(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => s.split_whitespace().map(str::to_string).collect(),
        Value::Array(a) => a.iter().flat_map(flag_tokens).collect(),
        _ => Vec::new(),
    }
}

/// The strict-vocabulary flags a token list carries, joined with their argument
/// (`-D warnings`, `--cov-fail-under 80`) so a rename is not a loss.
fn strict_flags(tokens: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let t = &tokens[i];
        if STRICT_FLAGS.contains(&t.as_str()) {
            let takes_arg = matches!(
                t.as_str(),
                "-D" | "--deny" | "-F" | "--forbid" | "-W" | "--cov-fail-under"
            );
            if takes_arg && i + 1 < tokens.len() {
                out.push(format!("{t} {}", tokens[i + 1]));
                i += 2;
                continue;
            }
            out.push(t.clone());
        } else if let Some(rest) = t.strip_prefix("--cov-fail-under=") {
            out.push(format!("--cov-fail-under {rest}"));
        }
        i += 1;
    }
    out
}

fn lax_flags(tokens: &[String]) -> Vec<String> {
    tokens
        .iter()
        .filter(|t| {
            let bare = t.split_once('=').map_or(t.as_str(), |(k, _)| k);
            LAX_FLAGS.contains(&bare)
        })
        .cloned()
        .collect()
}

/// One weakening in a configuration file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Weakening {
    /// Key path, e.g. `compilerOptions.strict`.
    pub key: String,
    pub what: String,
}

/// What `head` loosens relative to `base` under `rules`.
/// Keys through which a configuration inherits another one, per file: a gained or swapped
/// entry pulls in settings this diff cannot read.
const INHERITANCE: &[(&[&str], &str)] = &[
    (TSCONFIG, "extends"),
    (ESLINTRC, "extends"),
    (ESLINTRC, "plugins"),
    (ESLINTRC, "overrides.*.extends"),
    (JEST, "preset"),
    (JEST, "jest.preset"),
    (RUFF, "extend"),
    (PYPROJECT, "tool.ruff.extend"),
    (GOLANGCI, "linters.presets"),
];

/// `(key, what was gained)` for every inheritance key of `name` whose head side names an
/// entry the base side did not.
pub fn inherited_changes(name: &str, base: &Value, head: &Value) -> Vec<(String, String)> {
    let applies = |files: &[&str]| {
        files.iter().any(|f| {
            if let Some(prefix) = f.strip_suffix(".*.json") {
                name.starts_with(&format!("{prefix}.")) && name.ends_with(".json")
            } else {
                name == *f || name.ends_with(&format!("/{f}"))
            }
        })
    };
    let mut out = Vec::new();
    for (files, path) in INHERITANCE {
        if !applies(files) {
            continue;
        }
        let b_paths: std::collections::BTreeMap<String, &Value> =
            matching(base, path).into_iter().collect();
        for (key, h) in matching(head, path) {
            let bl = b_paths.get(&key).map(|v| as_list(v)).unwrap_or_default();
            let hl = as_list(h);
            let gained: Vec<&String> = hl.iter().filter(|x| !bl.contains(x)).collect();
            if !gained.is_empty() {
                out.push((key, sample(&gained)));
            }
        }
    }
    out
}

pub fn diff_trees(base: &Value, head: &Value, rules: &[&Rule]) -> Vec<Weakening> {
    let mut found = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for rule in rules {
        let b_paths: std::collections::BTreeMap<String, &Value> =
            matching(base, rule.path).into_iter().collect();
        let h_paths: std::collections::BTreeMap<String, &Value> =
            matching(head, rule.path).into_iter().collect();
        let keys: std::collections::BTreeSet<&String> =
            b_paths.keys().chain(h_paths.keys()).collect();
        for key in keys {
            // Two patterns can name the same key (`coverageThreshold.global.*` and
            // `coverageThreshold.*.*`); judge it once.
            if !seen.insert(key.clone()) {
                continue;
            }
            let b = b_paths.get(key).copied();
            let h = h_paths.get(key).copied();
            let what = match rule.judge {
                Judge::Grown => {
                    let bl = b.map(as_list).unwrap_or_default();
                    let hl = h.map(as_list).unwrap_or_default();
                    let gained: Vec<_> = hl.iter().filter(|x| !bl.contains(x)).collect();
                    (!gained.is_empty()).then(|| {
                        format!("gained {} entr(y/ies): {}", gained.len(), sample(&gained))
                    })
                }
                Judge::Shrunk => {
                    let Some(b) = b else { continue };
                    let bl = as_list(b);
                    let hl = h.map(as_list).unwrap_or_default();
                    let lost: Vec<_> = bl.iter().filter(|x| !hl.contains(x)).collect();
                    (!lost.is_empty()).then(|| {
                        if h.is_none() {
                            format!("removed (was {} entr(y/ies))", bl.len())
                        } else {
                            format!("lost {} entr(y/ies): {}", lost.len(), sample(&lost))
                        }
                    })
                }
                Judge::Floor => match (b.and_then(as_num), h.and_then(as_num)) {
                    (Some(bn), Some(hn)) if hn < bn => Some(format!("lowered from {bn} to {hn}")),
                    (Some(bn), None) if b.is_some() && h.is_none() => {
                        Some(format!("removed (was {bn})"))
                    }
                    _ => None,
                },
                Judge::Cap => {
                    let bn = b.and_then(as_num).unwrap_or(0.0);
                    match h.and_then(as_num) {
                        Some(hn) if hn > bn => Some(format!("raised from {bn} to {hn}")),
                        _ => None,
                    }
                }
                Judge::LooserWhenTrue => match (b.and_then(as_bool), h.and_then(as_bool)) {
                    (Some(true), _) => None,
                    (_, Some(true)) => Some("switched on".to_string()),
                    _ => None,
                },
                Judge::LooserWhenFalse => match (b.and_then(as_bool), h.and_then(as_bool)) {
                    (Some(true), Some(false)) => Some("switched off".to_string()),
                    (Some(true), None) if h.is_none() => Some("removed (was on)".to_string()),
                    _ => None,
                },
                Judge::LintLevel => match (b.and_then(lint_level), h.and_then(lint_level)) {
                    (Some(bl), Some(hl)) if hl < bl => Some(format!(
                        "lowered from {} to {}",
                        level_name(bl),
                        level_name(hl)
                    )),
                    _ => None,
                },
                Judge::Flags => {
                    let bt = b.map(flag_tokens).unwrap_or_default();
                    let ht = h.map(flag_tokens).unwrap_or_default();
                    let (bs, hs) = (strict_flags(&bt), strict_flags(&ht));
                    let lost: Vec<_> = bs.iter().filter(|f| !hs.contains(f)).collect();
                    let (bl, hl) = (lax_flags(&bt), lax_flags(&ht));
                    let gained: Vec<_> = hl.iter().filter(|f| !bl.contains(f)).collect();
                    let mut parts = Vec::new();
                    if !lost.is_empty() {
                        parts.push(format!(
                            "lost `{}`",
                            lost.iter()
                                .map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join("`, `")
                        ));
                    }
                    if !gained.is_empty() {
                        parts.push(format!(
                            "gained `{}`",
                            gained
                                .iter()
                                .map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join("`, `")
                        ));
                    }
                    (!parts.is_empty()).then(|| parts.join("; "))
                }
            };
            if let Some(what) = what {
                found.push(Weakening {
                    key: key.clone(),
                    what,
                });
            }
        }
    }
    found
}

fn level_name(l: u8) -> &'static str {
    match l {
        0 => "off",
        1 => "warn",
        _ => "error",
    }
}

fn sample(items: &[&String]) -> String {
    let shown: Vec<&str> = items.iter().take(3).map(|s| s.as_str()).collect();
    format!(
        "`{}`{}",
        shown.join("`, `"),
        if items.len() > shown.len() {
            ", ..."
        } else {
            ""
        }
    )
}

pub fn toolchain_config(ctx: &Context) -> Result<GateOutcome> {
    const GATE: &str = "toolchain-config";
    let settings = &ctx.config.gates.toolchain_config;
    let mut out = GateOutcome::new(GATE);
    let exempt = PathFilter::new(&settings.exempt_paths)?;

    for file in ctx.git.changed_files()? {
        if exempt.matches(&file.path) {
            continue;
        }
        let Some(class) = classify(&file.path) else {
            continue;
        };
        out.examined += 1;
        let lift =
            |subject: &str| ctx.find_override(GATE, tokens::ALLOW_TOOLCHAIN_WEAKENING, subject);

        match class {
            Classified::Executable => {
                if file.kind == ChangeKind::Added {
                    continue;
                }
                if let Some(ov) = lift(&file.path) {
                    out.overrides.push(ov);
                    continue;
                }
                let sev = match settings.severity() {
                    Severity::Error => Severity::Warning,
                    other => other,
                };
                out.push(
                    ctx.overridable(sev),
                    &crate::findings::TOOLCHAIN_CHANGE_NOT_ANALYSED,
                    Some(&file.path),
                    None,
                    format!(
                        "`{}` is configuration written as code; whether the change loosens it cannot be read from a diff.",
                        file.path
                    ),
                    "Review the change; record it with `allow-toolchain-weakening: <path> <reason>` if it is intended.",
                );
            }
            Classified::Data { name, rules } => {
                let head = ctx.git.head_content(&file.path)?;
                let base = ctx.git.base_content(&file.old_path)?;
                let (Some(base_src), Some(head_src)) = (base, head) else {
                    // A file that appears has no bar to lower; one that disappears is
                    // reported: its settings no longer apply.
                    if file.kind == ChangeKind::Deleted {
                        if let Some(ov) = lift(&file.path) {
                            out.overrides.push(ov);
                        } else {
                            out.push(
                                ctx.overridable(settings.severity()),
                                &crate::findings::TOOLCHAIN_CONFIG_DELETED,
                                Some(&file.path),
                                None,
                                format!("`{}` was deleted; the settings it carried no longer apply.", file.path),
                                "Restore it, or record the deletion with `allow-toolchain-weakening: <path> <reason>`.",
                            );
                        }
                    }
                    continue;
                };
                let (Some(base_tree), Some(head_tree)) =
                    (load(&name, &base_src), load(&name, &head_src))
                else {
                    out.push(
                        settings.severity(),
                        &crate::findings::TOOLCHAIN_CONFIG_UNREADABLE,
                        Some(&file.path),
                        None,
                        format!("`{}` could not be parsed on one side, so its weakening could not be checked.", file.path),
                        "Fix the file so it parses.",
                    );
                    continue;
                };
                for (key, gained) in inherited_changes(&name, &base_tree, &head_tree) {
                    if let Some(ov) = lift(&key).or_else(|| lift(&file.path)) {
                        out.overrides.push(ov);
                        continue;
                    }
                    let sev = match settings.severity() {
                        Severity::Error => Severity::Warning,
                        other => other,
                    };
                    out.push(
                        ctx.overridable(sev),
                        &crate::findings::TOOLCHAIN_CHANGE_NOT_ANALYSED,
                        Some(&file.path),
                        None,
                        format!(
                            "`{key}` in `{}` now inherits {gained}; what an inherited configuration loosens cannot be read from this diff.",
                            file.path
                        ),
                        &format!(
                            "Review the inherited configuration; record it with `allow-toolchain-weakening: {key} <reason>` if it is intended."
                        ),
                    );
                }
                for w in diff_trees(&base_tree, &head_tree, &rules) {
                    if let Some(ov) = lift(&w.key)
                        .or_else(|| w.key.rsplit('.').next().and_then(lift))
                        .or_else(|| lift(&file.path))
                    {
                        out.overrides.push(ov);
                        continue;
                    }
                    out.push(
                        ctx.overridable(settings.severity()),
                        &crate::findings::TOOLCHAIN_CONFIG_WEAKENED,
                        Some(&file.path),
                        None,
                        format!("`{}` {} in `{}`.", w.key, w.what, file.path),
                        &format!(
                            "Revert it, or justify it on its own line in the PR body or a commit message: `allow-toolchain-weakening: {} <reason>`.",
                            w.key
                        ),
                    );
                }
            }
        }
    }
    if out.examined == 0 {
        out.notes
            .push("no toolchain configuration files modified in this diff".to_string());
    }
    Ok(out)
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
}
