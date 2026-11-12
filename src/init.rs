//! `discipline init`: initialize a starter `discipline.toml` configuration.
//!
//! Detects common test runners (cargo/nextest, pytest, jest/vitest, go test, Maven/Gradle)
//! and emits a commented `[gates.test-floor]` block with `test_command` and `test_report`
//! pre-configured for JUnit identity ratcheting.

use std::path::Path;

/// Recognized test runners that produce JUnit XML test reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestRunner {
    CargoNextest,
    Cargo,
    Pytest,
    Vitest,
    Jest,
    Go,
    Maven,
    Gradle,
}

impl TestRunner {
    pub fn name(self) -> &'static str {
        match self {
            TestRunner::CargoNextest => "cargo nextest",
            TestRunner::Cargo => "cargo test",
            TestRunner::Pytest => "pytest",
            TestRunner::Vitest => "vitest",
            TestRunner::Jest => "jest",
            TestRunner::Go => "go test",
            TestRunner::Maven => "Maven",
            TestRunner::Gradle => "Gradle",
        }
    }

    pub fn test_command(self) -> &'static str {
        match self {
            TestRunner::CargoNextest | TestRunner::Cargo => "cargo nextest run --profile ci",
            TestRunner::Pytest => "pytest --junitxml=reports/junit.xml",
            TestRunner::Vitest => "npx vitest run --reporter=junit --outputFile=reports/junit.xml",
            TestRunner::Jest => "npm test -- --reporters=jest-junit",
            TestRunner::Go => "gotestsum --junitfile reports/junit.xml",
            TestRunner::Maven => "mvn test",
            TestRunner::Gradle => "gradle test",
        }
    }

    pub fn test_report(self) -> &'static str {
        match self {
            TestRunner::CargoNextest | TestRunner::Cargo => "target/nextest/ci/junit.xml",
            TestRunner::Pytest | TestRunner::Vitest | TestRunner::Jest | TestRunner::Go => {
                "reports/junit.xml"
            }
            TestRunner::Maven => "target/surefire-reports/junit.xml",
            TestRunner::Gradle => "build/test-results/test/junit.xml",
        }
    }

    pub fn config_snippet(self) -> String {
        format!(
            "# [gates.test-floor]\n# test_command = \"{}\"\n# test_report = \"{}\"\n# min_tests = 1",
            self.test_command(),
            self.test_report()
        )
    }

    pub fn mutation_preset(self) -> Option<&'static str> {
        match self {
            TestRunner::CargoNextest | TestRunner::Cargo => Some("cargo-mutants"),
            TestRunner::Pytest => Some("mutmut"),
            TestRunner::Vitest | TestRunner::Jest => Some("stryker"),
            TestRunner::Maven | TestRunner::Gradle => Some("pit"),
            TestRunner::Go => None,
        }
    }

    /// The command to set beside [`Self::mutation_preset`] when the preset's own command
    /// is another build tool's: `pit` runs Maven, so a Gradle project names its own task
    /// (the `pitest` task of the gradle-pitest plugin) and keeps the preset's guards.
    pub fn mutation_command(self) -> Option<&'static str> {
        match self {
            TestRunner::Gradle => Some("gradle pitest"),
            _ => None,
        }
    }

    /// Whether [`Self::mutation_preset`]'s command mutates only what the change touched.
    /// Only `cargo mutants --in-diff {diff_file}` does; the other presets run the tool over the
    /// whole project.
    pub fn mutation_preset_is_diff_scoped(self) -> bool {
        matches!(self, TestRunner::CargoNextest | TestRunner::Cargo)
    }

    pub fn mutation_snippet(self) -> Option<String> {
        self.mutation_preset().map(|preset| {
            let heading = if self.mutation_preset_is_diff_scoped() {
                "# Diff-scoped mutation testing (guards against special-cased test inputs):"
            } else {
                "# Mutation testing (guards against special-cased test inputs; this preset's command runs over the whole project, not only the diff):"
            };
            let command = match self.mutation_command() {
                Some(command) => format!("\n# command = \"{command}\""),
                None => String::new(),
            };
            format!(
                "{heading}\n# [[gates.command.commands]]\n# name = \"mutation\"\n# preset = \"{preset}\"{command}"
            )
        })
    }

    /// Detect test runner present in the directory `root`.
    pub fn detect(root: &Path) -> Option<Self> {
        // Rust
        if root.join("Cargo.toml").exists() {
            if root.join(".config/nextest.toml").exists() {
                return Some(TestRunner::CargoNextest);
            }
            if let Ok(content) = std::fs::read_to_string(root.join("Cargo.toml")) {
                if content.contains("nextest") {
                    return Some(TestRunner::CargoNextest);
                }
            }
            return Some(TestRunner::Cargo);
        }

        // Python
        if root.join("pytest.ini").exists()
            || root.join("conftest.py").exists()
            || root.join("tests/conftest.py").exists()
        {
            return Some(TestRunner::Pytest);
        }
        if let Ok(content) = std::fs::read_to_string(root.join("pyproject.toml")) {
            if content.contains("pytest") {
                return Some(TestRunner::Pytest);
            }
        }
        if let Ok(content) = std::fs::read_to_string(root.join("setup.cfg")) {
            if content.contains("pytest") {
                return Some(TestRunner::Pytest);
            }
        }

        // Go
        if root.join("go.mod").exists() {
            return Some(TestRunner::Go);
        }

        // JS / TS
        if root.join("vitest.config.ts").exists()
            || root.join("vitest.config.js").exists()
            || root.join("vitest.config.mts").exists()
            || root.join("vitest.config.mjs").exists()
        {
            return Some(TestRunner::Vitest);
        }
        if root.join("jest.config.js").exists()
            || root.join("jest.config.ts").exists()
            || root.join("jest.config.json").exists()
            || root.join("jest.config.mjs").exists()
            || root.join("jest.config.cjs").exists()
        {
            return Some(TestRunner::Jest);
        }
        if let Ok(content) = std::fs::read_to_string(root.join("package.json")) {
            if content.contains("\"vitest\"") {
                return Some(TestRunner::Vitest);
            }
            if content.contains("\"jest\"") || content.contains("\"test\"") {
                return Some(TestRunner::Jest);
            }
        }

        // Java / Kotlin / JVM
        if root.join("pom.xml").exists() {
            return Some(TestRunner::Maven);
        }
        if root.join("build.gradle").exists() || root.join("build.gradle.kts").exists() {
            return Some(TestRunner::Gradle);
        }

        // Fallback for Python repositories with requirements or setup.py, or a
        // `tests/test_*.py` layout, pytest's default discovery
        if root.join("requirements.txt").exists()
            || root.join("setup.py").exists()
            || root.join("pyproject.toml").exists()
            || has_pytest_test_file(&root.join("tests"))
        {
            return Some(TestRunner::Pytest);
        }

        None
    }
}

