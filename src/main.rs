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

/// Rust ignores SIGPIPE, so a closed reader (`discipline gates | head -1`) turns every
/// later `println!` into a panic. Restore the default so the process ends the way other
/// Unix tools do: killed by the signal, which a pipeline still sees as a non-zero status.
#[cfg(unix)]
fn restore_sigpipe() {
    // SAFETY: called first in `main`, before any thread is spawned; `signal` with
    // `SIG_DFL` only resets the disposition of SIGPIPE and touches no Rust-managed memory.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn restore_sigpipe() {}

/// 0 = pass, 1 = violations, 2 = the check itself could not run. Keeping the
/// last two apart lets CI tell "the change is bad" from "the gate is broken".
fn main() -> ExitCode {
    restore_sigpipe();
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
            policy_from: discipline::cli::PolicyFrom::Head,
            comment: false,
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
        Commands::Hook(args) => hook(args),
        Commands::Explain(args) => explain(args),
        Commands::Replay(args) => {
            let summary = discipline::replay::run(&discipline::replay::Options {
                last: args.last,
                reference: args.reference,
                config: args.config,
            })?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&summary)?);
            } else {
                print!("{}", summary.render());
            }
            Ok(true)
        }
        Commands::Mcp => {
            let stdin = std::io::stdin();
            discipline::mcp::serve(
                &discipline::mcp::ChildRunner,
                stdin.lock(),
                std::io::stdout(),
            )?;
            Ok(true)
        }
        Commands::Bench(args) => discipline::guards::perf::paired_ratio::cli_bench(args),
        Commands::Doctor(args) => doctor(args),
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

fn build_overrides(
    args: &ConfigArgs,
    extra_fail_on_overrides: Option<bool>,
    extra_directive_sources: Option<Vec<String>>,
) -> Overrides {
    Overrides {
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
    }
}

/// The base ref's configuration with this run's command-line layers applied. A base
/// without the file is judged by the built-in defaults, never by the change's own copy.
fn load_base_policy(
    git: &GitCtx,
    config_path: &str,
    overrides: &Overrides,
) -> Result<DisciplineConfig> {
    let own = if discipline::gitctx::config_in_tree(config_path) {
        git.base_content(config_path)?
    } else {
        None
    };
    let base_src = match own {
        Some(s) => Some(s),
        None if config_path != "discipline.toml" => git.base_content("discipline.toml")?,
        None => None,
    };
    match base_src {
        Some(content) => {
            let label = PathBuf::from(format!("{config_path} (base ref)"));
            DisciplineConfig::resolve_source(Some((&label, content)), overrides)
        }
        None => {
            eprintln!(
                "{} --policy-from base: the base ref has no discipline.toml; judging by built-in defaults.",
                style::yellow("note:")
            );
            DisciplineConfig::resolve(None, overrides)
        }
    }
}

