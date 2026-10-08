//! The `toolchain-config` tables: which key of which file moves the bar in which direction,
//! the configurations written as code, and the keys through which a file inherits another.

use crate::guards::config_delta::{Judge, Rule};

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
pub(super) const EXECUTABLE: &[&str] = &[
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

/// Keys through which a configuration inherits another one, per file: a gained or swapped
/// entry pulls in settings this diff cannot read.
pub(super) const INHERITANCE: &[(&[&str], &str)] = &[
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
