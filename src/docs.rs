//! Reference documentation and JSON Schema generator and verification sentinel.
//!
//! Provides `discipline docs --write | --check` to keep reference tables across
//! `README.md`, `docs/index.html`, and documentation files synchronized with
//! the canonical sources of truth (`action.yml`, `src/config.rs::GATES`, `src/schema.rs`, and clap).

use anyhow::{bail, Context, Result};
use clap::CommandFactory;
use std::collections::HashSet;
use std::path::Path;

use crate::config::{GateInfo, Suite, GATES};
use crate::schema::generate_schema;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionInput {
    pub name: String,
    pub description: String,
    pub required: bool,
    pub default: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionOutput {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ActionSpec {
    pub inputs: Vec<ActionInput>,
    pub outputs: Vec<ActionOutput>,
}

/// Parse `action.yml` inputs and outputs without third-party YAML dependencies.
pub fn parse_action_yml(content: &str) -> Result<ActionSpec> {
    let mut spec = ActionSpec::default();
    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        if trimmed == "inputs:" {
            i += 1;
            while i < lines.len() {
                let iline = lines[i];
                if !iline.starts_with("  ") || iline.starts_with("    ") {
                    if !iline.trim().is_empty() && !iline.starts_with('#') {
                        break;
                    }
                    i += 1;
                    continue;
                }
                // An input name at 2 spaces indentation
                let name = iline.trim().trim_end_matches(':').to_string();
                let mut desc = String::new();
                let mut req = false;
                let mut default_val = String::new();
                i += 1;

                while i < lines.len() {
                    let prop_line = lines[i];
                    if !prop_line.starts_with("    ") {
                        break;
                    }
                    let prop_trimmed = prop_line.trim();
                    if let Some(rest) = prop_trimmed.strip_prefix("description:") {
                        desc = unquote(rest.trim());
                    } else if let Some(rest) = prop_trimmed.strip_prefix("required:") {
                        req = rest.trim() == "true";
                    } else if let Some(rest) = prop_trimmed.strip_prefix("default:") {
                        default_val = unquote(rest.trim());
                    }
                    i += 1;
                }

                spec.inputs.push(ActionInput {
                    name,
                    description: desc,
                    required: req,
                    default: default_val,
                });
            }
        } else if trimmed == "outputs:" {
            i += 1;
            while i < lines.len() {
                let oline = lines[i];
                if !oline.starts_with("  ") || oline.starts_with("    ") {
                    if !oline.trim().is_empty() && !oline.starts_with('#') {
                        break;
                    }
                    i += 1;
                    continue;
                }
                let name = oline.trim().trim_end_matches(':').to_string();
                let mut desc = String::new();
                i += 1;

                while i < lines.len() {
                    let prop_line = lines[i];
                    if !prop_line.starts_with("    ") {
                        break;
                    }
                    let prop_trimmed = prop_line.trim();
                    if let Some(rest) = prop_trimmed.strip_prefix("description:") {
                        desc = unquote(rest.trim());
                    }
                    i += 1;
                }

                spec.outputs.push(ActionOutput {
                    name,
                    description: desc,
                });
            }
        } else {
            i += 1;
        }
    }

    Ok(spec)
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2
        && ((t.starts_with('\'') && t.ends_with('\'')) || (t.starts_with('"') && t.ends_with('"')))
    {
        return t[1..t.len() - 1].to_string();
    }
    t.to_string()
}

/// Render action inputs as a Markdown table.
pub fn render_action_inputs_markdown(spec: &ActionSpec) -> String {
    let mut out = String::from("| Input | Default | Description |\n|---|---|---|\n");
    for inp in &spec.inputs {
        let def = if inp.default.is_empty() {
            "*(none)*".to_string()
        } else {
            format!("`{}`", inp.default)
        };
        out.push_str(&format!(
            "| `{}` | {} | {} |\n",
            inp.name, def, inp.description
        ));
    }
    out
}

/// Render action outputs as a Markdown table.
pub fn render_action_outputs_markdown(spec: &ActionSpec) -> String {
    let mut out = String::from("| Output | Description |\n|---|---|\n");
    for outp in &spec.outputs {
        out.push_str(&format!("| `{}` | {} |\n", outp.name, outp.description));
    }
    out
}

/// Render action inputs as HTML table rows.
pub fn render_action_inputs_html(spec: &ActionSpec) -> String {
    let mut out = String::from("<table class=\"gate-table\">\n  <thead>\n    <tr>\n      <th>Input</th>\n      <th>Default</th>\n      <th>Description</th>\n    </tr>\n  </thead>\n  <tbody>\n");
    for inp in &spec.inputs {
        let def = if inp.default.is_empty() {
            "<em>(none)</em>".to_string()
        } else {
            format!("<code>{}</code>", html_escape(&inp.default))
        };
        out.push_str(&format!(
            "    <tr>\n      <td class=\"gate-id\">{}</td>\n      <td>{}</td>\n      <td>{}</td>\n    </tr>\n",
            html_escape(&inp.name),
            def,
            html_escape(&inp.description)
        ));
    }
    out.push_str("  </tbody>\n</table>");
    out
}

/// Render action outputs as HTML table rows.
pub fn render_action_outputs_html(spec: &ActionSpec) -> String {
    let mut out = String::from("<table class=\"gate-table\">\n  <thead>\n    <tr>\n      <th>Output</th>\n      <th>Description</th>\n    </tr>\n  </thead>\n  <tbody>\n");
    for outp in &spec.outputs {
        out.push_str(&format!(
            "    <tr>\n      <td class=\"gate-id\">{}</td>\n      <td>{}</td>\n    </tr>\n",
            html_escape(&outp.name),
            html_escape(&outp.description)
        ));
    }
    out.push_str("  </tbody>\n</table>");
    out
}

/// Render gates Markdown table for README.md.
pub fn render_gates_markdown(gates: &[GateInfo]) -> String {
    let mut out =
        String::from("| Gate | Suite | Languages | Rule Description |\n|---|---|---|---|\n");
    for g in gates.iter().filter(|g| g.available) {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            g.id,
            g.suite.label(),
            g.languages,
            g.summary
        ));
    }
    out
}