/// Whether `dir` holds a `test_*.py` file, the name pytest collects by default.
fn has_pytest_test_file(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().any(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("test_") && name.ends_with(".py")
        })
    })
}

/// A configuration profile `discipline init --profile` writes (#1075). The set is closed:
/// each name is a public interface (`tests/fixtures/v1_surface.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Profile {
    /// The built-in defaults in advisory mode: the first-week start (README, First Week).
    Observe,
    /// The human-review controls for a repository where coding agents open pull requests.
    AgentGuard,
}

impl Profile {
    pub const ALL: &'static [Profile] = &[Profile::Observe, Profile::AgentGuard];

    pub fn name(self) -> &'static str {
        match self {
            Profile::Observe => "observe",
            Profile::AgentGuard => "agent-guard",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|p| p.name() == name)
    }

    /// The keys the profile writes, in the order they are written. `discipline doctor`'s
    /// `init-profile` check and the table in `docs/CONFIGURATION.md` read the same list.
    pub fn keys(self) -> &'static [ProfileKey] {
        match self {
            Profile::Observe => OBSERVE_KEYS,
            Profile::AgentGuard => AGENT_GUARD_KEYS,
        }
    }
}

/// Which variant of a profile writes a key: `agent-guard` writes the review-based keys
/// only when a reviewer login other than the agents' is named (`--reviewer`), since a
/// ratification or approval by the login that opens the pull requests shows no second
/// party (#776).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    Always,
    WithReviewer,
    WithoutReviewer,
}

