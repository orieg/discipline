use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "discipline")]
#[command(version, about = "Universal CI/CD gatekeeper and AI coding agent diff sentinel", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Commands {
    /// Run the configured gates. Exit 0 = pass, 1 = violations, 2 = could not check
    Check(CheckArgs),
    /// Shorthand for checking uncommitted or working tree changes against HEAD
    Diff(DiffArgs),
    /// Record or manage grandfathered finding baselines
    Baseline(BaselineArgs),
    /// Write a discipline.toml with every available gate at its default
    Init(InitArgs),
    /// List every gate: id, suite, availability, and effective state
    Gates(GatesArgs),
    /// Print the JSON Schema for discipline.toml
    Schema,
    /// Run the embedded negative / positive controls against this binary
    SelfTest,
    /// Generate shell completion script to stdout (bash, zsh, fish, powershell, elvish)
    Completions(CompletionsArgs),
    /// Generate or check reference docs and schemas against sources of truth
    #[command(hide = true)]
    Docs(DocsArgs),
    /// Install pre-commit hook in the local git repository
    InstallHooks(InstallHooksArgs),
    /// Run the gates inside a coding agent's edit loop (Claude Code, Codex, Cursor, Aider)
    Hook(HookArgs),
    /// Explain a gate: what it checks, its state here, and the directive that lifts a finding
    Explain(ExplainArgs),
    /// Replay the last N merged changes through a configuration: what it would have blocked
    Replay(ReplayArgs),
    /// Serve the gates to an MCP client over stdio (read-only tools: check_diff, list_gates, explain_finding)
    Mcp,
    /// Benchmark tooling for the bench-regression gate
    Bench(BenchArgs),
    /// Check that the repository and its platform enforce discipline: workflows, CODEOWNERS, branch protection. Exit 0 = healthy, 1 = a failing check, 2 = could not check
    Doctor(DoctorArgs),
}

#[derive(Args, Debug)]
pub struct BenchArgs {
    #[command(subcommand)]
    pub command: BenchCommand,
}

#[derive(Subcommand, Debug)]
pub enum BenchCommand {
    /// Derive paired-ratio noise floors and baseline ratios from repeated same-commit runs
    Derive(BenchDeriveArgs),
}

#[derive(Args, Debug)]
pub struct BenchDeriveArgs {
    /// `discipline-bench-ratio/v1` run files of the same code (at least two)
    #[arg(required = true, num_args = 2..)]
    pub runs: Vec<PathBuf>,

    /// Merge the derived platform entry into this ratio baseline file (created if absent)
    #[arg(long)]
    pub baseline: Option<PathBuf>,

    /// Accept runs of different commits (recorded in the baseline); only when the differences cannot move a number
    #[arg(long)]
    pub allow_mixed_commits: bool,

    /// Derived cell floors above this percentage are reported but not gated
    #[arg(long, default_value_t = 50.0)]
    pub ceiling_pct: f64,
}

impl Commands {
    pub fn name(&self) -> &'static str {
        match self {
            Commands::Check(_) => "check",
            Commands::Diff(_) => "diff",
            Commands::Baseline(_) => "baseline",
            Commands::Init(_) => "init",
            Commands::Gates(_) => "gates",
            Commands::Schema => "schema",
            Commands::SelfTest => "self-test",
            Commands::Completions(_) => "completions",
            Commands::Docs(_) => "docs",
            Commands::InstallHooks(_) => "install-hooks",
            Commands::Hook(_) => "hook",
            Commands::Mcp => "mcp",
            Commands::Replay(_) => "replay",
            Commands::Explain(_) => "explain",
            Commands::Bench(_) => "bench",
            Commands::Doctor(_) => "doctor",
        }
    }
}

#[derive(Args, Debug, Clone)]
pub struct CompletionsArgs {
    /// Target shell for completion script
    #[arg(value_enum)]
    pub shell: clap_complete::Shell,
}

#[derive(Args, Debug, Clone)]
pub struct InstallHooksArgs {
    /// Overwrite existing pre-commit hook if present
    #[arg(short, long)]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct ReplayArgs {
    /// Number of first-parent commits (merged changes) to replay, newest first
    #[arg(long)]
    pub last: usize,

    /// Branch whose history is replayed (default: origin's default branch, else main / master)
    #[arg(long = "ref")]
    pub reference: Option<String>,

    /// Configuration under test (default: discipline.toml in the working tree)
    #[arg(short, long)]
    pub config: Option<PathBuf>,

