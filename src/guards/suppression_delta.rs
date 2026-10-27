//! Suppression delta sentinel (`suppression-delta`).
//!
//! A change must not add compiler, type-checker or linter suppressions (`#[allow]`,
//! `@ts-ignore`, `# noqa`, `NOLINT`, `@SuppressWarnings`, ...) without saying why.
//!
//! The sites come from the language packs (`ParsedFileFacts::escape_hatches`), so a
//! marker inside a string or an ordinary comment is not one, and the count is a delta:
//! the head side of each changed file is compared with its base side, and a site that
//! merely moved is not new. `unsafe` sites are left to `unsafe-safety-comment` and
//! `unsafe-budget`.

use crate::ast::{default_registry, EscapeHatchSite};
use crate::guards::{line_allows, Context, GateOutcome, PathFilter};
use crate::tokens::ALLOW_SUPPRESSION;
use anyhow::Result;
use std::collections::HashMap;

pub const GATE: &str = "suppression-delta";

/// A suppression site as this gate sees it: what kind, which rule, and the text.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Site {
    pub line: usize,
    /// `type-ignore` or `linter-disable`.
    pub kind: &'static str,
    pub rule: String,
    pub snippet: String,
}

impl Site {
    /// Identity across a move: kind, rule and whitespace-normalised text, not the line.
    fn signature(&self) -> (&'static str, String, String) {
        (
            self.kind,
            self.rule.trim().to_string(),
            self.snippet
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        )
    }
}

pub fn sites_of(hatches: &[EscapeHatchSite]) -> Vec<Site> {
    hatches
        .iter()
        .filter_map(|h| match h {
            EscapeHatchSite::TypeIgnore {
                line,
                tool,
                snippet,
            } => Some(Site {
                line: *line,
                kind: "type-ignore",
                rule: tool.clone(),
                snippet: snippet.clone(),
            }),
            EscapeHatchSite::LinterDisable {
                line,
                rule,
                snippet,
            } => Some(Site {
                line: *line,
                kind: "linter-disable",
                rule: rule.clone(),
                snippet: snippet.clone(),
            }),
            EscapeHatchSite::UnsafeBlock { .. } => None,
        })
        .collect()
}

/// Sites on the head side that the base side does not have (as a multiset). A site the
/// base had at another line, or the same site repeated as often as before, is not new.
pub fn new_sites(base: &[Site], head: &[Site]) -> Vec<Site> {
    let mut budget: HashMap<_, usize> = HashMap::new();
    for s in base {
        *budget.entry(s.signature()).or_default() += 1;
    }
    let mut new = Vec::new();
    for s in head {
        match budget.get_mut(&s.signature()) {
            Some(n) if *n > 0 => *n -= 1,
            _ => new.push(s.clone()),
        }
    }
    new
}

/// The pattern name `extract_suppression_rules` keys on, from the site's text.
fn pattern_of(site: &Site) -> String {
    let t = site.snippet.trim();
    let starts = |p: &str| t.starts_with(p);
    if starts("#[allow(") || starts("#![allow(") {
        "#[allow(".into()
    } else if starts("#[expect(") || starts("#![expect(") {
        "#[expect(".into()
    } else if starts("#pragma GCC diagnostic") {
        "#pragma GCC diagnostic ignored".into()
    } else if starts("#pragma clang diagnostic") {
        "#pragma clang diagnostic ignored".into()
    } else if starts("#pragma warning(") || starts("#pragma warning (") {
        "#pragma warning(disable)".into()
    } else if t.contains("# noqa") {
        "# noqa".into()
    } else if t.contains("type: ignore") {
        "# type: ignore".into()
    } else if t.contains("pylint: disable") {
        "# pylint: disable".into()
    } else if t.contains("@ts-nocheck") {
        "// @ts-nocheck".into()
    } else if t.contains("@ts-expect-error") {
        "// @ts-expect-error".into()
    } else if t.contains("@ts-ignore") {
        "// @ts-ignore".into()
    } else if t.contains("eslint-disable") {
        "eslint-disable".into()
    } else if t.contains("NOLINT") {
        "// NOLINT".into()
    } else if t.contains("nolint") || t.contains("lint:ignore") {
        "//nolint".into()
    } else if t.contains("pragma warning disable") {
        "#pragma warning disable".into()
    } else {
        site.rule.clone()
    }
}