/// Render gates catalog Markdown table for GATES.md / ROADMAP.md.
pub fn render_gates_catalog_markdown(gates: &[GateInfo], suite_filter: Option<Suite>) -> String {
    let mut out = String::from(
        "| Gate id | Suite | Status | Languages | Rule Description |\n|---|---|---|---|---|\n",
    );
    let iter = gates.iter().filter(|g| match suite_filter {
        Some(s) => g.suite == s,
        None => true,
    });
    for g in iter {
        let status = if g.available {
            "**shipped**"
        } else {
            "planned"
        };
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} |\n",
            g.id,
            g.suite.label(),
            status,
            g.languages,
            g.summary
        ));
    }
    out
}

/// Render gates HTML table rows for `docs/index.html`.
pub fn render_gates_html(gates: &[GateInfo]) -> String {
    let mut out = String::new();
    for g in gates.iter().filter(|g| g.available) {
        let suite_name = match g.suite {
            Suite::AgentGuard => "Agent Guard",
            Suite::Hygiene => "Hygiene",
            Suite::Integrity => "Integrity",
            Suite::Quality => "Quality",
            Suite::Verification => "Verification",
            Suite::Bench => "Performance",
        };
        let lang_badge = match g.languages {
            "any" => "Any",
            other => other,
        };
        out.push_str(&format!(
            "          <tr>\n            <td class=\"gate-id\">{}</td>\n            <td>{}</td>\n            <td><span class=\"gate-badge badge-error\">Error</span></td>\n            <td><span class=\"gate-badge badge-lang\">{}</span></td>\n            <td>{}</td>\n          </tr>\n",
            g.id,
            suite_name,
            lang_badge,
            html_escape(g.summary)
        ));
    }
    out.trim_end().to_string()
}

