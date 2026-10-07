use anyhow::{bail, Context as _, Result};
use clap::Parser;
use discipline::cli::{
    build_overrides, load_config, CheckArgs, Cli, Commands, ConfigArgs, DocsArgs, InstallHooksArgs,
};
use discipline::config::{split_list, DisciplineConfig, Overrides, GATES};
use discipline::gitctx::GitCtx;
use discipline::guards::{run_checks, Context};
use discipline::report::render_report;
use discipline::style;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Rust ignores SIGPIPE, so a closed reader (`discipline gates | head -1`) turns every
/// later `println!` into a panic. Restore the default so the process ends the way other
/// Unix tools do: killed by the signal, which a pipeline still sees as a non-zero status.
///
/// The work runs on a second thread, and a signal raised by a write may be handed to
/// any thread that does not block it. Handed to this one while it waits for the worker,
/// it would be acted on only after the worker's write had returned an error and its
/// `println!` had panicked. So this thread blocks the signal, and the worker, which
/// inherits that, unblocks it ([`take_sigpipe`]): the worker is then the only thread the
/// signal can be delivered to, and it ends the process inside the write.
#[cfg(unix)]
fn restore_sigpipe() {
    // SAFETY: called first in `main`, before any thread is spawned. `signal` with
    // `SIG_DFL` only resets the disposition of SIGPIPE. `sigemptyset` and `sigaddset`
    // initialise the zeroed set they are given, and `pthread_sigmask` reads that set
    // and changes this thread's mask only; none touches Rust-managed memory.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGPIPE);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    }
}

/// Let the calling thread receive SIGPIPE: the worker's first step (`restore_sigpipe`).
#[cfg(unix)]
fn take_sigpipe() {
    // SAFETY: `sigemptyset` and `sigaddset` initialise the zeroed set they are given,
    // and `pthread_sigmask` reads that set and changes the calling thread's mask only;
    // none touches Rust-managed memory.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGPIPE);
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
    }
}

#[cfg(not(unix))]
fn restore_sigpipe() {}

#[cfg(not(unix))]
fn take_sigpipe() {}

/// 0 = pass, 1 = violations, 2 = the check itself could not run. Keeping the
/// last two apart lets CI tell "the change is bad" from "the gate is broken".
///
/// The work is done on a thread with a deep stack (`discipline::deep_stack`): a syntax
/// tree may nest further than the main thread's stack lets a walker descend.
fn main() -> ExitCode {
    restore_sigpipe();
    match discipline::deep_stack::on_deep_stack(|| {
        take_sigpipe();
        run()
    }) {
        Ok(code) => code,
        Err(e) => {
            eprintln!(
                "{}: could not start the thread the work runs on, with a stack of {} MiB: {e}",
                style::red("discipline: error"),
                discipline::deep_stack::WORK_STACK_BYTES / (1024 * 1024)
            );
            ExitCode::from(2)
        }
    }
}