pub fn evaluate_suppression_delta(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.suppression_delta;
    let mut out = GateOutcome::new(GATE);

    let changed = ctx.git.changed_files()?;
    if changed.is_empty() {
        return Ok(out);
    }

    // A malformed glob is a configuration error (exit 2), never a silently
    // skipped exemption.
    let exempt = PathFilter::new(&settings.exempt_paths)?;

    let registry = default_registry();
    let vocab = super::agent_diff::assert_vocabulary(ctx.config);
    // (path, site, pattern name)
    let mut detected: Vec<(String, Site, String)> = Vec::new();

    for file in &changed {
        if file.is_deleted() || exempt.matches(&file.path) {
            continue;
        }
        let Some(pack) = registry.find_pack(&file.path) else {
            continue;
        };
        let Some(head_src) = ctx.git.head_content(&file.path)? else {
            continue;
        };
        let head_facts = match pack.extract(&file.path, &head_src, &vocab) {
            Ok(f) => f,
            Err(e) => {
                out.notes.push(format!(
                    "`{}`: not analysed, the head side does not parse ({e})",
                    file.path
                ));
                continue;
            }
        };
        let base_sites = match ctx.git.base_content(&file.old_path)? {
            Some(base_src) => match pack.extract(&file.old_path, &base_src, &vocab) {
                Ok(f) => sites_of(&f.escape_hatches),
                Err(_) => {
                    out.notes.push(format!(
                        "`{}`: the base side does not parse; every head-side suppression is judged as new",
                        file.path
                    ));
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        let head_sites = sites_of(&head_facts.escape_hatches);
        out.examined += head_sites.len();

        for site in new_sites(&base_sites, &head_sites) {
            let line_text = head_src
                .lines()
                .nth(site.line.saturating_sub(1))
                .unwrap_or("");
            if line_allows(line_text, GATE) {
                out.overrides.push(crate::tokens::OverrideRecord {
                    gate: GATE.to_string(),
                    code: Some(crate::findings::full_code(
                        GATE,
                        &crate::findings::SUPPRESSION_ADDED,
                    )),
                    subject: format!("{}:{}", file.path, site.line),
                    directive: format!("discipline:allow({GATE})"),
                    reason: "inline exemption marker".to_string(),
                    source: crate::tokens::OverrideSource::Inline {
                        file: file.path.clone(),
                        line: site.line,
                    },
                    hidden: true,
                    already_in_tree: false,
                });
                continue;
            }
            if settings
                .allowed_suppressions
                .iter()
                .any(|a| site.snippet.contains(a.as_str()))
            {
                continue;
            }
            let pat = pattern_of(&site);
            detected.push((file.path.clone(), site, pat));
        }
    }

    let mut unwaived = Vec::new();
    for (path, site, pat) in &detected {
        let mut candidate_subjects = extract_suppression_rules(&site.snippet, pat);
        if !site.rule.is_empty() {
            candidate_subjects.push(site.rule.clone());
        }
        candidate_subjects.push(path.clone());
        if let Some(file_name) = path.rsplit('/').next() {
            if file_name != path {
                candidate_subjects.push(file_name.to_string());
            }
        }

        let mut applied: Option<(crate::tokens::OverrideRecord, String)> = None;
        for subj in &candidate_subjects {
            if let Some(ov) = ctx.find_override(
                GATE,
                &crate::findings::SUPPRESSION_ADDED,
                ALLOW_SUPPRESSION,
                subj,
            ) {
                applied = Some((ov, subj.clone()));
                break;
            }
        }

        if let Some((ov, matched_subj)) = applied {
            out.overrides.push(ov.clone());
            out.notes.push(format!(
                "override applied: `{}: {}` for suppression `{}` in `{}:{}` ({})",
                ov.directive, ov.reason, matched_subj, path, site.line, ov.source
            ));
        } else {
            unwaived.push((path, site, pat));
        }
    }

    if unwaived.len() > settings.max_increase {
        for (path, site, pat) in &unwaived {
            let kind_label = suppression_kind_label(pat, site);
            let remediation = if settings.quote_line {
                let sanitized_snippet = crate::report::scrub_override_directives(&site.snippet);
                format!(
                    "line contains suppression annotation `{}`: `{}`; use `discipline:allow(suppression-delta): <rule-or-path> <reason>` to waive",
                    kind_label, sanitized_snippet
                )
            } else {
                format!(
                    "line contains suppression annotation `{}`; use `discipline:allow(suppression-delta): <rule-or-path> <reason>` to waive",
                    kind_label
                )
            };
            out.add_violation(
                ctx.overridable(settings.severity),
                &crate::findings::SUPPRESSION_ADDED,
                path,
                site.line,
                format!(
                    "new {} suppression `{kind_label}` introduced without override",
                    site.kind
                ),
                remediation,
            );
        }
    }

    Ok(out)
}

/// The suppression patterns [`pattern_of`] names by a fixed string. Any other pattern is
/// the site's own rule text, which a label never prints as it stands.
const KNOWN_PATTERNS: &[&str] = &[
    "#[allow(",
    "#[expect(",
    "#pragma GCC diagnostic ignored",
    "#pragma clang diagnostic ignored",
    "#pragma warning(disable)",
    "# noqa",
    "# type: ignore",
    "# pylint: disable",
    "// @ts-nocheck",
    "// @ts-expect-error",
    "// @ts-ignore",
    "eslint-disable",
    "// NOLINT",
    "//nolint",
    "#pragma warning disable",
];

/// The tool a site belongs to, named by a fixed string chosen from the site's pattern or
/// from the marker words of its text: never the text itself.
fn tool_label(pat: &str, site: &Site) -> &'static str {
    if let Some(known) = KNOWN_PATTERNS.iter().find(|k| **k == pat) {
        return known;
    }
    const MARKERS: &[(&str, &str)] = &[
        ("SuppressWarnings", "SuppressWarnings"),
        ("SuppressMessage", "SuppressMessage"),
        ("SuppressLint", "SuppressLint"),
        ("Suppress", "Suppress"),
        ("nowarn", "nowarn"),
        ("rubocop", "rubocop:disable"),
        ("swiftlint", "swiftlint:disable"),
        ("scalastyle", "scalastyle:off"),
        ("scalafix", "scalafix:off"),
        ("noinspection", "noinspection"),
        ("NOSONAR", "NOSONAR"),
        ("nosonar", "NOSONAR"),
        ("@unchecked", "@unchecked"),
    ];
    if site.rule == "coverage" {
        return "coverage ignore";
    }
    MARKERS
        .iter()
        .find(|(word, _)| site.snippet.contains(word))
        .map_or(site.kind, |(_, label)| label)
}

/// Whether `token` is spelled as a rule identifier: letters, digits and `_@/.:-`, with at
/// least one letter or digit (`clippy::all`, `no-console`, `Style/Foo`, `E501`).
fn is_rule_identifier(token: &str) -> bool {
    token.chars().any(|c| c.is_ascii_alphanumeric())
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_@/.:-".contains(c))
}

/// A flake8 / ruff code (`E501`, `F401`, `PLR0913`, `ASYNC100`): one to six capitals,
/// then one to four digits. In a `noqa` such codes may be separated by whitespace alone;
/// every other rule list is separated by commas.
fn is_noqa_code(token: &str) -> bool {
    let digits = token.trim_start_matches(|c: char| c.is_ascii_uppercase());
    let letters = token.len() - digits.len();
    (1..=6).contains(&letters)
        && (1..=4).contains(&digits.len())
        && digits.chars().all(|c| c.is_ascii_digit())
}

/// The rule identifiers a site names, and nothing else: the description a tool lets
/// follow its rules (`-- reason` for ESLint, `// reason` for `nolint`, `# reason` for
/// `noqa`) and any free text are dropped. The list stops at the first word that is not
/// spelled as an identifier, or that follows the previous identifier with no comma
/// between them (in a `noqa`, `spaced_codes`, a flake8 code excepted); quotes, braces and
/// brackets around the rules are ignored.
pub fn rule_identifiers(rule: &str, spaced_codes: bool) -> Vec<String> {
    let mut rest = rule;
    for marker in ["--", "//", "/*", "*/", "#", ";"] {
        if let Some(at) = rest.find(marker) {
            rest = &rest[..at];
        }
    }
    let mut rest = rest.trim();
    // Java's `value = "x"` and Scala's `Array("x")` wrap the rules.
    if let Some(after) = rest.strip_prefix("value") {
        if let Some(value) = after.trim_start().strip_prefix('=') {
            rest = value;
        }
    }
    if let Some(inner) = rest.strip_prefix("Array(") {
        rest = inner;
    }
    // A named argument (`reason = "..."`, `Justification = "..."`, `cat=deprecation`)
    // ends the rules: neither its name nor its value is one.
    let named = rest.find('=');
    if let Some(at) = named {
        rest = &rest[..at];
    }
    let mut ids: Vec<String> = Vec::new();
    let mut comma = true;
    let mut token = String::new();
    let wrapper = |c: char| "\"'{}()[]".contains(c);
    for c in rest.chars().chain(std::iter::once(' ')) {
        if c == ',' || c.is_whitespace() || wrapper(c) {
            if !token.is_empty() {
                let accepted = is_rule_identifier(&token)
                    && (comma || ids.is_empty() || (spaced_codes && is_noqa_code(&token)));
                if !accepted || ids.len() >= 16 {
                    break;
                }
                ids.push(std::mem::take(&mut token));
                comma = false;
            }
            if c == ',' {
                comma = true;
            }
        } else {
            token.push(c);
        }
    }
    if named.is_some() {
        ids.pop();
    }
    ids
}

/// The part of a site's rule that lists its rules, for the tools whose packs report the
/// whole comment or directive as the rule: the words after `#pragma warning disable`,
/// after `nolint:` / `lint:ignore` / `revive:disable`, inside `NOLINT(...)` and inside
/// `SuppressMessage(...)`.
fn rule_list<'r>(tool: &str, rule: &'r str) -> &'r str {
    let r = rule.trim();
    match tool {
        "#pragma warning disable" => r.split_once("warning disable").map_or(r, |(_, rest)| rest),
        "// NOLINT" => {
            let after = r.trim_start_matches(|c: char| c.is_ascii_uppercase());
            if after.len() == r.len() {
                r
            } else {
                after
                    .strip_prefix('(')
                    .and_then(|inner| inner.split_once(')'))
                    .map_or("", |(inner, _)| inner)
            }
        }
        "//nolint" => ["nolint", "lint:ignore", "revive:disable"]
            .iter()
            .find_map(|word| r.strip_prefix(word))
            .map_or(r, |rest| rest.trim_start_matches(':')),
        "SuppressMessage" => r.split_once('(').map_or(r, |(_, inner)| inner),
        _ => r,
    }
}