impl When {
    pub fn applies(self, with_reviewer: bool) -> bool {
        match self {
            When::Always => true,
            When::WithReviewer => with_reviewer,
            When::WithoutReviewer => !with_reviewer,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            When::Always => "always",
            When::WithReviewer => "with `--reviewer`",
            When::WithoutReviewer => "without `--reviewer`",
        }
    }
}

/// The value a profile key takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileValue {
    /// A fixed TOML value, as written.
    Toml(&'static str),
    /// The `--reviewer` logins.
    Reviewers,
    /// The `--agent-login` logins; the key is left out when none is named.
    AgentLogins,
}

impl ProfileValue {
    pub fn label(self) -> &'static str {
        match self {
            ProfileValue::Toml(v) => v,
            ProfileValue::Reviewers => "the `--reviewer` logins",
            ProfileValue::AgentLogins => "the `--agent-login` logins, when any is named",
        }
    }
}

/// One key a profile writes: `table.key = value`, and why.
#[derive(Debug, Clone, Copy)]
pub struct ProfileKey {
    pub table: &'static str,
    pub key: &'static str,
    pub value: ProfileValue,
    pub when: When,
    pub why: &'static str,
}

const OBSERVE_KEYS: &[ProfileKey] = &[ProfileKey {
    table: "meta",
    key: "mode",
    value: ProfileValue::Toml("\"advisory\""),
    when: When::Always,
    why: "findings are reported and never fail a run; delete the line to enforce",
}];

/// The policy files `agent-guard` protects with `ratified-paths`. The workflow
/// directories are in the gate's `never_ratifiable` default, so their edits fail it.
const AGENT_GUARD_PROTECTED: &str =
    "[\"discipline.toml\", \"discipline-baseline.toml\", \"AGENTS.md\", \"CLAUDE.md\", \"GEMINI.md\"]";

const AGENT_GUARD_KEYS: &[ProfileKey] = &[
    ProfileKey {
        table: "directives",
        key: "sources",
        value: ProfileValue::Toml("[\"pr-body\"]"),
        when: When::Always,
        why: "a directive is read from the reviewed pull-request body only, never from a commit message an agent writes",
    },
    ProfileKey {
        table: "directives",
        key: "max_overrides",
        value: ProfileValue::Toml("1"),
        when: When::Always,
        why: "one change applies at most one directive",
    },
    ProfileKey {
        table: "directives",
        key: "fail_on_overrides",
        value: ProfileValue::Toml("true"),
        when: When::WithoutReviewer,
        why: "every override a change writes fails the run, so a person decides whether it merges",
    },
    ProfileKey {
        table: "directives",
        key: "allowed_override_actors",
        value: ProfileValue::Reviewers,
        when: When::WithReviewer,
        why: "the logins whose approving review lets a directive apply",
    },
    ProfileKey {
        table: "directives",
        key: "require_approval",
        value: ProfileValue::Toml("true"),
        when: When::WithReviewer,
        why: "a directive applies only after a reviewer other than the author approves the head commit",
    },
    ProfileKey {
        table: "gates.review-threads",
        key: "enabled",
        value: ProfileValue::Toml("true"),
        when: When::Always,
        why: "a pull request with an unresolved review thread fails",
    },
    ProfileKey {
        table: "gates.ratified-paths",
        key: "enabled",
        value: ProfileValue::Toml("true"),
        when: When::WithReviewer,
        why: "an edit to the policy files needs a reviewer's ratification; a workflow edit fails (`never_ratifiable`)",
    },
    ProfileKey {
        table: "gates.ratified-paths",
        key: "protected_paths",
        value: ProfileValue::Toml(AGENT_GUARD_PROTECTED),
        when: When::WithReviewer,
        why: "the gate configuration, its baseline and the agent instructions",
    },
    ProfileKey {
        table: "gates.ratified-paths",
        key: "ratifiers",
        value: ProfileValue::Reviewers,
        when: When::WithReviewer,
        why: "the logins whose comment ratifies",
    },
    ProfileKey {
        table: "gates.ratified-paths",
        key: "agent_logins",
        value: ProfileValue::AgentLogins,
        when: When::WithReviewer,
        why: "logins that never ratify",
    },
    ProfileKey {
        table: "gates.ratified-paths",
        key: "refuse_author_ratification",
        value: ProfileValue::Toml("true"),
        when: When::WithReviewer,
        why: "a ratification by the pull request's own author does not count",
    },
];

