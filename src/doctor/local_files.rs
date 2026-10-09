//! Findings read from single repository files: the security policy, the test-report
//! setting of `test-floor` and the mutation testing preset.

use super::{Finding, Status};
use std::path::Path;

/// Where a forge looks for a repository's security policy (GitHub: the root, `.github/` or
/// `docs/`).
pub const SECURITY_POLICY_PATHS: &[&str] =
    &["SECURITY.md", ".github/SECURITY.md", "docs/SECURITY.md"];

/// `security-policy`: whether the repository says how to report a vulnerability privately
/// (OpenSSF Scorecard Security-Policy). Information when absent.
pub fn security_policy_finding(path: Option<&str>) -> Finding {
    match path {
        Some(p) => Finding::new("security-policy", Status::Pass, format!("{p} is present")),
        None => Finding::new(
            "security-policy",
            Status::Info,
            "no SECURITY.md: a reporter is not told how to report a vulnerability privately",
        )
        .fix("Add SECURITY.md with a private reporting channel (GitHub: the repository's security advisories)."),
    }
}

/// `test-report`: whether test-floor runtime identity ratcheting is configured.
pub fn test_report_finding(
    root: &Path,
    repo_config: Option<&crate::config::DisciplineConfig>,
) -> Option<Finding> {
    let configured_report = repo_config.and_then(|cfg| {
        cfg.gates
            .test_floor
            .test_report
            .as_deref()
            .or(cfg.gates.test_floor.head_report.as_deref())
    });
    let base_report = repo_config.and_then(|cfg| cfg.gates.test_floor.base_report.as_deref());

    if let Some(rep) = configured_report {
        let rep_normalized = rep.replace('\\', "/");
        let norm = rep_normalized.strip_prefix("./").unwrap_or(&rep_normalized);
        let is_build_output = norm.starts_with("target/")
            || norm.starts_with("build/")
            || norm.starts_with("out/")
            || norm.starts_with("dist/");
        if is_build_output && base_report.is_none() {
            Some(
                Finding::new(
                    "test-report",
                    Status::Warn,
                    format!(
                        "test_report is configured to a build-output path (`{rep}`) without `base_report`: git cannot retrieve base ref reports from build output directories, so identity ratcheting cannot engage"
                    ),
                )
                .fix("Configure `base_report` in [gates.test-floor] to point to a base artifact, or write test reports to a tracked path (or pass --test-base-report / DISCIPLINE_TEST_BASE_REPORT in CI)."),
            )
        } else {
            Some(Finding::new(
                "test-report",
                Status::Pass,
                format!("test-floor runtime identity ratcheting is configured (`{rep}`)"),
            ))
        }
    } else {
        crate::init::TestRunner::detect(root).map(|runner| {
            Finding::new(
                "test-report",
                Status::Info,
                format!(
                    "recognized test runner ({}); test-floor has no test_report configured: runtime-only test erosion (parametrized cases, uncollected files, inactive #[cfg] tests, macro-generated tests) goes unchecked by static counts",
                    runner.name()
                ),
            )
            .fix(format!(
                "Configure test_command and test_report in [gates.test-floor] (e.g. `{}`) or pass test_report in CI to enable identity ratcheting. In a repository whose base ref already has a discipline.toml, the change that adds test_report or test_command is reported by config-integrity as a new counting basis: put `allow-gate-weakening: test-floor <reason>` in its PR body or a commit message.",
                runner.test_report()
            ))
        })
    }
}

/// `mutation-testing`: whether a mutation testing preset is configured in `command`.
pub fn mutation_preset_finding(
    root: &Path,
    repo_config: Option<&crate::config::DisciplineConfig>,
) -> Option<Finding> {
    let configured_preset = repo_config.and_then(|cfg| {
        if !cfg.gates.command.enabled {
            return None;
        }
        if let Some(ref p) = cfg.gates.command.preset {
            if crate::guards::presets::resolve_preset(p).is_some_and(|d| d.category == "mutation") {
                return Some(p.clone());
            }
        }
        for cmd in &cfg.gates.command.commands {
            if let Some(ref p) = cmd.preset {
                if crate::guards::presets::resolve_preset(p)
                    .is_some_and(|d| d.category == "mutation")
                {
                    return Some(p.clone());
                }
            }
        }
        None
    });

    if let Some(preset) = configured_preset {
        Some(Finding::new(
            "mutation-testing",
            Status::Pass,
            format!("mutation testing is configured (`{preset}`)"),
        ))
    } else {
        crate::init::TestRunner::detect(root).and_then(|runner| {
            runner.mutation_preset().map(|preset| {
                Finding::new(
                    "mutation-testing",
                    Status::Info,
                    format!(
                        "recognized test runner ({}); no mutation preset configured in [gates.command]: special-cased test inputs (hard-coding return values for tested arguments) go undetected by static diff gates",
                        runner.name()
                    ),
                )
                .fix(format!(
                    "Configure a mutation preset in [gates.command] (e.g. `preset = \"{preset}\"`{}) to verify test discrimination{}.",
                    runner
                        .mutation_command()
                        .map(|c| format!(" with `command = \"{c}\"`, since the preset's own command is Maven's"))
                        .unwrap_or_default(),
                    if runner.mutation_preset_is_diff_scoped() {
                        " against modified code"
                    } else {
                        "; this preset's command runs over the whole project, not only the diff"
                    }
                ))
            })
        })
    }
}