/// A label for the suppression kind and rule (e.g. `allow(clippy::x)`, `noqa: F401`),
/// reported by default instead of quoting the raw source line. It holds a fixed tool name
/// and the [`rule_identifiers`] of the site, never the site's free text: a description
/// after the rules (`-- token=...`) can hold anything.
pub fn suppression_kind_label(pat: &str, site: &Site) -> String {
    let tool = tool_label(pat, site);
    if tool.starts_with("// @ts-") {
        // The rule is the checker (`typescript`); the rest of the comment is free text.
        return tool.to_string();
    }
    if tool == "# type: ignore" {
        // The site's rule is the checker (`mypy`); the error codes are in the brackets.
        let codes = site
            .snippet
            .split_once("ignore[")
            .and_then(|(_, rest)| rest.split_once(']'))
            .map(|(inner, _)| rule_identifiers(inner, false))
            .unwrap_or_default();
        return if codes.is_empty() {
            "# type: ignore".to_string()
        } else {
            format!("type: ignore[{}]", codes.join(", "))
        };
    }
    let ids = rule_identifiers(rule_list(tool, &site.rule), tool == "# noqa");
    if ids.is_empty() {
        return tool.to_string();
    }
    let rule = ids.join(", ");
    if tool.starts_with("#[allow(") {
        format!("allow({rule})")
    } else if tool.starts_with("#[expect(") {
        format!("expect({rule})")
    } else if matches!(
        tool,
        "SuppressWarnings" | "SuppressMessage" | "SuppressLint" | "Suppress" | "nowarn"
    ) {
        format!("{tool}({rule})")
    } else if tool == "# noqa" {
        if rule == "noqa" {
            "# noqa".to_string()
        } else {
            format!("noqa: {rule}")
        }
    } else if tool.contains("NOLINT") || tool.contains("nolint") {
        format!("{tool}({rule})")
    } else if tool == rule || tool.ends_with(&format!(" {rule}")) {
        tool.to_string()
    } else {
        format!("{tool} {rule}")
    }
}

