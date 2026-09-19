use anyhow::{bail, Context as _, Result};
use clap::Parser;
use discipline::cli::{CheckArgs, Cli, Commands, ConfigArgs, SuiteChoice};
use discipline::config::{split_list, DisciplineConfig, Overrides, GATES, HOSTNAME_DENYLIST_ENV};
use discipline::gitctx::GitCtx;
use discipline::guards::{run_checks, Context};
use discipline::report::render_report;
use discipline::style;
use std::path::Path;
use std::process::ExitCode;

/// 0 = pass, 1 = violations, 2 = the check itself could not run. Keeping the
/// last two apart lets CI tell "the change is bad" from "the gate is broken".
fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("{} {e:#}", style::red("discipline: could not check:"));
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<bool> {
    match Cli::parse().command {
        Commands::Check(args) => check(args),
        Commands::Diff(args) => check(CheckArgs {
            config: args.config,
            suite: SuiteChoice::AgentGuard,
            base: args.base.or_else(|| Some("HEAD~1".to_string())),
            staged: false,
            pr_body_file: None,
            fail_on_warnings: false,
            fail_on_overrides: false,
            directive_sources: Vec::new(),
            format: args.format,
            json_out: args.json_out,
            output_file: args.output_file,
            report_gitlab: args.report_gitlab,
            report_junit: args.report_junit,
            report_sarif: args.report_sarif,
            bench_provenance: None,
            allow_cross_host_bench: false,
        }),
        Commands::Init(args) => init(args.name),
        Commands::Gates(args) => gates(&args.config),
        Commands::Schema => schema(),
        Commands::SelfTest => discipline::selftest::run(),
    }
}

fn load_config(
    args: &ConfigArgs,
    repo_root: Option<&Path>,
    extra_fail_on_overrides: Option<bool>,
    extra_directive_sources: Option<Vec<String>>,
) -> Result<(DisciplineConfig, String)> {
    let overrides = Overrides {
        config_override: args
            .config_override
            .clone()
            .filter(|s| !s.trim().is_empty()),
        enable: args.enable.iter().flat_map(|s| split_list(s)).collect(),
        disable: args.disable.iter().flat_map(|s| split_list(s)).collect(),
        hostname_denylist: std::env::var(HOSTNAME_DENYLIST_ENV)
            .map(|v| split_list(&v))
            .unwrap_or_default(),
        directive_sources: extra_directive_sources,
        fail_on_overrides: extra_fail_on_overrides,
    };
    let explicit = args.config != Path::new("discipline.toml");
    let resolved_path = match repo_root {
        Some(root) if !args.config.is_absolute() => root.join(&args.config),
        _ => args.config.clone(),
    };

    let config_path_for_ctx = if let Some(root) = repo_root {
        if let Ok(rel) = resolved_path.strip_prefix(root) {
            rel.to_string_lossy().replace('\\', "/")
        } else {
            args.config.to_string_lossy().replace('\\', "/")
        }
    } else {
        args.config.to_string_lossy().replace('\\', "/")
    };
    let config_path_for_ctx = config_path_for_ctx.trim_start_matches("./").to_string();

    if resolved_path.exists() {
        let bytes = std::fs::read(&resolved_path).with_context(|| {
            format!(
                "failed to read configuration file {}",
                resolved_path.display()
            )
        })?;
        if bytes.contains(&0) {
            bail!(
                "configuration file {} contains a NUL byte",
                resolved_path.display()
            );
        }
        let config = DisciplineConfig::resolve(Some(&resolved_path), &overrides)?;
        Ok((config, config_path_for_ctx))
    } else if explicit {
        bail!(
            "configuration file {} does not exist",
            args.config.display()
        );
    } else {
        eprintln!(
            "{} no discipline.toml; using built-in defaults (every available gate on).",
            style::yellow("note:")
        );
        let config = DisciplineConfig::resolve(None, &overrides)?;
        Ok((config, config_path_for_ctx))
    }
}

fn is_gitlab_ci() -> bool {
    std::env::var("GITLAB_CI")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false)
}

