use anyhow::{bail, Context as _, Result};
use clap::Parser;
use discipline::cli::{
    BaselineArgs, CheckArgs, Cli, Commands, ConfigArgs, DocsArgs, InstallHooksArgs,
};
use discipline::config::{split_list, DisciplineConfig, Overrides, GATES, HOSTNAME_DENYLIST_ENV};
use discipline::gitctx::GitCtx;
use discipline::guards::{run_checks, Context};
use discipline::report::render_report;
use discipline::style;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// 0 = pass, 1 = violations, 2 = the check itself could not run. Keeping the
/// last two apart lets CI tell "the change is bad" from "the gate is broken".
fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            let _ = e.print();
            return ExitCode::from(e.exit_code() as u8);
        }
    };
    let cmd_name = cli.command.name();
    match run_command(cli.command) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!(
                "{}: {e:#}",
                style::red(&format!("discipline {cmd_name}: error"))
            );
            ExitCode::from(2)
        }
    }
}

fn run_command(command: Commands) -> Result<bool> {
    match command {
        Commands::Check(args) => check(args),
        Commands::Diff(args) => check(CheckArgs {
            config: args.config,
            suite: args.suite,
            base: args.base.or_else(|| Some("HEAD".to_string())),
            commit: None,
            commit_range: None,
            staged: false,
            pr_body_file: None,
            pr_title: None,
            fail_on_warnings: false,
            fail_on_overrides: false,
            actor: None,
            directive_sources: Vec::new(),
            quiet: false,
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
            baseline_file: args.baseline_file,
            no_baseline: args.no_baseline,
            trust_workspace: args.trust_workspace,
            advisory: args.advisory,
        }),
        Commands::Baseline(args) => baseline(args),
        Commands::Init(args) => init(args.name),
        Commands::Gates(args) => gates(&args.config),
        Commands::Schema => schema(),
        Commands::SelfTest => discipline::selftest::run(),
        Commands::Completions(args) => {
            clap_complete::generate(
                args.shell,
                &mut <discipline::cli::Cli as clap::CommandFactory>::command(),
                "discipline",
                &mut std::io::stdout(),
            );
            Ok(true)
        }
        Commands::Docs(args) => docs(args),
        Commands::InstallHooks(args) => install_hooks(args),
        Commands::Bench(args) => discipline::guards::perf::paired_ratio::cli_bench(args),
    }
}