/// The line a profile's file starts with; `discipline doctor` reads it back.
pub fn profile_marker(profile: Profile) -> String {
    format!(
        "# Written by `discipline init --profile {}`.",
        profile.name()
    )
}

/// The profile a file was written from, read from its marker line.
pub fn profile_of(content: &str) -> Option<Profile> {
    content.lines().find_map(|line| {
        line.trim_end()
            .strip_prefix("# Written by `discipline init --profile ")
            .and_then(|rest| rest.strip_suffix("`."))
            .and_then(Profile::from_name)
    })
}

/// Logins `--reviewer` and `--agent-login` name.
#[derive(Debug, Clone, Default)]
pub struct ProfileLogins {
    pub reviewers: Vec<String>,
    pub agent_logins: Vec<String>,
}

/// A forge login as it can be written in the file: letters, digits, `-`, `_`, `.`, and
/// the `[bot]` suffix of an app account.
fn valid_login(login: &str) -> bool {
    !login.is_empty()
        && login
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '[' | ']'))
}

fn toml_list(logins: &[String]) -> String {
    let items: Vec<String> = logins.iter().map(|l| format!("\"{l}\"")).collect();
    format!("[{}]", items.join(", "))
}

/// What `init` writes, and what it could not enable and why.
#[derive(Debug, Clone)]
pub struct Generated {
    pub content: String,
    pub notes: Vec<String>,
}

/// Generates the starter `discipline.toml` content for a project.
pub fn generate_starter(project_name: &str, root: &Path) -> String {
    generate(project_name, root, None, &ProfileLogins::default())
        .expect("no profile, no logins to refuse")
        .content
}