    /// Print the summary as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct ExplainArgs {
    /// A gate id (`assertion-reduction`) or a finding line naming `[gate-id]`
    pub query: String,

    #[command(flatten)]
    pub config: ConfigArgs,
}

#[derive(Args, Debug)]
pub struct HookArgs {
    #[command(subcommand)]
    pub command: HookCommand,
}

#[derive(Subcommand, Debug)]
pub enum HookCommand {
    /// Check the change so far and answer in the agent's hook contract (reads the hook payload on stdin)
    Run(HookRunArgs),
    /// Write the agent's hook configuration at the repository root; an existing file is never rewritten
    Install(HookInstallArgs),
}

#[derive(Args, Debug)]
pub struct HookRunArgs {
    /// The agent whose hook contract to answer in
    #[arg(long, value_enum)]
    pub agent: crate::hook::Agent,

    /// Base to measure the change against (default: the merge base with origin's default branch, else main / master)
    #[arg(short, long)]
    pub base: Option<String>,

    /// Files an agent appends to the command (Aider's lint-cmd); ignored, the whole change is checked
    #[arg(hide = true, trailing_var_arg = true)]
    pub files: Vec<String>,
}

#[derive(Args, Debug)]
pub struct HookInstallArgs {
    /// The agent to configure
    #[arg(long, value_enum)]
    pub agent: crate::hook::Agent,
}

#[derive(Args, Debug, Clone)]
pub struct DocsArgs {
    /// Check that reference documentation is up to date with sources
    #[arg(long, conflicts_with = "write")]
    pub check: bool,

    /// Regenerate and write reference documentation across the repository
    #[arg(long, conflicts_with = "check")]
    pub write: bool,
}

#[derive(Args, Debug, Clone)]
pub struct ConfigArgs {
    /// Path to discipline.toml. Absent file = built-in defaults (`discipline gates` lists them)
    #[arg(
        short,
        long,
        default_value = "discipline.toml",
        env = "DISCIPLINE_CONFIG"
    )]
    pub config: PathBuf,

    /// Inline TOML merged over the file (tables merge, lists append, scalars replace)
    #[arg(long, env = "DISCIPLINE_CONFIG_OVERRIDE", hide_env_values = true)]
    pub config_override: Option<String>,

    /// Gate ids to force on (comma separated)
    #[arg(long, value_delimiter = ',', env = "DISCIPLINE_ENABLE")]
    pub enable: Vec<String>,

    /// Gate ids to force off (comma separated)
    #[arg(long, value_delimiter = ',', env = "DISCIPLINE_DISABLE")]
    pub disable: Vec<String>,
}

#[derive(Args, Debug)]
pub struct CheckArgs {
    #[command(flatten)]
    pub config: ConfigArgs,

    /// Which check suite to run
    #[arg(short, long, value_enum, default_value_t = SuiteChoice::All)]
    pub suite: SuiteChoice,

    /// Base branch or commit to measure the change against (auto-detected in CI if omitted)
    #[arg(short, long, env = "DISCIPLINE_BASE_REF")]
    pub base: Option<String>,

    /// Specific commit to inspect (compares against parent commit <sha>~1)
    #[arg(long, conflicts_with = "commit_range", conflicts_with = "staged")]
    pub commit: Option<String>,

    /// Commit range to inspect (<before>..<after> or <before>...<after>)
    #[arg(long, conflicts_with = "commit", conflicts_with = "staged")]
    pub commit_range: Option<String>,

    /// Inspect the index against HEAD instead (pre-commit hook mode)
    #[arg(long, conflicts_with = "commit", conflicts_with = "commit_range")]
    pub staged: bool,

    /// File holding the PR body or commit message (override directives, hygiene scanning).
    /// Falls back to the PR_BODY environment variable
    #[arg(long, visible_alias = "commit-msg-file")]
    pub pr_body_file: Option<PathBuf>,

    /// PR title for PR-level hygiene checks (e.g. issue-link).
    /// Falls back to the PR_TITLE environment variable
    #[arg(long, env = "PR_TITLE")]
    pub pr_title: Option<String>,

    /// Treat warnings as failures
    #[arg(long, env = "DISCIPLINE_FAIL_ON_WARNINGS")]
    pub fail_on_warnings: bool,

    /// Treat applied overrides as failures (requires human sign-off)
    #[arg(long, env = "DISCIPLINE_FAIL_ON_OVERRIDES")]
    pub fail_on_overrides: bool,