pub fn extract_suppression_rules(snippet: &str, pat: &str) -> Vec<String> {
    let mut rules = Vec::new();
    let trimmed = snippet.trim();

    if pat.contains("allow(") || pat.contains("expect(") {
        if let Some(start) = trimmed.find('(') {
            if let Some(end) = trimmed[start + 1..].find(')') {
                let inner = &trimmed[start + 1..start + 1 + end];
                for part in inner.split(',') {
                    let r = part.trim().trim_matches(['"', '\'']);
                    if !r.is_empty() {
                        rules.push(r.to_string());
                        if let Some(leaf) = r.rsplit("::").next() {
                            if leaf != r && !leaf.is_empty() {
                                rules.push(leaf.to_string());
                            }
                        }
                    }
                }
            }
        }
    } else if pat == "# noqa" {
        rules.push("noqa".to_string());
        if let Some(pos) = trimmed.find("# noqa:") {
            let after = &trimmed[pos + 8..];
            for part in after.split(|c: char| c == ',' || c.is_whitespace()) {
                let r = part.trim();
                if !r.is_empty() {
                    rules.push(r.to_string());
                }
            }
        }
    } else if pat == "# type: ignore" {
        rules.push("type: ignore".to_string());
        rules.push("type-ignore".to_string());
        rules.push("type_ignore".to_string());
        rules.push("ignore".to_string());
        if let Some(pos) = trimmed.find("# type: ignore[") {
            if let Some(end) = trimmed[pos + 15..].find(']') {
                let inner = &trimmed[pos + 15..pos + 15 + end];
                for part in inner.split(',') {
                    let r = part.trim();
                    if !r.is_empty() {
                        rules.push(r.to_string());
                    }
                }
            }
        }
    } else if pat == "# pylint: disable" {
        rules.push("pylint".to_string());
        if let Some(pos) = trimmed.find("disable=") {
            let after = &trimmed[pos + 8..];
            for part in after.split(',') {
                let r = part.trim();
                if !r.is_empty() {
                    rules.push(r.to_string());
                }
            }
        }
    } else if pat.starts_with("// @ts-") || pat.starts_with("@ts-") {
        let tag = pat.trim_start_matches("//").trim().trim_start_matches('@');
        rules.push(format!("@{tag}"));
        rules.push(tag.to_string());
        rules.push(tag.replace('-', "_"));
    } else if pat.contains("eslint-disable") {
        rules.push("eslint-disable".to_string());
        rules.push("eslint".to_string());
        let after = if let Some(p) = trimmed.find("eslint-disable-next-line") {
            &trimmed[p + 24..]
        } else if let Some(p) = trimmed.find("eslint-disable") {
            &trimmed[p + 14..]
        } else {
            ""
        };
        let cleaned = after.trim_end_matches("*/").trim();
        for part in cleaned.split(|c: char| c == ',' || c.is_whitespace()) {
            let r = part.trim();
            if !r.is_empty() {
                rules.push(r.to_string());
            }
        }
    } else if pat.to_ascii_uppercase().contains("NOLINT") {
        rules.push("NOLINT".to_string());
        rules.push("nolint".to_string());
        if let Some(start) = trimmed.find('(') {
            if let Some(end) = trimmed[start + 1..].find(')') {
                let inner = trimmed[start + 1..start + 1 + end].trim();
                if !inner.is_empty() {
                    rules.push(inner.to_string());
                }
            }
        }
    } else if pat.contains("nolint") || pat.contains("lint:ignore") {
        rules.push("nolint".to_string());
        rules.push("lint:ignore".to_string());
        rules.push("ignore".to_string());
        if let Some(pos) = trimmed.find("//nolint:") {
            let after = &trimmed[pos + 9..];
            for part in after.split(',') {
                let r = part.trim();
                if !r.is_empty() {
                    rules.push(r.to_string());
                }
            }
        }
    } else if pat.contains("pragma warning disable") {
        rules.push("pragma".to_string());
        if let Some(pos) = trimmed.find("disable") {
            let after = &trimmed[pos + 7..];
            for part in after.split_whitespace() {
                let r = part.trim_end_matches(';').trim();
                if !r.is_empty() {
                    rules.push(r.to_string());
                }
            }
        }
    }

    rules
}