/// Generates `discipline.toml` for a project, from a profile when one is named.
pub fn generate(
    project_name: &str,
    root: &Path,
    profile: Option<Profile>,
    logins: &ProfileLogins,
) -> anyhow::Result<Generated> {
    for login in logins.reviewers.iter().chain(&logins.agent_logins) {
        if !valid_login(login) {
            anyhow::bail!("`{login}` is not a forge login");
        }
    }
    if profile != Some(Profile::AgentGuard)
        && !(logins.reviewers.is_empty() && logins.agent_logins.is_empty())
    {
        anyhow::bail!("`--reviewer` and `--agent-login` apply to `--profile agent-guard` only");
    }
    let mut notes = Vec::new();
    // A reviewer who is also an agent login is no second party.
    let reviewers: Vec<String> = logins
        .reviewers
        .iter()
        .filter(|r| {
            let agent = logins
                .agent_logins
                .iter()
                .any(|a| a.eq_ignore_ascii_case(r));
            if agent {
                notes.push(format!(
                    "`{r}` is also an `--agent-login`, so it is not used as a reviewer: an approval or ratification by the login the agents use shows no second party"
                ));
            }
            !agent
        })
        .cloned()
        .collect();
    let with_reviewer = !reviewers.is_empty();

    // With no runner detected the lines below name tools the repository shows no sign of,
    // so they are written as an example and say so.
    let detected = TestRunner::detect(root);
    let runner = detected.unwrap_or(TestRunner::CargoNextest);
    let example = if detected.is_some() {
        ""
    } else {
        "# No test runner was detected here. The commented lines below are an example for a\n# Cargo project: replace the command, the report path and the preset with your tools'.\n"
    };
    let test_floor_snippet = format!("{example}{}", runner.config_snippet());
    let mutation_part = match runner.mutation_snippet() {
        Some(s) => format!("\n{s}\n"),
        None => String::new(),
    };

    let header = match profile {
        None => String::new(),
        Some(p) => format!(
            "{}\n# `discipline doctor` reports a key below that is no longer set (`init-profile`).\n#\n",
            profile_marker(p)
        ),
    };
    let mode = if profile == Some(Profile::Observe) {
        "# Advisory mode reports findings without failing the run. Delete the line below to\n# enforce once the findings of a first period of pull requests are fixed or baselined\n# (`discipline baseline --write`). Setting it loosens every gate, so the change that\n# adds it carries `allow-gate-weakening: meta <reason>`.\nmode = \"advisory\"\n"
    } else {
        "# Advisory mode reports findings without failing the run. Uncomment it for a first\n# period of pull requests, and delete it to enforce once their findings are fixed or\n# baselined (`discipline baseline --write`). Setting it loosens every gate, so the change\n# that does carries `allow-gate-weakening: meta <reason>`.\n# mode = \"advisory\"\n"
    };
    let directives = if profile == Some(Profile::AgentGuard) {
        agent_guard_tables(&reviewers, &logins.agent_logins, with_reviewer)
    } else {
        "# [directives]\n# sources = [\"pr-body\", \"commits\", \"merged-pr-body\"]\n# allow_hidden = false\n# fail_on_overrides = false\n".to_string()
    };
    if profile == Some(Profile::Observe) {
        notes.push("the change that adds this file sets advisory mode, which `config-integrity` reports as a loosening: its pull-request body carries `allow-gate-weakening: meta <reason>`".to_string());
    }
    if profile == Some(Profile::AgentGuard) {
        if with_reviewer {
            notes.push("the change that adds this file grows `directives.allowed_override_actors` and `gates.ratified-paths.ratifiers`, which `config-integrity` reports as loosenings: its pull-request body carries `allow-gate-weakening: directives <reason>` and `allow-gate-weakening: ratified-paths <reason>`".to_string());
            if logins.agent_logins.is_empty() {
                notes.push("no `--agent-login` was named: `gates.ratified-paths.agent_logins` is left out, so `doctor`'s `agent-policy` check runs on `require_approval` alone".to_string());
            }
        } else {
            notes.push("`directives.require_approval` and `ratified-paths` are not enabled: they need a reviewer login other than the agents' (`--reviewer`), since an approval or ratification by the login that opens the pull requests shows no second party (#776); `fail_on_overrides` stands in, so any override fails the run".to_string());
        }
        notes.push("the controls hold only when the workflow judges a change by the base branch's policy: `policy_from: base` under the action's `with:` (`--policy-from base` elsewhere), and the check is required on the default branch (`discipline doctor`)".to_string());
    }

    let content = format!(
        r#"{header}# discipline.toml — configuration for Discipline CI gatekeeper.
#
# Schema version 1. Gates run at their built-in default enablement and severity.
# You only need to specify settings that differ from the defaults.
# Run `discipline gates` to view the effective status of all gates.

[meta]
version = 1
name = "{project_name}"
# description = "Brief description of the project"
{mode}
{directives}
# Gate customizations (examples):
# [gates.assertion-reduction]
# severity = "error"
# exempt_paths = ["tests/legacy/**"]

{test_floor_snippet}
{mutation_part}
# [gates.pii]
# allowed_users = ["runner", "user", "username"]
# hostname_denylist = ["internal.corp"]
# term_denylist = ["secret-project", "case:InternalCodeName"]

# [gates.time-estimates]
# allow_patterns = ['^timeout: \d+']
"#
    );
    Ok(Generated { content, notes })
}

/// The `agent-guard` tables, rendered from [`AGENT_GUARD_KEYS`].
fn agent_guard_tables(reviewers: &[String], agents: &[String], with_reviewer: bool) -> String {
    let mut out = String::from(
        "# The workflow that runs discipline sets `policy_from: base` under the action's\n# `with:` (`--policy-from base` elsewhere): a change is judged by the base branch's\n# policy, so it cannot loosen the policy that judges it.\n",
    );
    let mut table = "";
    for key in AGENT_GUARD_KEYS {
        if !key.when.applies(with_reviewer) {
            continue;
        }
        let value = match key.value {
            ProfileValue::Toml(v) => v.to_string(),
            ProfileValue::Reviewers => toml_list(reviewers),
            ProfileValue::AgentLogins if agents.is_empty() => continue,
            ProfileValue::AgentLogins => toml_list(agents),
        };
        if key.table != table {
            table = key.table;
            out.push_str(&format!("\n[{table}]\n"));
        }
        out.push_str(&format!("# {}\n{} = {value}\n", key.why, key.key));
    }
    out
}