fn load_config(
    args: &ConfigArgs,
    repo_root: Option<&Path>,
    extra_fail_on_overrides: Option<bool>,
    extra_directive_sources: Option<Vec<String>>,
) -> Result<(DisciplineConfig, String)> {
    let overrides = build_overrides(args, extra_fail_on_overrides, extra_directive_sources);
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

/// What the `merged-pr-body` lookup produced for this run.
#[derive(Default)]
struct MergedPulls {
    bodies: Vec<discipline::tokens::MergedBody>,
    pulls: Vec<discipline::forge::MergedPull>,
    notes: Vec<String>,
}

/// Most pushed commits looked up per run; a larger push is not a merged pull request.
const MERGED_LOOKUP_CAP: usize = 20;

/// On a push event, resolve each pushed commit's merged pull request through the forge.
///
/// Read only when the run is a push event in CI, no pull-request body is in hand, and
/// `merged-pr-body` is an allowed source. A direct push (no merged pull request) is an
/// empty result. A lookup the forge refuses or cannot serve is a named note by default
/// (`directives.degrade_offline`), or an error (exit 2) when that is false; `DISCIPLINE_NO_NETWORK` and an
/// unidentifiable forge are notes: they are the operator's own choice, not a failure.
fn merged_pull_bodies(
    args: &CheckArgs,
    config: &discipline::config::DisciplineConfig,
    git: &discipline::gitctx::GitCtx,
    commits: &[(String, String)],
    has_pr_body: bool,
) -> Result<MergedPulls> {
    let mut out = MergedPulls::default();
    let source_on = config
        .directives
        .sources
        .iter()
        .any(|s| s == "merged-pr-body");
    if args.staged
        || has_pr_body
        || !source_on
        || commits.is_empty()
        || !discipline::gitctx::is_push_event_environment()
    {
        return Ok(out);
    }
    let forge = match discipline::forge::detect_for(git) {
        Ok(f) => f,
        Err(e) => {
            out.notes.push(format!(
                "merged-pr-body: not read, cannot identify the forge: {e}"
            ));
            return Ok(out);
        }
    };
    if commits.len() > MERGED_LOOKUP_CAP {
        out.notes.push(format!(
            "merged-pr-body: not read, the push carries {} commits (more than {MERGED_LOOKUP_CAP}); directives come from commit messages only",
            commits.len()
        ));
        return Ok(out);
    }
    let api = discipline::forge::HttpApi::from_env();
    for (short, _) in commits {
        // The commit list carries abbreviated ids; the forge is asked by the full one.
        let full = git.full_oid(short).map_err(|e| {
            discipline::could_not_check::tag(discipline::could_not_check::Reason::Repository, e)
        })?;
        let oid = &full;
        match discipline::forge::merged_pull_for_commit(&api, &forge, oid) {
            Ok(Some(pull)) => {
                if out.pulls.iter().any(|p| p.number == pull.number) {
                    continue;
                }
                out.notes.push(format!(
                    "merged-pr-body: commit {} arrived through merged pull request #{} (author {})",
                    &oid[..oid.len().min(10)],
                    pull.number,
                    pull.author
                ));
                out.bodies.push(discipline::tokens::MergedBody {
                    number: pull.number,
                    author: pull.author.clone(),
                    body: pull.body.clone(),
                });
                out.pulls.push(pull);
            }
            Ok(None) => out.notes.push(format!(
                "merged-pr-body: commit {} arrived through no merged pull request (direct push); its message is the only directive source",
                &oid[..oid.len().min(10)]
            )),
            // The client refuses non-loopback hosts under DISCIPLINE_NO_NETWORK: the
            // operator's choice, reported as a note, not a failed lookup.
            Err(e) if e.contains("DISCIPLINE_NO_NETWORK") => {
                out.notes.push(format!(
                    "merged-pr-body: not read, network access is disabled (DISCIPLINE_NO_NETWORK): {e}"
                ));
                return Ok(out);
            }
            Err(e) => {
                let what = format!(
                    "merged-pr-body: cannot resolve the merged pull request of commit {} on {} ({e})",
                    &oid[..oid.len().min(10)],
                    forge.kind.label()
                );
                if config.directives.degrade_offline {
                    out.notes.push(format!(
                        "{what}; continuing without it (`directives.degrade_offline`)"
                    ));
                } else {
                    return Err(discipline::could_not_check::tag(
                        discipline::could_not_check::Reason::Forge,
                        anyhow::anyhow!(
                        "{what}. The pull request's body may carry the directives this push needs; \
                         give the run a token that can read pull requests, or set \
                         `directives.degrade_offline = true` (the default) to continue with a note."
                    ),
                    ));
                }
            }
        }
    }
    Ok(out)
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

fn detect_pull_context_from_ci() -> Option<discipline::override_policy::PullContext> {
    [
        "FORGEJO_EVENT_PATH",
        "GITEA_EVENT_PATH",
        "GITHUB_EVENT_PATH",
    ]
    .iter()
    .filter_map(|var| std::env::var(var).ok())
    .filter_map(|path| std::fs::read_to_string(path).ok())
    .filter_map(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
    .find_map(|json| discipline::override_policy::pull_context(&json))
    .or_else(|| {
        // GitLab has no event payload; a merge-request pipeline sets these. The author is
        // read from the merge request itself when approvals are checked.
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let number = env("CI_MERGE_REQUEST_IID")?.parse().ok()?;
        let head_sha =
            env("CI_MERGE_REQUEST_SOURCE_BRANCH_SHA").or_else(|| env("CI_COMMIT_SHA"))?;
        Some(discipline::override_policy::PullContext {
            number,
            author: env("GITLAB_USER_LOGIN").unwrap_or_default(),
            head_sha,
        })
    })
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

/// The reports of a run that could not check (exit 2). The JSON report (stdout under
/// `--format json`, `--json-out`, a JSON `--output-file`) has no outcomes and says why in
/// `could_not_check`; JUnit, SARIF and GitLab, which have no such field, carry one
/// `engine` finding holding the error so a dashboard shows the run as failed.
fn emit_fatal_reports(args: &CheckArgs, is_gitlab: bool, base: &str, err: &anyhow::Error) {
    let empty = |outcomes, errors, could_not_check| discipline::guards::CheckSummary {
        schema_version: discipline::output_schema::REPORT_SCHEMA_VERSION,
        could_not_check,
        base: base.to_string(),
        errors,
        warnings: 0,
        notes: 0,
        overrides: 0,
        baselined: 0,
        planned_gates: discipline::config::GATES
            .iter()
            .filter(|g| !g.available)
            .map(|g| g.id)
            .collect(),
        outcomes,
        policy_failures: Vec::new(),
        deprecations: Vec::new(),
    };
    let json_summary = empty(
        Vec::new(),
        0,
        Some(discipline::could_not_check::CouldNotCheck::from_error(err)),
    );
    let json = serde_json::to_string_pretty(&json_summary).unwrap_or_default();
    if args.format == discipline::cli::OutputFormat::Json {
        println!("{json}");
    }
    // The run is already failing (exit 2); a report that cannot be written is named so a
    // missing file is not mistaken for a run that never started.
    let write = |path: &Path, content: &str| {
        if let Err(e) = std::fs::write(path, content) {
            eprintln!("discipline check: could not write {}: {e}", path.display());
        }
    };
    if let Some(path) = &args.json_out {
        write(path, &json);
    }

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
        &discipline::findings::ENGINE_COULD_NOT_RUN,
        "engine",
        1,
        format!("fatal error during check execution: {err}"),
        "inspect error details and ensure environment/git state is valid",
    );
    let err_summary = empty(vec![fatal_outcome], 1, None);
    if let Some(path) = &args.output_file {
        let content = if args.format == discipline::cli::OutputFormat::Json {
            json
        } else {
            discipline::report::format_report_content(
                &err_summary,
                args.format,
                args.fail_on_warnings,
                false,
            )
            .unwrap_or_default()
        };
        write(path, &content);
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
    let is_gitlab = is_gitlab_ci();
    let mut progress = Progress::default();
    let result = check_inner(&args, is_gitlab, &mut progress);
    if let Err(err) = &result {
        // An error after the report was written (an output file, the comment) leaves that
        // report as it is; before it, the exit-2 report is all there is.
        if !progress.reported {
            emit_fatal_reports(&args, is_gitlab, &progress.base, err);
        }
    }
    result
}

/// How far a check got, for the report of a run that stops.
#[derive(Default)]
struct Progress {
    base: String,
    reported: bool,
}

fn check_inner(args: &CheckArgs, is_gitlab: bool, progress: &mut Progress) -> Result<bool> {
    use discipline::could_not_check::{tag, Reason};
    if args.trust_workspace {
        std::env::set_var("DISCIPLINE_TRUST_WORKSPACE", "1");
    }
    let base_ref = discipline::gitctx::detect_base_ref(
        args.base.as_deref(),
        args.commit.as_deref(),
        args.commit_range.as_deref(),
    );
    let named =
        discipline::gitctx::named_head(args.commit.as_deref(), args.commit_range.as_deref())
            .map_or(Ok(()), |n| discipline::gitctx::verify_named_head(&n));
    progress.base = base_ref.clone();
    let git = named
        .and_then(|()| GitCtx::open(&base_ref, args.staged))
        .map_err(|e| tag(Reason::Repository, e))?;
    progress.base = git.base_label().to_string();
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
    let (config, config_path) = load_config(
        &args.config,
        Some(git.root()),
        extra_fail,
        extra_sources.clone(),
    )
    .map_err(|e| tag(Reason::Configuration, e))?;
    // `--policy-from base`: the change is judged by the base ref's configuration. Its own
    // copy is still loaded (it must parse) and kept for `config-integrity` to diff.
    let (config, head_config) = if args.policy_from == discipline::cli::PolicyFrom::Base {
        let overrides = build_overrides(&args.config, extra_fail, extra_sources);
        let base = load_base_policy(&git, &config_path, &overrides)
            .map_err(|e| tag(Reason::Configuration, e))?;
        (base, Some(config))
    } else {
        (config, None)
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
                    .with_context(|| format!("failed to read PR body file {}", p.display()))
                    .map_err(|e| tag(Reason::Configuration, e))?,
            ),
            None => None,
        }
    } else {
        match &args.pr_body_file {
            Some(p) => Some(
                std::fs::read_to_string(p)
                    .with_context(|| format!("failed to read PR body file {}", p.display()))
                    .map_err(|e| tag(Reason::Configuration, e))?,
            ),
            None => std::env::var("PR_BODY")
                .ok()
                .filter(|b| !b.trim().is_empty())
                .or_else(detect_pr_body_from_ci),
        }
    };

    let commits = git.commits().map_err(|e| tag(Reason::Repository, e))?;
    // `merged-pr-body`: on a push event with no pull request body, the body of the merged
    // pull request each pushed commit arrived through is the review record that approved
    // its directives. A squash or rebase merge drops it from the commit message.
    let merged = merged_pull_bodies(args, &config, &git, &commits, raw_pr_body.is_some())?;
    let (directives, mut directive_notes) = discipline::tokens::extract_directives_with_merged(
        raw_pr_body.as_deref(),
        &commits,
        &merged.bodies,
        &config,
    );
    directive_notes.extend(merged.notes.iter().cloned());

    let had_pr_body = raw_pr_body.is_some();
    let pr_body = if had_pr_body {
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
            let b = discipline::baseline::DisciplineBaseline::load_from_file(&baseline_path)
                .map_err(|e| tag(Reason::Baseline, e))?;
            (Some(baseline_filename), Some(b))
        } else if explicit_baseline.is_some() {
            return Err(tag(
                Reason::Baseline,
                anyhow::anyhow!("baseline file `{}` does not exist", baseline_path.display()),
            ));
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    let ctx = Context {
        config: &config,
        head_config: head_config.as_ref(),
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
    let mut summary = run_checks(&config, args.suite, &ctx)?;

    // A push run reads no pull-request body. A finding whose remediation points at a
    // PR-body directive would send a maintainer to edit a body this run never reads;
    // say so, and name the sources a push does read.
    if is_push_or_commit && !had_pr_body {
        let push_note = if merged.bodies.is_empty() {
            "this run is a push (or a commit range), so PR-body directives are not in scope: \
             only the pushed commits' messages are read. A squash or rebase merge drops the \
             pull request's body, and a merge commit's default message does not carry it. \
             Put the directive in a commit message, restrict the step to pull_request, or \
             keep the `merged-pr-body` directive source enabled with a forge token."
                .to_string()
        } else {
            format!(
                "this run is a push; the body of merged pull request(s) {} was read \
                 (`merged-pr-body`) and carried no directive that lifts this finding.",
                merged
                    .bodies
                    .iter()
                    .map(|m| format!("#{}", m.number))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let push_note = push_note.as_str();
        for outcome in &mut summary.outcomes {
            let mut affected = false;
            for v in &mut outcome.violations {
                let mentions_directive = v.remediation.as_deref().is_some_and(|r| {
                    r.contains("allow-") || r.contains("removes:") || r.contains("no-issue:")
                });
                if mentions_directive {
                    affected = true;
                    if let Some(r) = &mut v.remediation {
                        r.push_str(" Note: ");
                        r.push_str(push_note);
                    }
                }
            }
            if affected {
                outcome.notes.push(format!("push event: {push_note}"));
            }
        }
    }

    // Run-level override limits: a budget, and an approval read from the forge.
    let pull = detect_pull_context_from_ci().or_else(|| match merged.pulls.as_slice() {
        [one] => Some(discipline::override_policy::PullContext {
            number: one.number,
            author: one.author.clone(),
            head_sha: one.head_sha.clone(),
        }),
        _ => None,
    });
    summary.policy_failures = discipline::override_policy::judge(
        &config.directives,
        summary.directive_overrides(),
        pull.as_ref(),
        &|| discipline::forge::detect_for(&git),
        &discipline::forge::HttpApi::from_env(),
    )?;

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
    progress.reported = true;
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

    if args.comment {
        post_comment(&git, &summary, success)?;
    }

    // A change that switches its own run to advisory does not get the exit 0 it asked for.
    let config_advisory = config.meta.mode == discipline::config::RunMode::Advisory;
    let advisory_refused =
        config_advisory && discipline::guards::integrity::advisory_mode_unapproved(&ctx)?;
    if advisory_refused {
        eprintln!(
            "{}",
            discipline::style::yellow("advisory: `mode = \"advisory\"` is introduced by this change and is not honoured until it merges (or `allow-gate-weakening: meta <reason>` lifts it)")
        );
    }
    let is_advisory = args.advisory || (config_advisory && !advisory_refused);
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

/// `--comment`: one pull-request comment, edited on every run. A run with no pull
/// request posts nothing; a token that cannot write (a fork) is named, not fatal; a
/// forge that cannot be identified or reached stops the run (exit 2).
fn post_comment(
    git: &GitCtx,
    summary: &discipline::guards::CheckSummary,
    success: bool,
) -> Result<()> {
    use discipline::comment::Posted;
    let Some(pull) = detect_pull_context_from_ci() else {
        eprintln!("comment: this run has no pull request in its event; nothing posted");
        return Ok(());
    };
    let forge = discipline::forge::detect_for(git)
        .map_err(|e| anyhow::anyhow!("--comment: cannot identify the forge: {e}"))?;
    let api = discipline::forge::HttpApi::from_env();
    let body = discipline::comment::render(summary, success);
    match discipline::comment::upsert(&api, &api, &forge, pull.number, &body)
        .map_err(|e| anyhow::anyhow!("--comment: {e}"))?
    {
        Posted::Created => eprintln!("comment: posted on #{}", pull.number),
        Posted::Updated => eprintln!("comment: updated on #{}", pull.number),
        Posted::Denied(e) => eprintln!(
            "{}",
            style::yellow(&format!(
                "comment: not posted on #{}: {e}. A pull request from a fork runs with a token that cannot write; the check's status still carries the verdict",
                pull.number
            ))
        ),
    }
    Ok(())
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
# sources = ["pr-body", "commits", "merged-pr-body"]
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
    let git = if args.whole_tree || args.migrate {
        GitCtx::open_whole_tree()?
    } else {
        let base_ref = discipline::gitctx::detect_base_ref(args.base.as_deref(), None, None);
        GitCtx::open(&base_ref, false)?
    };
    let (config, config_path) = load_config(&args.config, Some(git.root()), None, None)?;
    let existing_baseline = {
        let p = git.root().join(&args.baseline_file);
        if p.exists() {
            // An unreadable baseline is not an empty one: rewriting it would drop entries.
            Some(
                discipline::baseline::DisciplineBaseline::load_from_file(&p).with_context(
                    || {
                        format!(
                            "existing baseline `{}` could not be read; fix or remove it first",
                            args.baseline_file.display()
                        )
                    },
                )?,
            )
        } else {
            None
        }
    };
    if args.migrate {
        let Some(old) = existing_baseline.as_ref() else {
            bail!(
                "no baseline at `{}` to migrate",
                args.baseline_file.display()
            );
        };
        if old.version >= discipline::baseline::FINGERPRINT_VERSION {
            println!(
                "`{}` already uses fingerprint version {}; nothing to migrate.",
                args.baseline_file.display(),
                old.version
            );
            return Ok(true);
        }
    } else if args.write
        && args.suite != discipline::cli::SuiteChoice::All
        && existing_baseline
            .as_ref()
            .is_some_and(|b| b.version < discipline::baseline::FINGERPRINT_VERSION)
    {
        bail!(
            "`{}` uses fingerprint version 1; run `discipline baseline --migrate` before writing part of it with --suite",
            args.baseline_file.display()
        );
    }

    let commits = git.commits()?;
    let (directives, directive_notes) =
        discipline::tokens::extract_directives_for_config(None, &commits, &config);

    let ctx = Context {
        config: &config,
        head_config: None,
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
    let read_head = |f: &str| git.head_content(f).ok().flatten();

    if args.migrate {
        let old = existing_baseline.expect("checked above");
        let findings: Vec<&discipline::guards::Violation> = summary
            .outcomes
            .iter()
            .filter(|o| o.enabled)
            .flat_map(|o| &o.violations)
            .collect();
        let (migrated, report) = discipline::baseline::migrate(&old, &findings, read_head);
        migrated.write_to_file(&baseline_path)?;
        println!(
            "{} rewrote {} to fingerprint version {}: {} entr{} migrated, {} stale entr{} dropped",
            style::green("ok:"),
            args.baseline_file.display(),
            migrated.version,
            report.migrated,
            if report.migrated == 1 { "y" } else { "ies" },
            report.dropped,
            if report.dropped == 1 { "y" } else { "ies" },
        );
        println!(
            "Commit it in a change of its own: `config-integrity` accepts a migration that changes nothing but the baseline."
        );
        return Ok(true);
    }

    let mut entries = Vec::new();
    let examined_gates: std::collections::HashSet<&str> =
        summary.outcomes.iter().map(|o| o.gate).collect();

    // Preserve existing findings for gates that were not examined in this run (e.g. when --suite was passed)
    if let Some(existing) = existing_baseline {
        for entry in existing.findings {
            if !examined_gates.contains(entry.gate.as_str()) {
                entries.push(entry);
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
            entries.push(discipline::baseline::entry_for(v, read_head));
        }
    }

    entries.sort();

    let baseline_obj = discipline::baseline::DisciplineBaseline {
        version: discipline::baseline::FINGERPRINT_VERSION,
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

fn doctor(args: discipline::cli::DoctorArgs) -> Result<bool> {
    use discipline::doctor::{run, DoctorInput};
    let git = GitCtx::open_whole_tree()?;
    let env = |k: &str| std::env::var(k).ok();
    // An SSH remote may name a `~/.ssh/config` alias; the forge is at its HostName.
    let origin = git.remote_url("origin").map(|o| {
        discipline::forge::resolve_ssh_alias(&o, &|a| discipline::forge::ssh_hostname_from_home(a))
    });
    let mut forge = discipline::forge::detect(&env, origin.as_deref());
    if let Some(repo) = &args.repo {
        forge = match forge {
            Ok(mut f) => {
                f.repo = repo.clone();
                Ok(f)
            }
            // An explicit repository without a detectable forge is taken to be GitHub,
            // unless DISCIPLINE_FORGE was set: an invalid value stays an error.
            Err(e) if std::env::var("DISCIPLINE_FORGE").is_ok_and(|v| !v.trim().is_empty()) => {
                Err(e)
            }
            Err(_) => Ok(discipline::forge::Forge {
                kind: discipline::forge::ForgeKind::GitHub,
                url: "https://github.com".into(),
                repo: repo.clone(),
            }),
        };
    }
    let api = discipline::forge::HttpApi { env: &env };
    let report = run(&DoctorInput {
        root: git.root(),
        forge,
        branch: args.branch.clone(),
        local_only: args.local_only,
        api: &api,
    });
    match args.format {
        discipline::cli::DoctorFormat::Text => print!("{}", report.render_text()),
        discipline::cli::DoctorFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&report)?)
        }
    }
    match report.exit_code(args.strict) {
        0 => Ok(true),
        1 => Ok(false),
        _ => bail!("some checks could not be decided (see `unknown` above)"),
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

fn explain(args: discipline::cli::ExplainArgs) -> Result<bool> {
    let Some(g) = discipline::explain::gate_for(&args.query) else {
        let near = discipline::explain::suggestions(&args.query);
        if near.is_empty() {
            bail!(
                "no gate matches `{}`; `discipline gates` lists every gate id",
                args.query
            );
        }
        bail!(
            "no gate matches `{}`; did you mean: {}",
            args.query,
            near.join(", ")
        );
    };
    let repo = discipline::gitctx::discover_repository(".").ok();
    let repo_root = repo.as_ref().and_then(|r| r.workdir());
    let (config, _) = load_config(&args.config, repo_root, None, None)?;
    let state = config
        .gates
        .settings(g.id)
        .map(|s| (s.enabled(), s.severity()));
    print!("{}", discipline::explain::render(g, state));
    Ok(true)
}

fn hook(args: discipline::cli::HookArgs) -> Result<bool> {
    use discipline::cli::HookCommand;
    use discipline::hook::Installed;
    use std::io::{IsTerminal, Read, Write};
    match args.command {
        HookCommand::Run(a) => {
            let mut stdin = String::new();
            // Only the agents that send a payload are read from: an inherited pipe that
            // is never closed must not hang Aider's lint command.
            if a.agent != discipline::hook::Agent::Aider && !std::io::stdin().is_terminal() {
                std::io::stdin()
                    .read_to_string(&mut stdin)
                    .context("cannot read the hook payload on stdin")?;
            }
            let out = discipline::hook::run_with(a.agent, a.base, &stdin, a.if_configured)?;
            print!("{}", out.stdout);
            eprint!("{}", out.stderr);
            std::io::stdout()
                .flush()
                .context("cannot write the hook response")?;
            std::process::exit(i32::from(out.code));
        }
        HookCommand::Install(a) => {
            let mut results = vec![if a.user {
                discipline::hook::install_user(a.agent)?
            } else {
                discipline::hook::install(a.agent, &discipline::hook::repo_root()?)?
            }];
            if a.cloud_agent {
                if a.agent != discipline::hook::Agent::Copilot {
                    bail!("`--cloud-agent` is for copilot: Copilot cloud agent runs the repository's hooks");
                }
                results.push(discipline::hook::install_cloud_agent(
                    &discipline::hook::repo_root()?,
                )?);
            }
            let mut ok = true;
            for installed in results {
                match installed {
                    Installed::Written(p) => {
                        println!("{} wrote {}", style::green("ok:"), p.display());
                    }
                    Installed::AlreadyPresent(p) => {
                        println!(
                            "{} {} already runs discipline for {}",
                            style::green("ok:"),
                            p.display(),
                            a.agent.id()
                        );
                    }
                    Installed::Refused(p, snippet) => {
                        println!(
                            "{} exists and was not changed. Merge this into it:\n\n{snippet}",
                            p.display()
                        );
                        ok = false;
                    }
                }
            }
            Ok(ok)
        }
    }
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