#[cfg(test)]
mod tests {
    use crate::config::{Severity, SuppressionDeltaGate};

    #[test]
    fn test_suppression_delta_defaults() {
        let gate = SuppressionDeltaGate::default();
        assert!(gate.enabled);
        // Non-blocking by default: `#[allow]` / `# noqa` are routine reviewed
        // escape hatches, so the population in an unknown repository is high.
        assert_eq!(gate.severity, Severity::Warning);
        assert_eq!(gate.max_increase, 0);
    }

    /// Calls the gate over a change to `generated/bindings.rs` with `body` as its table.
    fn evaluate(body: &str) -> anyhow::Result<crate::guards::GateOutcome> {
        let config = crate::config::DisciplineConfig::from_toml_str(&format!(
            "[meta]\nversion = 1\nname = \"t\"\n[gates.suppression-delta]\nenabled = true\n{body}\n"
        ))?;
        let (_dir, git) = crate::gitctx::test_support::repo_with_changed_file(
            "generated/bindings.rs",
            "pub fn a() {}\n",
            "pub fn a() {}\n#[allow(dead_code)]\nfn b() {}\n",
        );
        super::evaluate_suppression_delta(&crate::guards::test_support::context(&config, &git))
    }

    /// `run_checks` compiles every glob before a gate runs, so the binary never brings
    /// an invalid one this far; the gate is called directly to reach its own refusal.
    #[test]
    fn invalid_exempt_glob_is_an_error_not_a_silently_skipped_pattern() {
        let Err(err) = evaluate("exempt_paths = [\"[\"]") else {
            panic!("an invalid exempt glob was accepted");
        };
        let text = format!("{err:#}");
        assert!(text.contains("invalid glob `[` in configuration"), "{text}");
    }