    /// Advisory mode: run all checks and emit reports, but exit code 0 even if violations occur
    #[arg(long, env = "DISCIPLINE_ADVISORY")]
    pub advisory: bool,

    /// Post the report as one pull-request comment, edited on every run (needs a token that can write comments; off by default)
    #[arg(long, env = "DISCIPLINE_COMMENT")]
    pub comment: bool,

    /// Which side's discipline.toml judges the change. `base` reads it from the base ref,
    /// so a policy edit takes effect once merged; `config-integrity` still reports it
    #[arg(
        long,
        value_enum,
        default_value = "head",
        env = "DISCIPLINE_POLICY_FROM"
    )]
    pub policy_from: PolicyFrom,

    /// Actor executing the check (for actor-aware override authorization).
    /// Falls back to DISCIPLINE_ACTOR, GITHUB_ACTOR, GITEA_ACTOR, FORGEJO_ACTOR, GITLAB_USER_LOGIN
    #[arg(long, env = "DISCIPLINE_ACTOR")]
    pub actor: Option<String>,

    /// Comma-separated list of allowed directive sources (pr-body, commits, merged-pr-body)
    #[arg(long, env = "DISCIPLINE_DIRECTIVE_SOURCES", value_delimiter = ',')]
    pub directive_sources: Vec<String>,

    /// Trust the workspace and disable libgit2 repository owner validation (off by default, or set DISCIPLINE_TRUST_WORKSPACE=1)
    #[arg(long)]
    pub trust_workspace: bool,

    /// Suppress output on success (only print output when violations are found)
    #[arg(short, long)]
    pub quiet: bool,

    /// Output format
    #[arg(short, long, value_enum, default_value_t = OutputFormat::Terminal)]
    pub format: OutputFormat,

    /// Also write the JSON report to this path, whatever --format is
    #[arg(long)]
    pub json_out: Option<PathBuf>,

    /// Write the formatted report to this path
    #[arg(short = 'o', long = "output-file")]
    pub output_file: Option<PathBuf>,

    /// Write GitLab Code Quality JSON report to this path
    #[arg(long, env = "DISCIPLINE_REPORT_GITLAB")]
    pub report_gitlab: Option<PathBuf>,

    /// Write JUnit XML report to this path
    #[arg(long, env = "DISCIPLINE_REPORT_JUNIT")]
    pub report_junit: Option<PathBuf>,

    /// Write SARIF report to this path
    #[arg(long, env = "DISCIPLINE_REPORT_SARIF")]
    pub report_sarif: Option<PathBuf>,

    /// Expected host or runner provenance tag for benchmark artifacts
    #[arg(long = "bench-provenance", env = "DISCIPLINE_BENCH_PROVENANCE")]
    pub bench_provenance: Option<String>,

    /// Allow benchmark comparison across mismatched host/runner provenance tags
    #[arg(
        long = "allow-cross-host-bench",
        env = "DISCIPLINE_ALLOW_CROSS_HOST_BENCH"
    )]
    pub allow_cross_host_bench: bool,

    /// In-job base benchmark result file for bench-regression dual-mode
    #[arg(long = "bench-base-file", env = "DISCIPLINE_BENCH_BASE_FILE")]
    pub bench_base_file: Option<PathBuf>,

    /// In-job head benchmark result file for bench-regression dual-mode
    #[arg(long = "bench-head-file", env = "DISCIPLINE_BENCH_HEAD_FILE")]
    pub bench_head_file: Option<PathBuf>,

    /// Path to grandfathering baseline file (defaults to discipline-baseline.toml if present)
    #[arg(long)]
    pub baseline_file: Option<PathBuf>,

    /// Ignore grandfathering baseline even if present
    #[arg(long)]
    pub no_baseline: bool,
}

#[derive(Args, Debug)]
pub struct DiffArgs {
    #[command(flatten)]
    pub config: ConfigArgs,

    /// Which check suite to run
    #[arg(short, long, value_enum, default_value_t = SuiteChoice::All)]
    pub suite: SuiteChoice,

    /// Base branch or commit ref to compare against (defaults to HEAD for uncommitted changes)
    #[arg(short, long)]
    pub base: Option<String>,

    /// Output format
    #[arg(short, long, value_enum, default_value_t = OutputFormat::Terminal)]
    pub format: OutputFormat,