/// The keys of `profile` that `content` no longer sets as the profile wrote them. A
/// login list is only checked to be non-empty. `agent-guard`'s reviewer variant is the
/// one a file is read as when it names a reviewer anywhere the profile puts one.
pub fn missing_profile_keys(profile: Profile, content: &str) -> anyhow::Result<Vec<String>> {
    let doc: toml::Value = toml::from_str(content)?;
    let lookup = |table: &str, key: &str| -> Option<toml::Value> {
        let mut v = &doc;
        for seg in table.split('.') {
            v = v.get(seg)?;
        }
        v.get(key).cloned()
    };
    let non_empty = |table: &str, key: &str| {
        lookup(table, key)
            .and_then(|v| v.as_array().map(|a| !a.is_empty()))
            .unwrap_or(false)
    };
    let with_reviewer = profile == Profile::AgentGuard
        && (non_empty("directives", "allowed_override_actors")
            || non_empty("gates.ratified-paths", "ratifiers")
            || lookup("directives", "require_approval").and_then(|v| v.as_bool()) == Some(true)
            || lookup("gates.ratified-paths", "enabled").and_then(|v| v.as_bool()) == Some(true));
    let mut missing = Vec::new();
    for key in profile.keys() {
        if !key.when.applies(with_reviewer) {
            continue;
        }
        let holds = match key.value {
            ProfileValue::Toml(v) => {
                let want: toml::Value = toml::from_str::<toml::Table>(&format!("v = {v}"))
                    .expect("a profile value is valid TOML")
                    .remove("v")
                    .expect("the key just written");
                lookup(key.table, key.key) == Some(want)
            }
            ProfileValue::Reviewers => non_empty(key.table, key.key),
            ProfileValue::AgentLogins => true,
        };
        if !holds {
            missing.push(format!("{}.{}", key.table, key.key));
        }
    }
    Ok(missing)
}