    /// Control: a glob that compiles exempts the file it matches, and only that file.
    #[test]
    fn valid_exempt_glob_still_matches() {
        let reported = evaluate("exempt_paths = [\"vendor/**\"]").unwrap();
        assert_eq!(reported.violations.len(), 1, "{:?}", reported.notes);
        let exempted = evaluate("exempt_paths = [\"generated/**\"]").unwrap();
        assert!(exempted.violations.is_empty(), "{:?}", exempted.notes);
    }

    #[test]
    fn a_moved_or_repeated_site_is_not_new_and_an_added_one_is() {
        use super::{new_sites, Site};
        let site = |line: usize, rule: &str| Site {
            line,
            kind: "linter-disable",
            rule: rule.into(),
            snippet: format!("#[allow({rule})]"),
        };
        let base = vec![site(3, "dead_code"), site(9, "unused")];
        let moved = vec![site(30, "unused"), site(12, "dead_code")];
        assert!(new_sites(&base, &moved).is_empty());
        let grown = vec![site(3, "dead_code"), site(9, "unused"), site(20, "unused")];
        assert_eq!(new_sites(&base, &grown), vec![site(20, "unused")]);
        let swapped = vec![site(3, "dead_code"), site(9, "clippy::all")];
        assert_eq!(new_sites(&base, &swapped), vec![site(9, "clippy::all")]);
        assert_eq!(new_sites(&[], &base).len(), 2);
    }

    #[test]
    fn test_extract_suppression_rules() {
        use super::extract_suppression_rules;

        let rust_rules = extract_suppression_rules("#[allow(dead_code, clippy::all)]", "#[allow(");
        assert!(rust_rules.contains(&"dead_code".to_string()));
        assert!(rust_rules.contains(&"clippy::all".to_string()));
        assert!(rust_rules.contains(&"all".to_string()));

        let py_noqa = extract_suppression_rules("x = 1 # noqa: E501", "# noqa");
        assert!(py_noqa.contains(&"noqa".to_string()));
        assert!(py_noqa.contains(&"E501".to_string()));

        let py_type =
            extract_suppression_rules("y = 2 # type: ignore[attr-defined]", "# type: ignore");
        assert!(py_type.contains(&"type: ignore".to_string()));
        assert!(py_type.contains(&"attr-defined".to_string()));

        let ts_rules = extract_suppression_rules("// @ts-ignore", "// @ts-ignore");
        assert!(ts_rules.contains(&"ts-ignore".to_string()));
        assert!(ts_rules.contains(&"@ts-ignore".to_string()));

        let cpp_rules = extract_suppression_rules("// NOLINT(readability-something)", "// NOLINT");
        assert!(cpp_rules.contains(&"NOLINT".to_string()));
        assert!(cpp_rules.contains(&"readability-something".to_string()));
    }

