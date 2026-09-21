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

/// Render gates Markdown table for README.md or ROADMAP.md.
pub fn render_gates_markdown(gates: &[GateInfo], target_prefix: &str) -> String {
    let mut out =
        String::from("| Gate | Suite | Languages | Rule Description |\n|---|---|---|---|\n");
    for g in gates.iter().filter(|g| g.available) {
        out.push_str(&format!(
            "| [`{}`]({target_prefix}{}) | {} | {} | {} |\n",
            g.id,
            g.id,
            g.suite.label(),
            g.languages,
            g.summary
        ));
    }
    out
}

/// Render gates catalog Markdown table for GATES.md / ROADMAP.md.
pub fn render_gates_catalog_markdown(
    gates: &[GateInfo],
    suite_filter: Option<Suite>,
    in_gates_md: bool,
) -> String {
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
        let gate_link = if g.available {
            if in_gates_md {
                format!("[`{}`](#{})", g.id, g.id)
            } else {
                format!("[`{}`](GATES.md#{})", g.id, g.id)
            }
        } else {
            format!("`{}`", g.id)
        };
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            gate_link,
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
    let defaults = crate::config::DisciplineConfig::default_for_repo("");
    let mut out = String::new();
    for g in gates.iter().filter(|g| g.available) {
        // The badge is the compiled default, never a hand-typed label.
        let badge = match defaults.gates.settings(g.id) {
            Some(s) if !s.enabled() => "<span class=\"gate-badge badge-lang\">Off</span>",
            Some(s) => match s.severity() {
                crate::config::Severity::Error => {
                    "<span class=\"gate-badge badge-error\">Error</span>"
                }
                crate::config::Severity::Warning => {
                    "<span class=\"gate-badge badge-warn\">Warning</span>"
                }
                crate::config::Severity::Note => {
                    "<span class=\"gate-badge badge-lang\">Note</span>"
                }
            },
            None => "<span class=\"gate-badge badge-lang\">n/a</span>",
        };
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
            "          <tr>\n            <td class=\"gate-id\">{}</td>\n            <td>{}</td>\n            <td>{}</td>\n            <td><span class=\"gate-badge badge-lang\">{}</span></td>\n            <td>{}</td>\n          </tr>\n",
            g.id,
            suite_name,
            badge,
            lang_badge,
            html_escape(g.summary)
        ));
    }
    out.trim_end().to_string()
}

/// One row of the generated configuration reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigKeyRow {
    /// Dotted key path, e.g. `gates.pii.allowed_users` or `gates.command.commands[].name`.
    pub path: String,
    pub ty: String,
    pub default: String,
    pub description: String,
}

/// Enumerate every configuration key the JSON Schema accepts, in schema order.
///
/// The key set, types and descriptions come from [`generate_schema`] (the same
/// value `discipline schema` prints) and the defaults come from the compiled
/// `Default` implementations, so the reference table cannot drift from either.
pub fn config_key_rows() -> Vec<ConfigKeyRow> {
    let schema = generate_schema();
    let defaults = serde_json::to_value(crate::config::DisciplineConfig::default_for_repo(""))
        .unwrap_or(serde_json::Value::Null);
    let mut rows = Vec::new();
    collect_rows(&schema, &schema, "", Some(&defaults), &mut rows);
    rows
}

fn resolve_ref<'a>(
    root: &'a serde_json::Value,
    node: &'a serde_json::Value,
) -> &'a serde_json::Value {
    let reference = node
        .get("$ref")
        .or_else(|| {
            node.get("allOf")
                .and_then(|a| a.get(0))
                .and_then(|a| a.get("$ref"))
        })
        .and_then(|r| r.as_str());
    match reference.and_then(|r| r.strip_prefix("#/$defs/")) {
        Some(name) => root.get("$defs").and_then(|d| d.get(name)).unwrap_or(node),
        None => node,
    }
}