/// Render configuration schema Markdown table.
pub fn render_config_schema_markdown() -> String {
    String::from(
        "| Section / Key | Type | Default | Description |\n|---|---|---|---|\n\
| `meta.version` | integer | `1` | Configuration schema version (must be 1) |\n\
| `meta.name` | string | `\"\"` | Repository or project name |\n\
| `meta.description` | string | `\"\"` | Optional description of the project |\n\
| `directives.sources` | list | `[\"pr-body\", \"commits\"]` | Allowed directive source channels |\n\
| `directives.allow_hidden` | boolean | `false` | Allow directives inside HTML comments `<!-- -->` |\n\
| `directives.fail_on_overrides` | boolean | `false` | Treat applied overrides as failures requiring human sign-off |\n\
| `gates.<id>.enabled` | boolean | `true` | Whether this gate is active |\n\
| `gates.<id>.severity` | string | `\"error\"` | Violation severity: `\"error\"` (blocking) or `\"warning\"` (non-blocking) |\n\
| `gates.<id>.exempt_paths` | list | `[]` | File path globs exempted from gate evaluation |\n\
| `gates.assertion-reduction.extra_assert_macros` | list | `[]` | Additional macro names treated as assertions |\n\
| `gates.assertion-reduction.assert_helper_fns` | list | `[]` | Additional function names treated as assertions |\n\
| `gates.vacuous-tests.extra_assert_macros` | list | `[]` | Additional macro names treated as assertions |\n\
| `gates.vacuous-tests.assert_helper_fns` | list | `[]` | Additional function names treated as assertions |\n\
| `gates.unsafe-safety-comment.placeholders` | list | `[]` | Additional placeholder phrases to reject in SAFETY comments |\n\
| `gates.deletion-rationale.paths` | list | `[\"**\"]` | Path globs where file deletions require a rationale |\n\
| `gates.time-estimates.include` | list | `[\"**/*.md\"]` | Markdown file globs swept for duration estimates |\n\
| `gates.time-estimates.extra_patterns` | list | `[]` | Additional custom banned regex patterns |\n\
| `gates.time-estimates.allow_patterns` | list | `[...]` | Regex patterns permitted as operational exceptions |\n\
| `gates.time-estimates.scan_pr_body` | boolean | `true` | Whether to scan the PR description text |\n\
| `gates.pii.home_paths` | boolean | `true` | Check for leaked workstation home directory paths |\n\
| `gates.pii.lan_ips` | boolean | `true` | Check for leaked private RFC 1918 LAN IP addresses |\n\
| `gates.pii.allowed_users` | list | `[...]` | Allowed username tokens in paths |\n\
| `gates.pii.hostname_denylist` | list | `[]` | Whole-token case-insensitive hostnames to reject |\n\
| `gates.pii.extra_patterns` | list | `[]` | Additional regex patterns to reject |\n\
| `gates.pii.allow_patterns` | list | `[]` | Custom regex patterns exempted from rejection |\n\
| `gates.pii.scan_pr_body` | boolean | `true` | Whether to scan the PR description text |\n\
| `gates.agent-scratch.paths` | list | `[...]` | Directory and file globs forbidden from being tracked |\n\
| `gates.golden-output.paths` | list | `[...]` | Committed golden/snapshot globs requiring override to edit |\n\
| `gates.bench-regression.tolerance_pct` | number | `0.5` | Maximum allowed benchmark regression percentage |\n\
| `gates.bench-regression.paths` | list | `[...]` | Benchmark artifact globs tracked across revisions |\n\
| `gates.bench-regression.provenance` | string | `\"\"` | Expected host or runner provenance tag for benchmark artifacts |\n\
| `gates.bench-regression.allow_cross_host` | boolean | `false` | Allow benchmark comparison across mismatched provenance tags |\n",
    )
}