fn check(args: CheckArgs) -> Result<bool> {
    let is_gitlab = is_gitlab_ci();
    let base_ref = discipline::gitctx::detect_base_ref(args.base.as_deref());
    let git = GitCtx::open(&base_ref, args.staged)?;
    let extra_fail = if args.fail_on_overrides {
        Some(true)
    } else {
        None
    };
    let non_empty_sources: Vec<String> = args
        .directive_sources
        .iter()
        .flat_map(|s| split_list(s))
        .collect();
    let extra_sources = if non_empty_sources.is_empty() {
        None
    } else {
        Some(non_empty_sources)
    };
    let (config, config_path) =
        load_config(&args.config, Some(git.root()), extra_fail, extra_sources)?;

    let pr_body = match &args.pr_body_file {
        Some(p) => Some(
            std::fs::read_to_string(p)
                .with_context(|| format!("failed to read PR body file {}", p.display()))?,
        ),
        None => std::env::var("PR_BODY")
            .ok()
            .filter(|b| !b.trim().is_empty()),
    };
    let commits = git.commits()?;
    let (directives, directive_notes) =
        discipline::tokens::extract_directives(pr_body.as_deref(), &commits, &config.directives);

    let ctx = Context {
        config: &config,
        git: &git,
        config_path: &config_path,
        staged: args.staged,
        pr_body,
        directives,
        directive_notes,
        bench_provenance: args.bench_provenance.clone(),
        allow_cross_host_bench: args.allow_cross_host_bench,
    };
    let summary = run_checks(&config, args.suite, &ctx)?;
    let fail_on_overrides = config.directives.fail_on_overrides;
    render_report(
        &summary,
        args.format,
        args.fail_on_warnings,
        fail_on_overrides,
    )?;
    if let Some(path) = &args.output_file {
        let content = discipline::report::format_report_content(
            &summary,
            args.format,
            args.fail_on_warnings,
            fail_on_overrides,
        )?;
        std::fs::write(path, content)
            .with_context(|| format!("failed to write output file {}", path.display()))?;
    }
    if let Some(path) = &args.json_out {
        std::fs::write(path, serde_json::to_string_pretty(&summary)?)
            .with_context(|| format!("failed to write JSON report {}", path.display()))?;
    }

    // Auto-bundle or explicit report outputs for GitLab CI / multi-CI
    let report_gitlab = args.report_gitlab.or_else(|| {
        if is_gitlab {
            Some(std::path::PathBuf::from("gl-codequality.json"))
        } else {
            None
        }
    });
    let report_junit = args.report_junit.or_else(|| {
        if is_gitlab {
            Some(std::path::PathBuf::from("junit.xml"))
        } else {
            None
        }
    });
    let report_sarif = args.report_sarif;

    if let Some(path) = &report_gitlab {
        let content = discipline::report::gitlab::format_gitlab(&summary);
        std::fs::write(path, content).with_context(|| {
            format!(
                "failed to write GitLab Code Quality report {}",
                path.display()
            )
        })?;
    }
    if let Some(path) = &report_junit {
        let content = discipline::report::junit::format_junit(&summary, args.fail_on_warnings);
        std::fs::write(path, content)
            .with_context(|| format!("failed to write JUnit report {}", path.display()))?;
    }
    if let Some(path) = &report_sarif {
        let sarif_val = discipline::report::sarif::format_sarif(&summary);
        let content = serde_json::to_string_pretty(&sarif_val)?;
        std::fs::write(path, content)
            .with_context(|| format!("failed to write SARIF report {}", path.display()))?;
    }

    Ok(summary.is_success(args.fail_on_warnings, fail_on_overrides))
}

fn init(name: Option<String>) -> Result<bool> {
    let repo = git2::Repository::discover(".").ok();
    let repo_root = repo.as_ref().and_then(|r| r.workdir());
    let config_path = repo_root
        .map(|r| r.join("discipline.toml"))
        .unwrap_or_else(|| std::path::PathBuf::from("discipline.toml"));
    if config_path.exists() {
        bail!("discipline.toml already exists");
    }
    let project_name = name.unwrap_or_else(|| {
        std::env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| "my-project".to_string())
    });
    let starter = format!(
        r#"# discipline.toml — configuration for Discipline CI gatekeeper.
#
# Schema version 1. By default, every available gate is enabled at severity = "error".
# You only need to specify settings that differ from the defaults.
# Run `discipline gates` to view the effective status of all gates.

[meta]
version = 1
name = "{project_name}"
# description = "Brief description of the project"

# [directives]
# sources = ["pr-body", "commits"]
# allow_hidden = false
# fail_on_overrides = false

# Gate customizations (examples):
# [gates.assertion-reduction]
# severity = "error"
# exempt_paths = ["tests/legacy/**"]

# [gates.pii]
# allowed_users = ["runner", "user", "username"]
# hostname_denylist = ["internal.corp"]

# [gates.time-estimates]
# allow_patterns = ['^timeout: \d+']
"#
    );
    std::fs::write(&config_path, starter)?;
    println!(
        "{} wrote minimal discipline.toml for `{project_name}` with every available gate on.",
        style::green("ok:")
    );
    Ok(true)
}

fn schema() -> Result<bool> {
    let s = discipline::schema::generate_schema();
    println!("{}", serde_json::to_string_pretty(&s)?);
    Ok(true)
}

fn gates(args: &ConfigArgs) -> Result<bool> {
    let repo = git2::Repository::discover(".").ok();
    let repo_root = repo.as_ref().and_then(|r| r.workdir());
    let (config, _) = load_config(args, repo_root, None, None)?;
    println!("{:<24} {:<13} {:<9} SUMMARY", "GATE", "SUITE", "STATE");
    for g in GATES {
        let state = match config.gates.settings(g.id) {
            // Pad before styling: escape codes would count toward the width.
            _ if !g.available => style::dim(&format!("{:<9}", "planned")),
            Some(s) if s.enabled() => style::green(&format!("{:<9}", "on")),
            _ => style::yellow(&format!("{:<9}", "off")),
        };
        println!(
            "{:<24} {:<13} {} {}",
            g.id,
            g.suite.label(),
            state,
            g.summary
        );
    }
    Ok(true)
}