/// Walk `node`'s `properties`, emitting one row per leaf key. `defaults` is the
/// serialized compiled default at the same position, or `None` inside an array
/// item where no per-entry default exists.
fn collect_rows(
    root: &serde_json::Value,
    node: &serde_json::Value,
    prefix: &str,
    defaults: Option<&serde_json::Value>,
    rows: &mut Vec<ConfigKeyRow>,
) {
    let node = resolve_ref(root, node);
    let Some(props) = node.get("properties").and_then(|p| p.as_object()) else {
        return;
    };
    let required: Vec<&str> = node
        .get("required")
        .and_then(|r| r.as_array())
        .map(|r| r.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    for (key, prop) in props {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        let child_default = defaults.and_then(|d| d.get(key));
        let target = resolve_ref(root, prop);
        let is_table = (target.get("properties").is_some() && prop.get("$ref").is_none())
            || prop.get("allOf").is_some();
        if is_table {
            collect_rows(root, prop, &path, child_default, rows);
            continue;
        }
        let items = prop.get("items");
        if let Some(items) = items.filter(|i| resolve_ref(root, i).get("properties").is_some()) {
            rows.push(ConfigKeyRow {
                path: path.clone(),
                ty: "array of tables".to_string(),
                default: format_default(prop, required.contains(&key.as_str()), child_default),
                description: describe(root, key, prop),
            });
            collect_rows(root, items, &format!("{path}[]"), None, rows);
            continue;
        }
        rows.push(ConfigKeyRow {
            path,
            ty: type_label(prop),
            default: format_default(prop, required.contains(&key.as_str()), child_default),
            description: describe(root, key, prop),
        });
    }
}

fn type_label(prop: &serde_json::Value) -> String {
    match prop.get("$ref").and_then(|r| r.as_str()) {
        Some("#/$defs/StringListOrReset") => return "list".to_string(),
        Some("#/$defs/Severity") => return "string".to_string(),
        _ => {}
    }
    match prop.get("type") {
        Some(serde_json::Value::String(t)) if t == "array" => "list".to_string(),
        Some(serde_json::Value::String(t)) => t.clone(),
        Some(serde_json::Value::Array(ts)) => ts
            .iter()
            .filter_map(|t| t.as_str())
            .collect::<Vec<_>>()
            .join(" or "),
        _ => "value".to_string(),
    }
}

fn describe(root: &serde_json::Value, key: &str, prop: &serde_json::Value) -> String {
    let own = prop.get("description").and_then(|d| d.as_str());
    let text = match own {
        Some(d) => d.to_string(),
        None => match key {
            "severity" => resolve_ref(root, prop)
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or_default()
                .to_string(),
            "exempt_paths" => "File path globs exempted from this gate".to_string(),
            _ => String::new(),
        },
    };
    table_cell(&text)
}

fn format_default(
    prop: &serde_json::Value,
    required: bool,
    value: Option<&serde_json::Value>,
) -> String {
    if let Some(c) = prop.get("const") {
        return format!("`{c}`");
    }
    if required {
        return "*(required)*".to_string();
    }
    let Some(value) = value else {
        return "*(per entry)*".to_string();
    };
    match value {
        serde_json::Value::Null => "*(unset)*".to_string(),
        serde_json::Value::Array(items) if !items.is_empty() => {
            let compact = value.to_string();
            if compact.len() <= 48 {
                code_cell(&compact)
            } else {
                format!("*({} entries)*", items.len())
            }
        }
        other => code_cell(&other.to_string()),
    }
}

fn code_cell(s: &str) -> String {
    let fence = if s.contains('`') { "``" } else { "`" };
    table_cell(&format!("{fence}{s}{fence}"))
}

fn table_cell(s: &str) -> String {
    s.replace('|', "\\|")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\n', " ")
}

/// Render configuration schema Markdown table.
pub fn render_config_schema_markdown() -> String {
    let mut out =
        String::from("| Section / Key | Type | Default | Description |\n|---|---|---|---|\n");
    for row in config_key_rows() {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            row.path, row.ty, row.default, row.description
        ));
    }
    out
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

            let is_gates_md = path.file_name().and_then(|f| f.to_str()) == Some("GATES.md");
            let in_docs = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|f| f.to_str())
                == Some("docs");
            let target_prefix = if in_docs {
                "GATES.md#"
            } else {
                "docs/GATES.md#"
            };
            // Generate content
            let generated_text = match marker_name {
                "gates" => {
                    if is_html {
                        render_gates_html(gates)
                    } else if is_gates_md {
                        render_gates_catalog_markdown(gates, None, true)
                    } else {
                        render_gates_markdown(gates, target_prefix)
                    }
                }
                "gates:agent-guard" => {
                    render_gates_catalog_markdown(gates, Some(Suite::AgentGuard), is_gates_md)
                }
                "gates:hygiene" => {
                    render_gates_catalog_markdown(gates, Some(Suite::Hygiene), is_gates_md)
                }
                "gates:integrity" => {
                    render_gates_catalog_markdown(gates, Some(Suite::Integrity), is_gates_md)
                }
                "gates:quality" => {
                    render_gates_catalog_markdown(gates, Some(Suite::Quality), is_gates_md)
                }
                "gates:verification" => {
                    render_gates_catalog_markdown(gates, Some(Suite::Verification), is_gates_md)
                }
                "gates:bench" => {
                    render_gates_catalog_markdown(gates, Some(Suite::Bench), is_gates_md)
                }
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

    // 3. Process man/man1/discipline.1
    let man1_dir = root.join("man/man1");
    let man1_path = man1_dir.join("discipline.1");
    let generated_man1_str = generate_man1()?;

    let existing_man1_str = if man1_path.exists() {
        std::fs::read_to_string(&man1_path)?
    } else {
        String::new()
    };

    if existing_man1_str != generated_man1_str {
        has_diffs = true;
        let diff = unified_diff(&man1_path, &existing_man1_str, &generated_man1_str);
        eprintln!("{diff}");

        if write {
            if !man1_dir.exists() {
                std::fs::create_dir_all(&man1_dir)?;
            }
            std::fs::write(&man1_path, &generated_man1_str)
                .with_context(|| format!("failed to write {}", man1_path.display()))?;
            println!("Updated {}", man1_path.display());
        }
    }

    if has_diffs {
        if write {
            println!("Reference docs, schemas, and man pages written successfully.");
            Ok(true)
        } else {
            eprintln!("Error: reference documentation, schemas, or man pages are out of date.");
            eprintln!("Run 'cargo run -- docs --write' to update generated reference docs.");
            Ok(false)
        }
    } else {
        println!("All reference documentation and schemas are up to date.");
        Ok(true)
    }
}

/// Generate man1 page for discipline CLI.
pub fn generate_man1() -> Result<String> {
    let mut buf = Vec::new();
    clap_mangen::Man::new(<crate::cli::Cli as clap::CommandFactory>::command()).render(&mut buf)?;
    Ok(String::from_utf8(buf)?)
}