fn docs(args: DocsArgs) -> Result<bool> {
    let repo = discipline::gitctx::discover_repository(".").ok();
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
            "{} no discipline.toml; using built-in defaults (`discipline gates` shows which gates are on).",
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

fn write_structured_reports(
    summary: &discipline::guards::CheckSummary,
    report_gitlab: Option<&Path>,
    report_junit: Option<&Path>,
    report_sarif: Option<&Path>,
    fail_on_warnings: bool,
) -> Result<()> {
    if let Some(path) = report_gitlab {
        let content = discipline::report::gitlab::format_gitlab(summary);
        std::fs::write(path, content).with_context(|| {
            format!(
                "failed to write GitLab Code Quality report {}",
                path.display()
            )
        })?;
    }
    if let Some(path) = report_junit {
        let content = discipline::report::junit::format_junit(summary, fail_on_warnings);
        std::fs::write(path, content)
            .with_context(|| format!("failed to write JUnit report {}", path.display()))?;
    }
    if let Some(path) = report_sarif {
        let sarif_val = discipline::report::sarif::format_sarif(summary);
        let content = serde_json::to_string_pretty(&sarif_val)?;
        std::fs::write(path, content)
            .with_context(|| format!("failed to write SARIF report {}", path.display()))?;
    }
    Ok(())
}

fn emit_fatal_reports(args: &CheckArgs, is_gitlab: bool, base: &str, err: &anyhow::Error) {
    let mut fatal_outcome = discipline::guards::GateOutcome {
        gate: "engine",
        suite: "engine",
        enabled: true,
        examined: 0,
        inline_exemptions: 0,
        baselined: 0,
        notes: Vec::new(),
        violations: Vec::new(),
        overrides: Vec::new(),
    };
    fatal_outcome.add_violation(
        discipline::guards::Severity::Error,
        "engine",
        1,
        format!("fatal error during check execution: {err}"),
        "inspect error details and ensure environment/git state is valid",
    );
    let err_summary = discipline::guards::CheckSummary {
        base: base.to_string(),
        errors: 1,
        warnings: 0,
        notes: 0,
        overrides: 0,
        baselined: 0,
        planned_gates: discipline::config::GATES
            .iter()
            .filter(|g| !g.available)
            .map(|g| g.id)
            .collect(),
        outcomes: vec![fatal_outcome],
    };
    if let Some(path) = &args.json_out {
        let _ = std::fs::write(
            path,
            serde_json::to_string_pretty(&err_summary).unwrap_or_default(),
        );
    }
    if let Some(path) = &args.output_file {
        let _ = std::fs::write(
            path,
            discipline::report::format_report_content(
                &err_summary,
                args.format,
                args.fail_on_warnings,
                false,
            )
            .unwrap_or_default(),
        );
    }
    let report_gitlab = args.report_gitlab.as_deref().or_else(|| {
        if is_gitlab {
            Some(Path::new("gl-codequality.json"))
        } else {
            None
        }
    });
    let report_junit = args.report_junit.as_deref().or_else(|| {
        if is_gitlab {
            Some(Path::new("junit.xml"))
        } else {
            None
        }
    });
    let report_sarif = args.report_sarif.as_deref();
    let _ = write_structured_reports(
        &err_summary,
        report_gitlab,
        report_junit,
        report_sarif,
        args.fail_on_warnings,
    );
}

fn check(args: CheckArgs) -> Result<bool> {
    if args.trust_workspace {
        std::env::set_var("DISCIPLINE_TRUST_WORKSPACE", "1");
    }
    let is_gitlab = is_gitlab_ci();
    let base_ref = discipline::gitctx::detect_base_ref(
        args.base.as_deref(),
        args.commit.as_deref(),
        args.commit_range.as_deref(),
    );
    let git = match GitCtx::open(&base_ref, args.staged) {
        Ok(g) => g,
        Err(err) => {
            emit_fatal_reports(&args, is_gitlab, &base_ref, &err);
            return Err(err);
        }
    };
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
        match load_config(&args.config, Some(git.root()), extra_fail, extra_sources) {
            Ok(c) => c,
            Err(err) => {
                emit_fatal_reports(&args, is_gitlab, git.base_label(), &err);
                return Err(err);
            }
        };

    let is_push_or_commit = discipline::gitctx::is_push_event_environment()
        || args.commit.is_some()
        || args.commit_range.is_some();

    let pr_title = if args.staged {
        args.pr_title.clone()
    } else {
        let explicit = args
            .pr_title
            .clone()
            .or_else(|| std::env::var("PR_TITLE").ok())
            .filter(|t| !t.trim().is_empty())
            .or_else(detect_pr_title_from_ci);
        if explicit.is_some() {
            explicit
        } else if is_push_or_commit {
            git.head_commit_subject()
                .ok()
                .filter(|s| !s.trim().is_empty())
        } else {
            None
        }
    };

    let raw_pr_body = if args.staged {
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
    let (directives, directive_notes) = discipline::tokens::extract_directives_for_config(
        raw_pr_body.as_deref(),
        &commits,
        &config,
    );

    let pr_body = if raw_pr_body.is_some() {
        raw_pr_body
    } else if is_push_or_commit {
        git.head_commit_body().ok().filter(|b| !b.trim().is_empty())
    } else {
        None
    };

    let explicit_baseline = args.baseline_file.clone().or_else(|| {
        std::env::var("DISCIPLINE_BASELINE")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
    });
    let no_baseline = args.no_baseline
        || std::env::var("DISCIPLINE_NO_BASELINE")
            .ok()
            .map(|s| s == "1" || s.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

    let baseline_filename = explicit_baseline
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| discipline::baseline::DEFAULT_BASELINE_FILE.to_string());

    let (baseline_path_ref, loaded_baseline) = if !no_baseline {
        let baseline_path = git.root().join(&baseline_filename);
        if baseline_path.exists() {
            let b = match discipline::baseline::DisciplineBaseline::load_from_file(&baseline_path) {
                Ok(b) => b,
                Err(err) => {
                    emit_fatal_reports(&args, is_gitlab, git.base_label(), &err);
                    return Err(err);
                }
            };
            (Some(baseline_filename), Some(b))
        } else if explicit_baseline.is_some() {
            let err = anyhow::anyhow!("baseline file `{}` does not exist", baseline_path.display());
            emit_fatal_reports(&args, is_gitlab, git.base_label(), &err);
            return Err(err);
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    let ctx = Context {
        config: &config,
        git: &git,
        config_path: &config_path,
        baseline_path: baseline_path_ref.as_deref(),
        baseline: loaded_baseline.as_ref(),
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
    let summary = match run_checks(&config, args.suite, &ctx) {
        Ok(s) => s,
        Err(err) => {
            emit_fatal_reports(&args, is_gitlab, git.base_label(), &err);
            return Err(err);
        }
    };

    let raw_fail_on_overrides = config.directives.fail_on_overrides;
    let actor = args
        .actor
        .clone()
        .or_else(|| std::env::var("DISCIPLINE_ACTOR").ok())
        .or_else(|| std::env::var("GITHUB_ACTOR").ok())
        .or_else(|| std::env::var("GITEA_ACTOR").ok())
        .or_else(|| std::env::var("FORGEJO_ACTOR").ok())
        .or_else(|| std::env::var("GITLAB_USER_LOGIN").ok())
        .filter(|s| !s.trim().is_empty());

    let actor_authorized = match &actor {
        Some(act) => config
            .directives
            .allowed_override_actors
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(act)),
        None => false,
    };

    let fail_on_overrides = if raw_fail_on_overrides && actor_authorized {
        false
    } else {
        raw_fail_on_overrides
    };

    let success = summary.is_success(args.fail_on_warnings, fail_on_overrides);
    if !(args.quiet && success) {
        render_report(
            &summary,
            args.format,
            args.fail_on_warnings,
            fail_on_overrides,
        )?;
    }
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
    let report_gitlab = args.report_gitlab.as_deref().or_else(|| {
        if is_gitlab {
            Some(Path::new("gl-codequality.json"))
        } else {
            None
        }
    });
    let report_junit = args.report_junit.as_deref().or_else(|| {
        if is_gitlab {
            Some(Path::new("junit.xml"))
        } else {
            None
        }
    });
    let report_sarif = args.report_sarif.as_deref();

    write_structured_reports(
        &summary,
        report_gitlab,
        report_junit,
        report_sarif,
        args.fail_on_warnings,
    )?;

    let is_advisory = args.advisory || config.meta.mode == discipline::config::RunMode::Advisory;
    if is_advisory && !success {
        eprintln!(
            "{}",
            discipline::style::yellow("advisory: violations detected, but exiting 0 due to advisory mode (--advisory / mode = \"advisory\")")
        );
        Ok(true)
    } else {
        Ok(success)
    }
}

fn init(name: Option<String>) -> Result<bool> {
    let repo = discipline::gitctx::discover_repository(".").ok();
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
# Schema version 1. Gates run at their built-in default enablement and severity.
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
        "{} wrote minimal discipline.toml for `{project_name}` using built-in gate defaults.",
        style::green("ok:")
    );
    Ok(true)
}

fn schema() -> Result<bool> {
    let s = discipline::schema::generate_schema();
    println!("{}", serde_json::to_string_pretty(&s)?);
    Ok(true)
}

fn baseline(mut args: BaselineArgs) -> Result<bool> {
    if args.baseline_file == std::path::Path::new(discipline::baseline::DEFAULT_BASELINE_FILE) {
        if let Some(p) = std::env::var("DISCIPLINE_BASELINE")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
        {
            args.baseline_file = p;
        }
    }
    if args.trust_workspace {
        std::env::set_var("DISCIPLINE_TRUST_WORKSPACE", "1");
    }
    let git = if args.whole_tree {
        GitCtx::open_whole_tree()?
    } else {
        let base_ref = discipline::gitctx::detect_base_ref(args.base.as_deref(), None, None);
        GitCtx::open(&base_ref, false)?
    };
    let (config, config_path) = load_config(&args.config, Some(git.root()), None, None)?;

    let commits = git.commits()?;
    let (directives, directive_notes) =
        discipline::tokens::extract_directives_for_config(None, &commits, &config);

    let ctx = Context {
        config: &config,
        git: &git,
        config_path: &config_path,
        baseline_path: None,
        baseline: None,
        staged: false,
        pr_title: None,
        pr_body: None,
        directives,
        directive_notes,
        bench_provenance: None,
        allow_cross_host_bench: false,
        bench_base_file: None,
        bench_head_file: None,
    };

    let summary = run_checks(&config, args.suite, &ctx)?;

    let baseline_path = git.root().join(&args.baseline_file);
    let mut entries = Vec::new();

    let examined_gates: std::collections::HashSet<&str> =
        summary.outcomes.iter().map(|o| o.gate).collect();

    // Preserve existing findings for gates that were not examined in this run (e.g. when --suite was passed)
    if baseline_path.exists() {
        if let Ok(existing) =
            discipline::baseline::DisciplineBaseline::load_from_file(&baseline_path)
        {
            for entry in existing.findings {
                if !examined_gates.contains(entry.gate.as_str()) {
                    entries.push(entry);
                }
            }
        }
    }

    let policy = discipline::baseline::RecordPolicy {
        fail_on_warnings: args.fail_on_warnings,
        all_severities: args.all_severities,
    };
    let mut recorded = discipline::baseline::SeverityTally::default();
    let mut skipped = discipline::baseline::SeverityTally::default();

    for o in &summary.outcomes {
        if !o.enabled {
            continue;
        }
        for v in &o.violations {
            if !policy.records(v.severity) {
                skipped.add(v.severity, v.gate);
                continue;
            }
            recorded.add(v.severity, v.gate);
            let fp = discipline::baseline::compute_violation_fingerprint_with_content(v, |f| {
                git.head_content(f).ok().flatten()
            });
            entries.push(discipline::baseline::BaselineEntry {
                gate: v.gate.to_string(),
                rule: v.title.clone(),
                path: v.file.clone().unwrap_or_default(),
                fingerprint: fp,
            });
        }
    }

    entries.sort();

    let baseline_obj = discipline::baseline::DisciplineBaseline {
        version: 1,
        findings: entries,
    };

    // Nothing disappears silently: say what was left out and how to include it.
    let breakdown = {
        let mut b = format!(
            "  recorded: {}\n  skipped: {}",
            recorded.summary(),
            skipped.summary_by_gate()
        );
        if skipped.total() > 0 {
            b.push_str(if policy.fail_on_warnings {
                "\n  (notes never block; pass --all-severities to record them anyway)"
            } else {
                "\n  (non-blocking under the current configuration; pass --all-severities to record them, \
                 or --fail-on-warnings if `check` runs with it)"
            });
        }
        b
    };

    if args.write {
        baseline_obj.write_to_file(&baseline_path)?;
        println!(
            "{} recorded {} grandfathered finding{} to {}",
            style::green("ok:"),
            baseline_obj.findings.len(),
            if baseline_obj.findings.len() == 1 {
                ""
            } else {
                "s"
            },
            args.baseline_file.display()
        );
        println!("{breakdown}");
        if !baseline_obj.findings.is_empty() {
            println!(
                "\nTo commit this baseline under `config-integrity`, include this directive on its own line in the commit message or PR body:\n  allow-gate-weakening: baseline initial grandfathered baseline"
            );
        }
        Ok(true)
    } else {
        println!(
            "Found {} finding{} eligible for grandfathering.",
            baseline_obj.findings.len(),
            if baseline_obj.findings.len() == 1 {
                ""
            } else {
                "s"
            }
        );
        println!("{breakdown}");
        println!(
            "Run `discipline baseline --write` to record them to {}.",
            args.baseline_file.display()
        );
        Ok(true)
    }
}

fn gates(args: &ConfigArgs) -> Result<bool> {
    let repo = discipline::gitctx::discover_repository(".").ok();
    let repo_root = repo.as_ref().and_then(|r| r.workdir());
    let (config, _) = load_config(args, repo_root, None, None)?;
    println!(
        "{:<24} {:<13} {:<9} {:<42} SUMMARY",
        "GATE", "SUITE", "STATE", "SEVERITY"
    );
    for g in GATES {
        let finding_overrides = match g.id {
            "shell-secrets" => " (tokens: error, heuristics: warn)",
            "ignored-tests" => " (conditional skips: note)",
            _ => "",
        };
        let (state, severity) = match config.gates.settings(g.id) {
            _ if !g.available => (
                style::dim(&format!("{:<9}", "planned")),
                format!("{:<42}", "-"),
            ),
            Some(s) if s.enabled() => {
                let sev = s.severity().to_string();
                let full = format!("{sev}{finding_overrides}");
                (
                    style::green(&format!("{:<9}", "on")),
                    format!("{:<42}", full),
                )
            }
            Some(s) => {
                let sev = s.severity().to_string();
                let full = format!("{sev}{finding_overrides}");
                (
                    style::yellow(&format!("{:<9}", "off")),
                    format!("{:<42}", full),
                )
            }
            _ => (
                style::yellow(&format!("{:<9}", "off")),
                format!("{:<42}", "-"),
            ),
        };
        println!(
            "{:<24} {:<13} {} {} {}",
            g.id,
            g.suite.label(),
            state,
            severity,
            g.summary
        );
    }
    Ok(true)
}

fn install_hooks(args: InstallHooksArgs) -> Result<bool> {
    let repo = discipline::gitctx::discover_repository(".").context(
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
    let hook_content = "#!/bin/sh\n# Discipline pre-commit sentinel\ndiscipline check --staged\n";

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
            let first_line = existing.lines().next().unwrap_or("");
            if first_line.starts_with("#!")
                && !first_line.contains("sh")
                && !first_line.contains("bash")
                && !first_line.contains("zsh")
            {
                bail!(
                    "existing hook at {} uses a non-shell interpreter ('{}'). Cannot safely append shell commands. Integrate 'discipline check --staged' manually or use --force to overwrite.",
                    pre_commit_path.display(),
                    first_line.trim()
                );
            }
            let mut updated = existing;
            if !updated.ends_with('\n') {
                updated.push('\n');
            }
            updated.push_str("\n# Discipline pre-commit sentinel\ndiscipline check --staged\n");
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