/// Render CLI reference Markdown from clap definition.
pub fn render_cli_markdown() -> String {
    let cmd = crate::cli::Cli::command();
    let mut out = String::from("| Subcommand | Description |\n|---|---|\n");
    for sub in cmd.get_subcommands() {
        if sub.is_hide_set() {
            continue;
        }
        let about = sub.get_about().map(|s| s.to_string()).unwrap_or_default();
        out.push_str(&format!("| `{}` | {} |\n", sub.get_name(), about));
    }
    out
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Replace regions between `<!-- generated:<name> -->` and `<!-- /generated -->`.
pub fn update_generated_regions(
    path: &Path,
    content: &str,
    spec: &ActionSpec,
    gates: &[GateInfo],
) -> Result<String> {
    let is_html = path.extension().and_then(|e| e.to_str()) == Some("html");
    let mut lines = content.lines().peekable();
    let mut out = Vec::new();
    let mut seen_markers = HashSet::new();

    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed.starts_with("<!-- generated:") && trimmed.ends_with("-->") {
            let marker_name = trimmed
                .strip_prefix("<!-- generated:")
                .and_then(|s| s.strip_suffix("-->"))
                .map(|s| s.trim())
                .unwrap_or("");

            if marker_name.is_empty() {
                bail!("malformed generated marker in {}", path.display());
            }

            if !seen_markers.insert(marker_name.to_string()) {
                bail!(
                    "duplicate generated marker '{}' in {}",
                    marker_name,
                    path.display()
                );
            }

            // Write opening marker
            out.push(line.to_string());

            // Generate content
            let generated_text = match marker_name {
                "gates" => {
                    if is_html {
                        render_gates_html(gates)
                    } else if path.file_name().and_then(|f| f.to_str()) == Some("GATES.md") {
                        render_gates_catalog_markdown(gates, None)
                    } else {
                        render_gates_markdown(gates)
                    }
                }
                "gates:agent-guard" => {
                    render_gates_catalog_markdown(gates, Some(Suite::AgentGuard))
                }
                "gates:hygiene" => render_gates_catalog_markdown(gates, Some(Suite::Hygiene)),
                "gates:integrity" => render_gates_catalog_markdown(gates, Some(Suite::Integrity)),
                "gates:quality" => render_gates_catalog_markdown(gates, Some(Suite::Quality)),
                "gates:verification" => {
                    render_gates_catalog_markdown(gates, Some(Suite::Verification))
                }
                "gates:bench" => render_gates_catalog_markdown(gates, Some(Suite::Bench)),
                "action-inputs" => {
                    if is_html {
                        render_action_inputs_html(spec)
                    } else {
                        render_action_inputs_markdown(spec)
                    }
                }
                "action-outputs" => {
                    if is_html {
                        render_action_outputs_html(spec)
                    } else {
                        render_action_outputs_markdown(spec)
                    }
                }
                "config-schema" | "schema" => render_config_schema_markdown(),
                "cli" => render_cli_markdown(),
                other => bail!(
                    "unknown generated marker target '{}' in {}",
                    other,
                    path.display()
                ),
            };

            for gline in generated_text.lines() {
                out.push(gline.to_string());
            }

            // Skip existing content until `<!-- /generated -->`
            let mut found_closing = false;
            for skip_line in lines.by_ref() {
                if skip_line.trim() == "<!-- /generated -->" {
                    out.push(skip_line.to_string());
                    found_closing = true;
                    break;
                }
            }

            if !found_closing {
                bail!(
                    "unclosed generated marker '{}' in {}",
                    marker_name,
                    path.display()
                );
            }
        } else if trimmed == "<!-- /generated -->" {
            bail!(
                "unexpected closing marker '<!-- /generated -->' without opening marker in {}",
                path.display()
            );
        } else {
            out.push(line.to_string());
        }
    }

    let mut result = out.join("\n");
    if content.ends_with('\n') {
        result.push('\n');
    }
    Ok(result)
}

