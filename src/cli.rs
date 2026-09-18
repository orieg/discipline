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
pub enum Commands {
    /// Run the configured gates. Exit 0 = pass, 1 = violations, 2 = could not check
    Check(CheckArgs),
    /// Shorthand for `check --suite agent-guard` against `HEAD~1`
    Diff(DiffArgs),
    /// Write a discipline.toml with every available gate at its default
    Init(InitArgs),
    /// List every gate: id, suite, availability, and effective state
    Gates(GatesArgs),
    /// Run the embedded negative / positive controls against this binary
    SelfTest,
}

#[derive(Args, Debug, Clone)]
pub struct ConfigArgs {
    /// Path to discipline.toml. Absent file = built-in defaults (all gates on)
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

    /// Base branch or commit to measure the change against
    #[arg(
        short,
        long,
        default_value = "origin/main",
        env = "DISCIPLINE_BASE_REF"
    )]
    pub base: String,

    /// Inspect the index against HEAD instead (pre-commit hook mode)
    #[arg(long)]
    pub staged: bool,

    /// File holding the PR body (override directives, PR-body hygiene).
    /// Falls back to the PR_BODY environment variable
    #[arg(long)]
    pub pr_body_file: Option<PathBuf>,

    /// Treat warnings as failures
    #[arg(long, env = "DISCIPLINE_FAIL_ON_WARNINGS")]
    pub fail_on_warnings: bool,

    /// Treat applied overrides as failures (requires human sign-off)
    #[arg(long, env = "DISCIPLINE_FAIL_ON_OVERRIDES")]
    pub fail_on_overrides: bool,

    /// Comma-separated list of allowed directive sources (pr-body, commits)
    #[arg(long, env = "DISCIPLINE_DIRECTIVE_SOURCES", value_delimiter = ',')]
    pub directive_sources: Vec<String>,

    /// Output format
    #[arg(short, long, value_enum, default_value_t = OutputFormat::Terminal)]
    pub format: OutputFormat,

    /// Also write the JSON report to this path, whatever --format is
    #[arg(long)]
    pub json_out: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct DiffArgs {
    /// Base branch or commit ref to compare against
    #[arg(short, long, default_value = "HEAD~1")]
    pub base: String,
}

#[derive(Args, Debug)]
pub struct InitArgs {
    /// Name of the project (defaults to current directory name)
    #[arg(short, long)]
    pub name: Option<String>,
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
}