/// The profile table of `docs/CONFIGURATION.md`, rendered from [`Profile::keys`].
pub fn render_profiles_markdown() -> String {
    let mut out =
        String::from("| Profile | Key | Value | Written | Why |\n|---|---|---|---|---|\n");
    for profile in Profile::ALL {
        for key in profile.keys() {
            let value = match key.value {
                ProfileValue::Toml(v) => format!("`{v}`"),
                other => other.label().to_string(),
            };
            out.push_str(&format!(
                "| `{}` | `{}.{}` | {} | {} | {} |\n",
                profile.name(),
                key.table,
                key.key,
                value,
                key.when.label(),
                key.why
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_RUNNERS: &[TestRunner] = &[
        TestRunner::CargoNextest,
        TestRunner::Cargo,
        TestRunner::Pytest,
        TestRunner::Vitest,
        TestRunner::Jest,
        TestRunner::Go,
        TestRunner::Maven,
        TestRunner::Gradle,
    ];

    /// A suggested preset runs with the project's own build tool: its own command starts
    /// with that tool, or the suggestion carries a command that does.
    #[test]
    fn a_suggested_mutation_preset_runs_with_the_project_build_tool() {
        let tool = |runner: TestRunner| match runner {
            TestRunner::CargoNextest | TestRunner::Cargo => "cargo ",
            TestRunner::Pytest => "mutmut ",
            TestRunner::Vitest | TestRunner::Jest => "npx ",
            TestRunner::Maven => "mvn ",
            TestRunner::Gradle => "gradle ",
            TestRunner::Go => "go ",
        };
        for runner in ALL_RUNNERS {
            let Some(preset) = runner.mutation_preset() else {
                assert!(runner.mutation_snippet().is_none());
                continue;
            };
            let def = crate::guards::presets::resolve_preset(preset).expect("a real preset");
            let command = runner.mutation_command().unwrap_or(def.default_command);
            assert!(
                command.starts_with(tool(*runner)),
                "{runner:?}: `{preset}` would run `{command}`"
            );
            let snippet = runner.mutation_snippet().unwrap();
            assert_eq!(
                snippet.contains("# command = "),
                runner.mutation_command().is_some(),
                "{snippet}"
            );
        }
    }

    /// Only a preset whose command takes the diff is called diff-scoped.
    #[test]
    fn only_a_preset_whose_command_takes_the_diff_is_called_diff_scoped() {
        for runner in ALL_RUNNERS {
            let Some(preset) = runner.mutation_preset() else {
                continue;
            };
            let def = crate::guards::presets::resolve_preset(preset).unwrap();
            let takes_the_diff = def.default_command.contains("--in-diff");
            assert_eq!(
                runner.mutation_preset_is_diff_scoped(),
                takes_the_diff,
                "{preset}"
            );
            assert_eq!(
                runner.mutation_snippet().unwrap().contains("Diff-scoped"),
                takes_the_diff,
                "{preset}"
            );
        }
    }

    fn agent_guard(reviewers: &[&str], agents: &[&str]) -> Generated {
        let dir = tempfile::tempdir().unwrap();
        generate(
            "demo",
            dir.path(),
            Some(Profile::AgentGuard),
            &ProfileLogins {
                reviewers: reviewers.iter().map(|s| s.to_string()).collect(),
                agent_logins: agents.iter().map(|s| s.to_string()).collect(),
            },
        )
        .unwrap()
    }

    /// Every profile's file loads as a configuration, names its profile on its marker
    /// line, and sets every key of its variant.
    #[test]
    fn each_profile_writes_a_loadable_file_with_all_its_keys() {
        let dir = tempfile::tempdir().unwrap();
        let observe = generate(
            "demo",
            dir.path(),
            Some(Profile::Observe),
            &ProfileLogins::default(),
        )
        .unwrap();
        for (profile, generated) in [
            (Profile::Observe, observe),
            (Profile::AgentGuard, agent_guard(&[], &[])),
            (Profile::AgentGuard, agent_guard(&["owner"], &["agent-bot"])),
        ] {
            let cfg = crate::config::DisciplineConfig::from_toml_str(&generated.content)
                .unwrap_or_else(|e| panic!("{e}\n{}", generated.content));
            assert_eq!(profile_of(&generated.content), Some(profile));
            assert_eq!(
                missing_profile_keys(profile, &generated.content).unwrap(),
                Vec::<String>::new(),
                "{}",
                generated.content
            );
            if profile == Profile::Observe {
                assert_eq!(cfg.meta.mode, crate::config::RunMode::Advisory);
            }
        }
        // The starter carries no marker and leaves advisory mode commented out.
        let starter = generate_starter("demo", dir.path());
        assert_eq!(profile_of(&starter), None);
        assert!(starter.contains("# mode = \"advisory\""));
    }

    /// Without a reviewer the review-based keys are left out and the output says why;
    /// with one they are written, and `fail_on_overrides` is not (an approval could
    /// never lift it).
    #[test]
    fn agent_guard_writes_the_review_keys_only_with_a_reviewer() {
        let alone = agent_guard(&[], &[]);
        let cfg = crate::config::DisciplineConfig::from_toml_str(&alone.content).unwrap();
        assert!(cfg.directives.fail_on_overrides);
        assert!(!cfg.directives.require_approval);
        assert!(!cfg.gates.ratified_paths.enabled);
        assert!(cfg.gates.review_threads.enabled);
        assert_eq!(cfg.directives.sources, vec!["pr-body".to_string()]);
        assert_eq!(cfg.directives.max_overrides, Some(1));
        assert!(
            alone.notes.iter().any(|n| n.contains("#776")),
            "{:?}",
            alone.notes
        );

        let reviewed = agent_guard(&["owner"], &["agent-bot"]);
        let cfg = crate::config::DisciplineConfig::from_toml_str(&reviewed.content).unwrap();
        assert!(!cfg.directives.fail_on_overrides);
        assert!(cfg.directives.require_approval);
        assert_eq!(
            cfg.directives.allowed_override_actors,
            vec!["owner".to_string()]
        );
        let rp = &cfg.gates.ratified_paths;
        assert!(rp.enabled && rp.refuse_author_ratification);
        assert_eq!(rp.ratifiers, vec!["owner".to_string()]);
        assert_eq!(rp.agent_logins, vec!["agent-bot".to_string()]);
        assert!(rp.protected_paths.contains(&"discipline.toml".to_string()));
    }

    /// A reviewer who is also an agent login is not a second party: it is dropped, and
    /// with no other reviewer the file is the one written without a reviewer.
    #[test]
    fn a_reviewer_who_is_an_agent_login_is_not_used() {
        let generated = agent_guard(&["Agent-Bot"], &["agent-bot"]);
        assert_eq!(generated.content, agent_guard(&[], &["agent-bot"]).content);
        assert!(generated.notes.iter().any(|n| n.contains("`Agent-Bot`")));
    }

    #[test]
    fn a_login_that_is_not_one_or_a_login_without_agent_guard_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let logins = |r: &str| ProfileLogins {
            reviewers: vec![r.to_string()],
            agent_logins: vec![],
        };
        for bad in ["", "own er", "o\"wner", "a\nb"] {
            assert!(generate("d", dir.path(), Some(Profile::AgentGuard), &logins(bad)).is_err());
        }
        assert!(generate(
            "d",
            dir.path(),
            Some(Profile::AgentGuard),
            &logins("app[bot]")
        )
        .is_ok());
        assert!(generate("d", dir.path(), Some(Profile::Observe), &logins("owner")).is_err());
        assert!(generate("d", dir.path(), None, &logins("owner")).is_err());
    }

    /// Removing or changing any key a profile wrote is reported by name; a key the
    /// variant does not write is not asked for.
    #[test]
    fn a_removed_or_changed_profile_key_is_named() {
        let content = agent_guard(&["owner"], &[]).content;
        for (from, to, key) in [
            (
                "sources = [\"pr-body\"]",
                "sources = [\"pr-body\", \"commits\"]",
                "directives.sources",
            ),
            (
                "max_overrides = 1",
                "max_overrides = 3",
                "directives.max_overrides",
            ),
            ("require_approval = true", "", "directives.require_approval"),
            (
                "refuse_author_ratification = true",
                "",
                "gates.ratified-paths.refuse_author_ratification",
            ),
            (
                "ratifiers = [\"owner\"]",
                "ratifiers = []",
                "gates.ratified-paths.ratifiers",
            ),
        ] {
            let edited = content.replacen(from, to, 1);
            assert_ne!(edited, content, "{from}");
            assert_eq!(
                missing_profile_keys(Profile::AgentGuard, &edited).unwrap(),
                vec![key.to_string()],
                "{from}"
            );
        }
        let alone = agent_guard(&[], &[]).content;
        let edited = alone.replacen("fail_on_overrides = true", "fail_on_overrides = false", 1);
        assert_eq!(
            missing_profile_keys(Profile::AgentGuard, &edited).unwrap(),
            vec!["directives.fail_on_overrides".to_string()]
        );
        let edited = alone.replacen(
            "[gates.review-threads]\n",
            "[gates.review-threads-off]\n",
            1,
        );
        assert!(missing_profile_keys(Profile::AgentGuard, &edited)
            .unwrap()
            .contains(&"gates.review-threads.enabled".to_string()));
    }

    /// The marker is read from its own line only.
    #[test]
    fn the_profile_marker_is_read_from_its_own_line() {
        assert_eq!(
            profile_of("# Written by `discipline init --profile agent-guard`.\n"),
            Some(Profile::AgentGuard)
        );
        assert_eq!(
            profile_of("# Written by `discipline init --profile full`.\n"),
            None
        );
        assert_eq!(
            profile_of("x = 1 # Written by `discipline init --profile observe`.\n"),
            None
        );
    }

    /// The docs table lists every key of every profile.
    #[test]
    fn the_profile_table_lists_every_key() {
        let table = render_profiles_markdown();
        for profile in Profile::ALL {
            for key in profile.keys() {
                assert!(
                    table.contains(&format!(
                        "| `{}` | `{}.{}` |",
                        profile.name(),
                        key.table,
                        key.key
                    )),
                    "{table}"
                );
            }
        }
    }
}