    #[test]
    fn rule_identifiers_stop_at_free_text() {
        use super::rule_identifiers;
        let ids = |r: &str| rule_identifiers(r, false);
        assert_eq!(ids("no-console -- token=x"), ["no-console"]);
        assert_eq!(ids("no-alert, no-eval -- why"), ["no-alert", "no-eval"]);
        assert_eq!(ids("invalid-name because SECRET"), ["invalid-name"]);
        assert_eq!(ids("clippy::x, reason = \"SECRET\""), ["clippy::x"]);
        assert_eq!(ids("{\"a\", \"b\"}"), ["a", "b"]);
        assert_eq!(ids("value = \"x\""), ["x"]);
        assert_eq!(ids("Array(\"x\")"), ["x"]);
        assert_eq!(ids("cat=deprecation"), Vec::<String>::new());
        assert_eq!(ids("tok$en"), Vec::<String>::new());
        assert_eq!(ids("E1 # more"), ["E1"]);
        assert_eq!(
            rule_identifiers("F401 E501 SECRETWORD42", true),
            ["F401", "E501"]
        );
        assert_eq!(rule_identifiers("F401 FREETEXTSECRET42", true), ["F401"]);
        assert_eq!(ids("F401 E501"), ["F401"]);
    }

    #[test]
    fn test_suppression_kind_label() {
        use super::{suppression_kind_label, Site};
        let site = |rule: &str, snippet: &str| Site {
            line: 1,
            kind: "linter-disable",
            rule: rule.into(),
            snippet: snippet.into(),
        };

        assert_eq!(
            suppression_kind_label("#[allow(", &site("clippy::all", "#[allow(clippy::all)]")),
            "allow(clippy::all)"
        );
        assert_eq!(
            suppression_kind_label("#[expect(", &site("dead_code", "#[expect(dead_code)]")),
            "expect(dead_code)"
        );
        assert_eq!(
            suppression_kind_label(
                "@SuppressWarnings",
                &site("unchecked", "@SuppressWarnings(\"unchecked\")")
            ),
            "SuppressWarnings(unchecked)"
        );
        assert_eq!(
            suppression_kind_label("# noqa", &site("E501", "x = 1 # noqa: E501")),
            "noqa: E501"
        );
        assert_eq!(
            suppression_kind_label("# noqa", &site("noqa", "x = 1 # noqa")),
            "# noqa"
        );
        assert_eq!(
            suppression_kind_label(
                "# type: ignore",
                &site("attr-defined", "y = 1 # type: ignore[attr-defined]")
            ),
            "type: ignore[attr-defined]"
        );
        assert_eq!(
            suppression_kind_label("// NOLINT", &site("readability", "// NOLINT(readability)")),
            "// NOLINT(readability)"
        );
    }

    #[test]
    fn test_quote_line_config_controls_line_text_in_remediation() {
        // Default: quote_line = false -> does not quote snippet in remediation
        let default_run = evaluate("").unwrap();
        assert_eq!(default_run.violations.len(), 1);
        let rem_default = default_run.violations[0].remediation.as_deref().unwrap();
        assert!(rem_default.contains("allow(dead_code)"));
        assert!(!rem_default.contains("#[allow(dead_code)]"));

        // Opt-in: quote_line = true -> quotes snippet in remediation
        let quoted_run = evaluate("quote_line = true").unwrap();
        assert_eq!(quoted_run.violations.len(), 1);
        let rem_quoted = quoted_run.violations[0].remediation.as_deref().unwrap();
        assert!(rem_quoted.contains("allow(dead_code)"));
        // The source snippet `#[allow(dead_code)]` is quoted; `scrub_override_directives`
        // redacts `allow(` to prevent agent directive instruction bypasses.
        assert!(rem_quoted.contains("dead_code)]"));
    }
}