fn run() -> ExitCode {
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
            // An error can quote a forge's answer or a file of the change. Its own line
            // breaks are kept (a parse error shows an excerpt under it); nothing else
            // that drives a terminal is.
            eprintln!(
                "{}: {}",
                style::red(&format!("discipline {cmd_name}: error")),
                discipline::report::text::terminal_text(&format!("{e:#}"))
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
            test_base_report: None,
            test_head_report: None,
            test_report: None,
            baseline_file: args.baseline_file,
            no_baseline: args.no_baseline,
            trust_workspace: args.trust_workspace,
            advisory: args.advisory,
            policy_from: discipline::cli::PolicyFrom::Head,
            comment: false,
        }),
        Commands::Baseline(args) => discipline::baseline::run_cli(args),
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
        Commands::Hook(args) => discipline::hook::run_cli(args),
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
        Commands::Audit(args) => {
            let summary = discipline::audit::run(&discipline::audit::Options {
                last: args.last,
                reference: args.reference,
                reasons: args.reasons,
                forge: args.forge,
                replay: args.replay,
            })?;
            use discipline::cli::AuditFormat;
            let format = if args.json {
                AuditFormat::Json
            } else {
                args.format
            };
            let out = match format {
                AuditFormat::Json => serde_json::to_string_pretty(&summary)? + "\n",
                AuditFormat::Html => discipline::audit_html::render(&summary),
                AuditFormat::Text => summary.render(),
            };
            match &args.output {
                Some(path) => std::fs::write(path, out)
                    .with_context(|| format!("cannot write {}", path.display()))?,
                None => print!("{out}"),
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
        Commands::Lease(args) => discipline::lease::run_cli(args),
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
        match discipline::forge::commit_origin(&api, &forge, oid) {
            Ok(discipline::forge::CommitOrigin::Merged(pull)) => {
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
            Ok(discipline::forge::CommitOrigin::DirectPush) => out.notes.push(format!(
                "merged-pr-body: commit {} arrived through no merged pull request (direct push); its message is the only directive source",
                &oid[..oid.len().min(10)]
            )),
            Ok(discipline::forge::CommitOrigin::NotOnForge) => out.notes.push(format!(
                "merged-pr-body: commit {} is not on {} (a local commit), so no merged pull request carries it; its message is the only directive source",
                &oid[..oid.len().min(10)],
                forge.kind.label()
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
    discipline::gitctx::event_payload()?
        .get("pull_request")
        .and_then(|pr| pr.get("body"))
        .and_then(|b| b.as_str())
        .filter(|b| !b.trim().is_empty())
        .map(|b| b.to_string())
}

fn detect_pull_context_from_ci() -> Option<discipline::override_policy::PullContext> {
    discipline::gitctx::event_payload()
        .and_then(|json| discipline::override_policy::pull_context(&json))
        .or_else(|| {
            // GitLab has no event payload; a merge-request pipeline sets these. No variable
            // names the merge request's author (`GITLAB_USER_LOGIN` is the login that started
            // the pipeline), so a check that needs the author reads it from the forge.
            let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
            let number = env("CI_MERGE_REQUEST_IID")?.parse().ok()?;
            let head_sha =
                env("CI_MERGE_REQUEST_SOURCE_BRANCH_SHA").or_else(|| env("CI_COMMIT_SHA"))?;
            Some(discipline::override_policy::PullContext {
                number,
                author: None,
                head_sha,
            })
        })
}

fn detect_pr_title_from_ci() -> Option<String> {
    discipline::gitctx::event_payload()?
        .get("pull_request")
        .and_then(|pr| pr.get("title"))
        .and_then(|b| b.as_str())
        .filter(|b| !b.trim().is_empty())
        .map(|b| b.to_string())
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
        refused_hidden_directives: Vec::new(),
        deprecations: Vec::new(),
        directive_notes: Vec::new(),
        unused_directives: Vec::new(),
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
    // Before the base is detected and before any reader of the payload: a payload variable
    // that names an unusable file stops the run here, so the readers below, which cannot
    // fail, never take it for a run with no payload.
    discipline::gitctx::try_event_payload()?;
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
    // A base copy whose only fault is a glob or pattern that does not compile would stop
    // the change that repairs it. Such a key is read from the change's copy, and said so.
    let repaired = match &head_config {
        Some(head) => discipline::guards::base_policy_repaired_by_head(&config, head, args.suite)?,
        None => None,
    };
    let (config, head_values) = repaired.unwrap_or((config, Vec::new()));
    for taken in &head_values {
        eprintln!(
            "{} --policy-from base: {}.",
            style::yellow("note:"),
            taken.note
        );
    }

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
    let read = discipline::tokens::read_directives(
        raw_pr_body.as_deref(),
        &commits,
        &merged.bodies,
        &config,
    );
    let (directives, mut directive_notes, refused_hidden_directives) =
        (read.active, read.notes, read.refused_hidden);
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
        // Named relative to the repository root when it is inside it: an absolute path
        // carries the runner's directory layout into the report.
        let shown = discipline::baseline::path_for_message(git.root(), &baseline_path);
        if baseline_path.exists() {
            let b = discipline::baseline::DisciplineBaseline::load_from_file_named(
                &baseline_path,
                &shown,
            )
            .map_err(|e| tag(Reason::Baseline, e))?;
            (Some(baseline_filename), Some(b))
        } else if explicit_baseline.is_some() {
            return Err(tag(
                Reason::Baseline,
                anyhow::anyhow!("baseline file `{shown}` does not exist"),
            ));
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    let forge_api = discipline::forge::HttpApi::from_env();
    let identify_forge = || discipline::forge::detect_for(&git);
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
        test_base_report: args.test_base_report.clone(),
        test_head_report: args.test_head_report.clone(),
        test_report: args.test_report.clone(),
        forge: Some(discipline::guards::ForgeAccess {
            api: &forge_api,
            identify: &identify_forge,
            pull: detect_pull_context_from_ci(),
        }),
    };
    let mut summary = run_checks(&config, args.suite, &ctx)?;
    discipline::guards::note_head_values(&mut summary, &head_values);

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
            author: Some(one.author.clone()),
            head_sha: one.head_sha.clone(),
        }),
        _ => None,
    });
    summary.refused_hidden_directives = refused_hidden_directives;
    summary.policy_failures = discipline::override_policy::judge(
        &config.directives,
        summary.directive_overrides(),
        summary.inline_overrides(),
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
                "comment: not posted on #{}: {}. A pull request from a fork runs with a token that cannot write; the check's status still carries the verdict",
                pull.number,
                discipline::report::text::terminal_line(&e)
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
    let target_root = repo_root.unwrap_or_else(|| std::path::Path::new("."));
    let starter = discipline::init::generate_starter(&project_name, target_root);
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

fn doctor(args: discipline::cli::DoctorArgs) -> Result<bool> {
    use discipline::doctor::{run, DoctorInput};
    let git = GitCtx::open_whole_tree()?;
    let env = |k: &str| std::env::var(k).ok();
    let mut forge = discipline::forge::detect_for(&git);
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
        copilot_home: discipline::hook::copilot_home(),
        home: std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .filter(|h| !h.is_empty())
            .map(std::path::PathBuf::from),
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