/// Compute a unified diff between old and new text.
pub fn unified_diff(path: &Path, old: &str, new: &str) -> String {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let mut diff = String::new();

    diff.push_str(&format!("--- {} (committed)\n", path.display()));
    diff.push_str(&format!("+++ {} (generated)\n", path.display()));

    let mut i = 0;
    let mut j = 0;

    while i < old_lines.len() || j < new_lines.len() {
        if i < old_lines.len() && j < new_lines.len() && old_lines[i] == new_lines[j] {
            i += 1;
            j += 1;
        } else {
            let start_i = i;
            let start_j = j;
            let mut chunk_old = Vec::new();
            let mut chunk_new = Vec::new();

            while i < old_lines.len() && (j >= new_lines.len() || old_lines[i] != new_lines[j]) {
                chunk_old.push(old_lines[i]);
                i += 1;
            }
            while j < new_lines.len()
                && (i >= old_lines.len() || old_lines.get(i) != new_lines.get(j))
            {
                chunk_new.push(new_lines[j]);
                j += 1;
            }

            diff.push_str(&format!(
                "@@ -{},{} +{},{} @@\n",
                start_i + 1,
                chunk_old.len(),
                start_j + 1,
                chunk_new.len()
            ));
            for l in chunk_old {
                diff.push_str(&format!("-{l}\n"));
            }
            for l in chunk_new {
                diff.push_str(&format!("+{l}\n"));
            }
        }
    }

    diff
}

/// Execute docs check or write across repository files.
pub fn run_docs_check_or_write(root: &Path, write: bool) -> Result<bool> {
    let action_path = root.join("action.yml");
    let action_content = std::fs::read_to_string(&action_path)
        .with_context(|| format!("failed to read {}", action_path.display()))?;
    let action_spec = parse_action_yml(&action_content)?;

    let candidate_files = [
        root.join("README.md"),
        root.join("docs/index.html"),
        root.join("docs/GATES.md"),
        root.join("docs/CONFIGURATION.md"),
        root.join("docs/ROADMAP.md"),
    ];

    let mut has_diffs = false;

    // 1. Process Markdown and HTML files containing markers
    for file_path in &candidate_files {
        if !file_path.exists() {
            continue;
        }
        let original = std::fs::read_to_string(file_path)
            .with_context(|| format!("failed to read {}", file_path.display()))?;
        let updated = update_generated_regions(file_path, &original, &action_spec, GATES)?;

        if original != updated {
            has_diffs = true;
            let diff = unified_diff(file_path, &original, &updated);
            eprintln!("{diff}");

            if write {
                std::fs::write(file_path, &updated)
                    .with_context(|| format!("failed to write {}", file_path.display()))?;
                println!("Updated {}", file_path.display());
            }
        }
    }

    // 2. Process discipline.schema.json
    let schema_path = root.join("discipline.schema.json");
    let generated_schema_val = generate_schema();
    let generated_schema_str = serde_json::to_string_pretty(&generated_schema_val)? + "\n";

    let existing_schema_str = if schema_path.exists() {
        std::fs::read_to_string(&schema_path)?
    } else {
        String::new()
    };

    if existing_schema_str != generated_schema_str {
        has_diffs = true;
        let diff = unified_diff(&schema_path, &existing_schema_str, &generated_schema_str);
        eprintln!("{diff}");

        if write {
            std::fs::write(&schema_path, &generated_schema_str)
                .with_context(|| format!("failed to write {}", schema_path.display()))?;
            println!("Updated {}", schema_path.display());
        }
    }

    if has_diffs {
        if write {
            println!("Reference docs and schemas written successfully.");
            Ok(true)
        } else {
            eprintln!("Error: reference documentation is out of date.");
            eprintln!("Run 'cargo run -- docs --write' to update generated reference docs.");
            Ok(false)
        }
    } else {
        println!("All reference documentation and schemas are up to date.");
        Ok(true)
    }
}
