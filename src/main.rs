use anyhow::{bail, Context as _, Result};
use clap::Parser;
use discipline::cli::{
    CheckArgs, Cli, Commands, ConfigArgs, DocsArgs, InstallHooksArgs, SuiteChoice,
};
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
            pr_title: None,
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
            bench_base_file: None,
            bench_head_file: None,
        }),
        Commands::Init(args) => init(args.name),
        Commands::Gates(args) => gates(&args.config),
        Commands::Schema => schema(),
        Commands::SelfTest => discipline::selftest::run(),
        Commands::Docs(args) => docs(args),
        Commands::InstallHooks(args) => install_hooks(args),
    }
}

fn docs(args: DocsArgs) -> Result<bool> {
    let repo = git2::Repository::discover(".").ok();
    let root = repo
        .as_ref()
        .and_then(|r| r.workdir())
        .unwrap_or_else(|| Path::new("."));
    discipline::docs::run_docs_check_or_write(root, args.write)
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
            .or_else(|_| std::env::var("DOCS_HOSTNAME_DENYLIST"))
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
        let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let res_canon = resolved_path
            .canonicalize()
            .unwrap_or_else(|_| resolved_path.clone());
        if let Ok(rel) = res_canon.strip_prefix(&root_canon) {
            rel.to_string_lossy().replace('\\', "/")
        } else if let Ok(rel) = resolved_path.strip_prefix(root) {
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

fn detect_pr_body_from_ci() -> Option<String> {
    for var in &[
        "FORGEJO_EVENT_PATH",
        "GITEA_EVENT_PATH",
        "GITHUB_EVENT_PATH",
    ] {
        if let Ok(path) = std::env::var(var) {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(body) = json
                        .get("pull_request")
                        .and_then(|pr| pr.get("body"))
                        .and_then(|b| b.as_str())
                    {
                        if !body.trim().is_empty() {
                            return Some(body.to_string());
                        }
                    }
                }
            }
        }
    }
    None
}

fn detect_pr_title_from_ci() -> Option<String> {
    for var in &[
        "FORGEJO_EVENT_PATH",
        "GITEA_EVENT_PATH",
        "GITHUB_EVENT_PATH",
    ] {
        if let Ok(path) = std::env::var(var) {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(title) = json
                        .get("pull_request")
                        .and_then(|pr| pr.get("title"))
                        .and_then(|b| b.as_str())
                    {
                        if !title.trim().is_empty() {
                            return Some(title.to_string());
                        }
                    }
                }
            }
        }
    }
    None
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

    let pr_title = if args.staged {
        args.pr_title
    } else {
        args.pr_title
            .or_else(|| std::env::var("PR_TITLE").ok())
            .filter(|t| !t.trim().is_empty())
            .or_else(detect_pr_title_from_ci)
    };

    let pr_body = if args.staged {
        match &args.pr_body_file {
            Some(p) => Some(
                std::fs::read_to_string(p)
                    .with_context(|| format!("failed to read PR body file {}", p.display()))?,
            ),
            None => None,
        }
    } else {
        match &args.pr_body_file {
            Some(p) => Some(
                std::fs::read_to_string(p)
                    .with_context(|| format!("failed to read PR body file {}", p.display()))?,
            ),
            None => std::env::var("PR_BODY")
                .ok()
                .filter(|b| !b.trim().is_empty())
                .or_else(detect_pr_body_from_ci),
        }
    };
    let commits = git.commits()?;
    let (directives, directive_notes) =
        discipline::tokens::extract_directives_for_config(pr_body.as_deref(), &commits, &config);

    let ctx = Context {
        config: &config,
        git: &git,
        config_path: &config_path,
        staged: args.staged,
        pr_title,
        pr_body,
        directives,
        directive_notes,
        bench_provenance: args.bench_provenance.clone(),
        allow_cross_host_bench: args.allow_cross_host_bench,
        bench_base_file: args.bench_base_file.clone(),
        bench_head_file: args.bench_head_file.clone(),
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

fn install_hooks(args: InstallHooksArgs) -> Result<bool> {
    let repo = git2::Repository::discover(".").context(
        "cannot install hooks: current directory is not a git repository (no .git directory found)",
    )?;
    let git_dir = repo.path();
    let hooks_dir = git_dir.join("hooks");
    if !hooks_dir.exists() {
        std::fs::create_dir_all(&hooks_dir).with_context(|| {
            format!("failed to create hooks directory: {}", hooks_dir.display())
        })?;
    }

    let pre_commit_path = hooks_dir.join("pre-commit");
    let hook_content =
        "#!/bin/sh\n# Discipline pre-commit sentinel\nexec discipline check --staged\n";

    if pre_commit_path.exists() {
        let existing = std::fs::read_to_string(&pre_commit_path).with_context(|| {
            format!(
                "failed to read existing hook: {}",
                pre_commit_path.display()
            )
        })?;
        if existing.contains("discipline check") {
            println!(
                "{} pre-commit hook already configured for discipline at {}",
                style::green("ok:"),
                pre_commit_path.display()
            );
            return Ok(true);
        }

        if args.force {
            std::fs::write(&pre_commit_path, hook_content).with_context(|| {
                format!("failed to overwrite hook: {}", pre_commit_path.display())
            })?;
            println!(
                "{} overwrote pre-commit hook with discipline sentinel at {}",
                style::green("ok:"),
                pre_commit_path.display()
            );
        } else {
            let mut updated = existing;
            if !updated.ends_with('\n') {
                updated.push('\n');
            }
            updated
                .push_str("\n# Discipline pre-commit sentinel\nexec discipline check --staged\n");
            std::fs::write(&pre_commit_path, updated)
                .with_context(|| format!("failed to update hook: {}", pre_commit_path.display()))?;
            println!(
                "{} appended discipline pre-commit sentinel to {}",
                style::green("ok:"),
                pre_commit_path.display()
            );
        }
    } else {
        std::fs::write(&pre_commit_path, hook_content)
            .with_context(|| format!("failed to write hook: {}", pre_commit_path.display()))?;
        println!(
            "{} installed discipline pre-commit hook at {}",
            style::green("ok:"),
            pre_commit_path.display()
        );
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&pre_commit_path, perms).with_context(|| {
            format!(
                "failed to set 0755 permissions on {}",
                pre_commit_path.display()
            )
        })?;
    }

    Ok(true)
}