    /// Also write the JSON report to this path, whatever --format is
    #[arg(long)]
    pub json_out: Option<PathBuf>,

    /// Write the formatted report to this path
    #[arg(short = 'o', long = "output-file")]
    pub output_file: Option<PathBuf>,

    /// Write GitLab Code Quality JSON report to this path
    #[arg(long, env = "DISCIPLINE_REPORT_GITLAB")]
    pub report_gitlab: Option<PathBuf>,

    /// Write JUnit XML report to this path
    #[arg(long, env = "DISCIPLINE_REPORT_JUNIT")]
    pub report_junit: Option<PathBuf>,

    /// Write SARIF report to this path
    #[arg(long, env = "DISCIPLINE_REPORT_SARIF")]
    pub report_sarif: Option<PathBuf>,

    /// Path to grandfathering baseline file (defaults to discipline-baseline.toml if present)
    #[arg(long)]
    pub baseline_file: Option<PathBuf>,

    /// Ignore grandfathering baseline even if present
    #[arg(long)]
    pub no_baseline: bool,

    /// Trust the workspace and disable libgit2 repository owner validation (off by default, or set DISCIPLINE_TRUST_WORKSPACE=1)
    #[arg(long)]
    pub trust_workspace: bool,

    /// Advisory mode: run checks and emit reports, but exit 0 even if violations are found
    #[arg(long, env = "DISCIPLINE_ADVISORY")]
    pub advisory: bool,
}

#[derive(Args, Debug, Clone)]
pub struct BaselineArgs {
    #[command(flatten)]
    pub config: ConfigArgs,

    /// Record current findings to the baseline file
    #[arg(long)]
    pub write: bool,

    /// Path to grandfathering baseline file (defaults to discipline-baseline.toml)
    #[arg(long, default_value = "discipline-baseline.toml")]
    pub baseline_file: PathBuf,

    /// Base branch or commit ref to compare against
    #[arg(short, long, env = "DISCIPLINE_BASE_REF")]
    pub base: Option<String>,

    /// Specific suite to run: all, agent-guard, hygiene, integrity ...
    #[arg(short, long, value_enum, default_value_t = SuiteChoice::All)]
    pub suite: SuiteChoice,

    /// Record every pre-existing finding in the tree, not just the diff.
    /// Use when adopting discipline on an existing repository; conflicts with --base
    #[arg(long, conflicts_with = "base")]
    pub whole_tree: bool,

    /// Also record warnings and notes. By default only findings that would
    /// block under the current configuration are recorded: `error`, plus
    /// `warning` under --fail-on-warnings
    #[arg(long)]
    pub all_severities: bool,

    /// Treat warnings as blocking when choosing what to record (same switch
    /// as `check --fail-on-warnings`)
    #[arg(long, env = "DISCIPLINE_FAIL_ON_WARNINGS")]
    pub fail_on_warnings: bool,

    /// Trust the workspace and disable libgit2 repository owner validation
    #[arg(long)]
    pub trust_workspace: bool,
}

#[derive(Args, Debug)]
pub struct InitArgs {
    /// Name of the project (defaults to current directory name)
    #[arg(short, long)]
    pub name: Option<String>,
}

#[derive(Args, Debug)]
pub struct DoctorArgs {
    /// Branch whose protection is checked (default: the repository's default branch)
    #[arg(long)]
    pub branch: Option<String>,
    /// Repository path on the forge, e.g. OWNER/NAME (default: from the CI environment or the `origin` remote; set DISCIPLINE_FORGE for a self-hosted forge)
    #[arg(long)]
    pub repo: Option<String>,
    /// Check only local files; skip the platform API
    #[arg(long)]
    pub local_only: bool,
    /// Treat warnings as failures
    #[arg(long)]
    pub strict: bool,
    /// Output format
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: DoctorFormat,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoctorFormat {
    Text,
    Json,
}

#[derive(Args, Debug)]
pub struct GatesArgs {
    #[command(flatten)]
    pub config: ConfigArgs,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuiteChoice {
    All,
    AgentGuard,
    Hygiene,
    Integrity,
    Quality,
    Verification,
    Bench,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    Terminal,
    GithubSummary,
    Json,
    Junit,
    Sarif,
    Gitlab,
    AgentPrompt,
}

/// Which side's configuration judges a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum PolicyFrom {
    /// The configuration in the working tree (the change's own copy).
    Head,
    /// The configuration on the base ref.
    Base,
}
