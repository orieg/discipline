//! Verification claim labels sentinel (`claim-labels`, `#703`).
//!
//! Enforces that verification claims in the pull request body, commit messages in range,
//! and added lines of Markdown files carry a `RUN` or `READ` marker in the same paragraph
//! or list item, rejects placeholder markers (`(RUN: <command>)`, `(READ: file:line)`),
//! verifies `(READ: path:line)` targets against the repository at `HEAD`, and notes the
//! count of `RUN` labels that could not be tied to a vouched verification command.

use crate::guards::{line_allows, Context, GateOutcome, PathFilter};
use crate::tokens;
use anyhow::{anyhow, Result};
use regex::Regex;
use std::collections::BTreeSet;
use std::sync::LazyLock;

pub const GATE: &str = "claim-labels";

/// Built-in verification claim patterns (`docs/benchmarks/claim-labels-precision.md`).
/// Kept as data so unit tests and `self-test` exercise the exact shipped patterns.
pub fn claim_label_patterns() -> Vec<&'static str> {
    vec![
        // Class 1: tests-passed
        r"(?i)\b(?:all (?:unit |integration |e2e |end-to-end |existing |new |\d+ )?tests? pass(?:ed|es|ing)?|\d+\s*/\s*\d+\s+tests?\s+pass(?:ed|es)?|\d+\s+passed;\s*0\s+failed|(?:full |complete |entire )?test suite pass(?:ed|es)|100%\s+pass\s+rate)\b",
        // Class 2: verified-by-execution (anchored to clause/bullet start to avoid matching passive software descriptions)
        r"(?i)(?:^|[-*•]\s+|\.\s+|;\s+)(?:verified|confirmed|reproduced)\s+(?:by\s+running|locally|that|on|with|against)\b",
        // Class 3: zero-regressions / zero-warnings
        r"(?i)\b(?:no regressions?|zero regressions?|zero (?:clippy |compiler |formatting )?(?:warnings?|errors?|deviations?))\b",
    ]
}

static STRUCTURED_MARKER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[\(\[]\s*`?(RUN|READ)\s*:\s*([^\)\]\r\n`]+?)`?\s*[\)\]]").expect("valid regex")
});

static BARE_MARKER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:\(\s*`?(RUN|READ)`?\s*\)|\[\s*`?(RUN|READ)`?\s*\]|`(RUN|READ)`|\*\*(RUN|READ)\*\*|(?:^|[\s—–\-:;,\(\[])(RUN|READ)\s*:|(?:—|–|--)\s*(RUN|READ)\b)",
    )
    .expect("valid regex")
});

static CODE_SPAN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`([^`]+)`").expect("valid regex"));

static QUOTED_TEXT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""[^"]*"|“[^”]*”"#).expect("valid regex"));

const LABEL_PLACEHOLDERS: &[&str] = &[
    "command",
    "cmd",
    "target",
    "path",
    "file",
    "file:line",
    "path:line",
    "path/to/file",
    "path/to/file:line",
    "...",
    "todo",
    "tbd",
    "xxx",
    "fixme",
    "placeholder",
    "none",
    "n/a",
    "na",
];

#[derive(Debug, Clone)]
pub struct ProseUnit {
    pub lines: Vec<(usize, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructuredMarkerKind {
    Run,
    Read,
}

#[derive(Debug, Clone)]
pub struct StructuredMarker {
    pub line_num: usize,
    pub kind: StructuredMarkerKind,
    pub target: String,
}

/// Splits Markdown/prose text into paragraphs, list items (including continuation lines),
/// and table rows, ignoring fenced code blocks (` ``` ` / `~~~`), indented code blocks,
/// and blockquotes (`>`).
pub fn extract_prose_units(content: &str) -> Vec<ProseUnit> {
    let mut units = Vec::new();
    let mut current: Vec<(usize, String)> = Vec::new();
    let mut in_list_item = false;
    let mut fence: Option<(char, usize)> = None;

    let flush = |cur: &mut Vec<(usize, String)>, out: &mut Vec<ProseUnit>| {
        if !cur.is_empty() {
            out.push(ProseUnit {
                lines: std::mem::take(cur),
            });
        }
    };

    for (idx, raw_line) in content.lines().enumerate() {
        let line_num = idx + 1;
        let trimmed = raw_line.trim();

        // Fenced code blocks (` ``` ` or `~~~`)
        let fence_char = trimmed.chars().next().filter(|&c| c == '`' || c == '~');
        if let Some(ch) = fence_char {
            let count = trimmed.chars().take_while(|&c| c == ch).count();
            if count >= 3 {
                match fence {
                    None => {
                        flush(&mut current, &mut units);
                        in_list_item = false;
                        fence = Some((ch, count));
                    }
                    Some((open_ch, open_count))
                        if open_ch == ch
                            && count >= open_count
                            && trimmed[count..].trim().is_empty() =>
                    {
                        fence = None;
                    }
                    _ => {}
                }
                continue;
            }
        }
        if fence.is_some() {
            continue;
        }

        if trimmed.is_empty() {
            flush(&mut current, &mut units);
            in_list_item = false;
            continue;
        }

        // Blockquotes (`> ...`) and HTML comments are not claims
        if trimmed.starts_with('>') || (trimmed.starts_with("<!--") && trimmed.ends_with("-->")) {
            flush(&mut current, &mut units);
            in_list_item = false;
            continue;
        }

        // Indented code block (4+ leading spaces or leading tab when not continuing a list item)
        let indent_cols: usize = raw_line
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .map(|c| if c == '\t' { 4 } else { 1 })
            .sum();
        if indent_cols >= 4 && !in_list_item {
            flush(&mut current, &mut units);
            continue;
        }

        // Headings flush the current unit and are not claims themselves
        if trimmed.starts_with('#') {
            flush(&mut current, &mut units);
            in_list_item = false;
            continue;
        }

        // Table rows: each row is its own unit (separator rows are skipped)
        if trimmed.starts_with('|') || trimmed.ends_with('|') {
            flush(&mut current, &mut units);
            in_list_item = false;
            let is_sep = trimmed
                .split('|')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .all(|c| {
                    c.chars()
                        .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
                });
            if !is_sep {
                units.push(ProseUnit {
                    lines: vec![(line_num, raw_line.to_string())],
                });
            }
            continue;
        }

        let is_bullet = trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
            || trimmed.starts_with("+ ")
            || trimmed.starts_with("• ")
            || {
                let digits = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
                digits > 0 && trimmed[digits..].starts_with(". ")
            };

        if is_bullet {
            flush(&mut current, &mut units);
            in_list_item = true;
            current.push((line_num, raw_line.to_string()));
        } else {
            current.push((line_num, raw_line.to_string()));
        }
    }

    flush(&mut current, &mut units);
    units
}

/// Whether `target` inside `(RUN: <target>)` or `(READ: <target>)` is a placeholder.
pub fn is_label_placeholder(target: &str) -> bool {
    let clean = target
        .trim()
        .trim_matches(|c: char| matches!(c, '`' | '"' | '\''));
    if clean.is_empty() {
        return true;
    }
    if clean.starts_with('<') && clean.ends_with('>') {
        return true;
    }
    if clean.contains("<command>")
        || clean.contains("<cmd>")
        || clean.contains("<path>")
        || clean.contains("<file")
        || clean.contains("<line>")
        || clean.contains("<target>")
    {
        return true;
    }
    let lower = clean.to_ascii_lowercase();
    LABEL_PLACEHOLDERS.contains(&lower.as_str())
}

/// Strips inline code spans and quoted strings, and strips leading bullet markers so
/// clause-anchored claim patterns match at the start of a list item.
fn strip_non_claim_spans(line: &str) -> String {
    let without_code = CODE_SPAN_RE.replace_all(line, " ");
    let without_quotes = QUOTED_TEXT_RE.replace_all(&without_code, " ");
    without_quotes.into_owned()
}

fn is_question_or_todo(line: &str) -> bool {
    let t = line.trim();
    if t.ends_with('?') {
        return true;
    }
    let after_bullet = t
        .strip_prefix("- ")
        .or_else(|| t.strip_prefix("* "))
        .or_else(|| t.strip_prefix("+ "))
        .unwrap_or(t)
        .trim_start();
    if after_bullet.starts_with("[ ]") {
        return true;
    }
    let lower = after_bullet.to_ascii_lowercase();
    lower.starts_with("todo:") || lower.starts_with("fixme:")
}

/// Resolves a `(READ: path[:line])` target against the repository at `HEAD` (falling back
/// to the working tree on disk when a file is newly added in the working tree).
/// Returns `Ok(None)` when valid, or `Ok(Some(reason))` when unresolvable.
pub fn check_read_target(target: &str, ctx: &Context) -> Result<Option<String>> {
    let token = target
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(|c: char| matches!(c, '`' | '"' | '\'' | ',' | ';'));
    if token.is_empty() {
        return Ok(Some("empty READ target".to_string()));
    }

    // Parse optional `:line` (or `:start-end`) suffix after the path.
    let (path_part, line_spec) = match token.rsplit_once(':') {
        Some((p, rest))
            if !p.is_empty() && rest.chars().next().is_some_and(|c| c.is_ascii_digit()) =>
        {
            (p, Some(rest))
        }
        _ => (token, None),
    };

    if path_part.starts_with('/')
        || path_part.starts_with('\\')
        || path_part == "."
        || path_part == ".."
        || path_part.split('/').any(|seg| seg == "..")
    {
        return Ok(Some(format!(
            "`{path_part}` resolves outside the repository"
        )));
    }

    let content = match ctx.git.head_content(path_part)? {
        Some(c) => c,
        None => {
            let disk = ctx.git.root().join(path_part);
            if disk.is_file() {
                std::fs::read_to_string(&disk).unwrap_or_default()
            } else {
                return Ok(Some(format!("`{path_part}` does not exist at HEAD")));
            }
        }
    };

    if let Some(spec) = line_spec {
        // Accept `line` or `start-end`
        let end_str = spec.split('-').next_back().unwrap_or(spec);
        let start_str = spec.split('-').next().unwrap_or(spec);
        let (Ok(start_line), Ok(end_line)) = (start_str.parse::<usize>(), end_str.parse::<usize>())
        else {
            return Ok(Some(format!(
                "`{token}` has an invalid line number `{spec}`"
            )));
        };
        let total_lines = content.lines().count();
        if start_line == 0 || end_line < start_line || end_line > total_lines {
            return Ok(Some(format!(
                "`{path_part}:{spec}` is out of range (file has {total_lines} line(s))"
            )));
        }
    }

    Ok(None)
}

fn compile_patterns(extra: &[String], allow: &[String]) -> Result<(Vec<Regex>, Vec<Regex>)> {
    let mut claim_res = Vec::new();
    for pat in claim_label_patterns() {
        claim_res.push(Regex::new(pat).expect("built-in claim pattern is valid regex"));
    }
    for pat in extra {
        let re = Regex::new(pat).map_err(|e| {
            crate::could_not_check::tag_gate(
                GATE,
                anyhow!("`gates.claim-labels.extra_patterns` pattern `{pat}` is invalid: {e}"),
            )
        })?;
        claim_res.push(re);
    }
    let mut allow_res = Vec::new();
    for pat in allow {
        let re = Regex::new(pat).map_err(|e| {
            crate::could_not_check::tag_gate(
                GATE,
                anyhow!("`gates.claim-labels.allow_patterns` pattern `{pat}` is invalid: {e}"),
            )
        })?;
        allow_res.push(re);
    }
    Ok((claim_res, allow_res))
}

struct SourceTarget<'a> {
    label: &'a str,
    override_subjects: &'a [String],
    content: &'a str,
    added_lines: Option<&'a BTreeSet<usize>>,
}

struct ScanEnv<'a> {
    claim_res: &'a [Regex],
    allow_res: &'a [Regex],
    vouched_cmds: &'a [String],
    ctx: &'a Context<'a>,
    out: &'a mut GateOutcome,
    unchecked_run_labels: &'a mut usize,
}

fn inspect_source(target: &SourceTarget<'_>, env: &mut ScanEnv<'_>) -> Result<()> {
    let source_label = target.label;
    let added_lines = target.added_lines;
    let severity = env
        .ctx
        .overridable(env.ctx.config.gates.claim_labels.severity);
    let units = extract_prose_units(target.content);

    for unit in units {
        env.out.examined += 1;

        let unit_has_in_scope_line = match added_lines {
            Some(added) => unit.lines.iter().any(|(ln, _)| added.contains(ln)),
            None => true,
        };
        if !unit_has_in_scope_line {
            continue;
        }

        let unit_text: String = unit
            .lines
            .iter()
            .map(|(_, l)| l.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        // 1. Collect structured and bare markers in this paragraph / list item.
        let mut has_valid_marker = false;
        let mut structured_markers = Vec::new();
        for (ln, line) in &unit.lines {
            for caps in STRUCTURED_MARKER_RE.captures_iter(line) {
                let kind = if caps.get(1).unwrap().as_str().eq_ignore_ascii_case("RUN") {
                    StructuredMarkerKind::Run
                } else {
                    StructuredMarkerKind::Read
                };
                let marker_target = caps.get(2).unwrap().as_str().trim().to_string();
                structured_markers.push(StructuredMarker {
                    line_num: *ln,
                    kind,
                    target: marker_target,
                });
            }
        }

        for marker in &structured_markers {
            let line_in_scope = added_lines.is_none_or(|added| added.contains(&marker.line_num));
            let line_text = unit
                .lines
                .iter()
                .find(|(ln, _)| *ln == marker.line_num)
                .map(|(_, l)| l.as_str())
                .unwrap_or("");
            if line_allows(line_text, GATE) {
                env.out.overrides.push(tokens::OverrideRecord {
                    gate: GATE.to_string(),
                    code: None,
                    subject: format!("{source_label}:{}", marker.line_num),
                    directive: "discipline:allow(claim-labels)".to_string(),
                    reason: "inline exemption marker".to_string(),
                    source: tokens::OverrideSource::Inline {
                        file: source_label.to_string(),
                        line: marker.line_num,
                    },
                    hidden: true,
                });
                has_valid_marker = true;
                continue;
            }

            if is_label_placeholder(&marker.target) {
                if line_in_scope {
                    report_or_override(
                        env,
                        target,
                        severity,
                        &crate::findings::LABEL_PLACEHOLDER,
                        marker.line_num,
                        format!(
                            "verification label placeholder `{}` in `{source_label}:{}`; replace the placeholder with the actual command or `path:line`",
                            marker.target, marker.line_num
                        ),
                    );
                }
                continue;
            }

            match marker.kind {
                StructuredMarkerKind::Read => match check_read_target(&marker.target, env.ctx)? {
                    Some(reason) => {
                        if line_in_scope {
                            report_or_override(
                                env,
                                target,
                                severity,
                                &crate::findings::READ_LABEL_UNRESOLVABLE,
                                marker.line_num,
                                format!(
                                    "unresolvable `(READ: {})` label in `{source_label}:{}`: {reason}",
                                    marker.target, marker.line_num
                                ),
                            );
                        }
                    }
                    None => {
                        has_valid_marker = true;
                    }
                },
                StructuredMarkerKind::Run => {
                    has_valid_marker = true;
                    let clean_cmd = marker
                        .target
                        .trim()
                        .trim_matches(|c: char| matches!(c, '`' | '"' | '\''));
                    if matches_vouched_command(clean_cmd, env.vouched_cmds).is_none() {
                        *env.unchecked_run_labels += 1;
                    }
                }
            }
        }

        // Check bare `RUN` / `READ` markers if no structured marker already satisfied the unit.
        if !has_valid_marker && structured_markers.is_empty() {
            if let Some(caps) = BARE_MARKER_RE.captures(&unit_text) {
                has_valid_marker = true;
                let matched_word = (1..=6)
                    .find_map(|i| caps.get(i).map(|m| m.as_str()))
                    .unwrap_or("");
                if matched_word.eq_ignore_ascii_case("RUN") {
                    let any_span_vouched = CODE_SPAN_RE.captures_iter(&unit_text).any(|c| {
                        let span = c.get(1).map(|m| m.as_str()).unwrap_or("");
                        matches_vouched_command(span, env.vouched_cmds).is_some()
                    });
                    if !any_span_vouched {
                        *env.unchecked_run_labels += 1;
                    }
                }
            }
        }

        if has_valid_marker {
            continue;
        }

        // 2. No valid marker in this paragraph/list item: check in-scope lines for claims.
        for (ln, line) in &unit.lines {
            if !added_lines.is_none_or(|added| added.contains(ln)) {
                continue;
            }
            if line_allows(line, GATE) {
                env.out.overrides.push(tokens::OverrideRecord {
                    gate: GATE.to_string(),
                    code: Some(crate::findings::full_code(
                        GATE,
                        &crate::findings::CLAIM_WITHOUT_LABEL,
                    )),
                    subject: format!("{source_label}:{ln}"),
                    directive: "discipline:allow(claim-labels)".to_string(),
                    reason: "inline exemption marker".to_string(),
                    source: tokens::OverrideSource::Inline {
                        file: source_label.to_string(),
                        line: *ln,
                    },
                    hidden: true,
                });
                break;
            }
            if is_question_or_todo(line) {
                continue;
            }
            if env.allow_res.iter().any(|re| re.is_match(line)) {
                continue;
            }
            let cleaned = strip_non_claim_spans(line);
            if let Some(matched) = env
                .claim_res
                .iter()
                .find_map(|re| re.find(&cleaned).map(|m| m.as_str().trim()))
            {
                let phrase = matched.trim_start_matches(|c: char| {
                    matches!(c, '-' | '*' | '•' | '.' | ';') || c.is_whitespace()
                });
                report_or_override(
                    env,
                    target,
                    severity,
                    &crate::findings::CLAIM_WITHOUT_LABEL,
                    *ln,
                    format!(
                        "verification claim `{phrase}` in `{source_label}:{ln}` carries no `RUN` or `READ` label in the same paragraph or list item"
                    ),
                );
                break;
            }
        }
    }

    Ok(())
}

fn vouched_commands_for_context(ctx: &Context) -> Vec<String> {
    let cfg = ctx.config;
    let mut out = Vec::new();
    if let Some(cmd) = cfg
        .gates
        .command
        .command
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        out.push(cmd.to_string());
    }
    for entry in &cfg.gates.command.commands {
        if let Some(cmd) = entry
            .command
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if !out.iter().any(|c| c == cmd) {
                out.push(cmd.to_string());
            }
        }
    }
    let sessions_dir = ctx.git.root().join(".git/discipline/sessions");
    let mut saw_session_file = false;
    if let Ok(entries) = std::fs::read_dir(&sessions_dir) {
        for entry in entries.flatten() {
            if entry.path().extension().is_some_and(|e| e == "json") {
                saw_session_file = true;
                if let Ok(text) = std::fs::read_to_string(entry.path()) {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                        if let Some(cmds) = val.get("commands").and_then(|c| c.as_array()) {
                            for item in cmds {
                                let passed = item
                                    .get("passed")
                                    .and_then(|b| b.as_bool())
                                    .or_else(|| {
                                        item.get("exit_code")
                                            .and_then(|c| c.as_i64())
                                            .map(|c| c == 0)
                                    })
                                    .unwrap_or(false);
                                if passed {
                                    if let Some(cmd) = item
                                        .get("command")
                                        .and_then(|s| s.as_str())
                                        .map(str::trim)
                                        .filter(|s| !s.is_empty())
                                    {
                                        if !out.iter().any(|c| c == cmd) {
                                            out.push(cmd.to_string());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    if out.is_empty() && !saw_session_file {
        for default_cmd in [
            "cargo test",
            "pytest",
            "go test ./...",
            "npm test",
            "pnpm test",
        ] {
            out.push(default_cmd.to_string());
        }
    }
    out
}

fn normalize_ws(cmd: &str) -> String {
    cmd.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn matches_vouched_command(raw_cmd: &str, vouched: &[String]) -> Option<String> {
    let norm = normalize_ws(raw_cmd);
    if norm.is_empty() {
        return None;
    }
    vouched.iter().find(|v| normalize_ws(v) == norm).cloned()
}

fn report_or_override(
    env: &mut ScanEnv<'_>,
    target: &SourceTarget<'_>,
    severity: crate::config::Severity,
    kind: &'static crate::findings::FindingKind,
    line: usize,
    message: String,
) {
    let source_label = target.label;
    let override_subjects = target.override_subjects;
    let line_subject = format!("{source_label}:{line}");
    if let Some(ov) = env
        .ctx
        .find_override(GATE, kind, tokens::ALLOW_CLAIM_LABEL, &line_subject)
    {
        env.out.overrides.push(ov);
        return;
    }
    for subj in override_subjects {
        if let Some(ov) =
            env.ctx
                .find_whole_file_override(GATE, kind, tokens::ALLOW_CLAIM_LABEL, subj)
        {
            env.out.overrides.push(ov);
            return;
        }
    }
    let primary_subject = override_subjects
        .first()
        .map(String::as_str)
        .unwrap_or(source_label);
    let remediation = format!(
        "Add `(RUN: <command>)` or `(READ: path:line)` in the same paragraph or list item, or justify with `allow-claim-label: {primary_subject} <reason>` in the PR body or a commit message."
    );
    env.out
        .add_violation(severity, kind, source_label, line, message, remediation);
}

/// Evaluates `claim-labels` across the pull request body, commit messages in range, and
/// changed (or tracked) Markdown files.
pub fn evaluate_claim_labels(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.claim_labels;
    let mut out = GateOutcome::new(GATE);

    let exempt = PathFilter::new(&settings.exempt_paths)?;
    let (claim_res, allow_res) =
        compile_patterns(&settings.extra_patterns, &settings.allow_patterns)?;
    let vouched_cmds = vouched_commands_for_context(ctx);
    let mut unchecked_run_labels = 0usize;

    let mut env = ScanEnv {
        claim_res: &claim_res,
        allow_res: &allow_res,
        vouched_cmds: &vouched_cmds,
        ctx,
        out: &mut out,
        unchecked_run_labels: &mut unchecked_run_labels,
    };

    // 1. Pull request body
    match &ctx.pr_body {
        Some(body) => {
            let subjects = vec![
                "pr-body".to_string(),
                "PR body".to_string(),
                "<PR body>".to_string(),
            ];
            let target = SourceTarget {
                label: "PR body",
                override_subjects: &subjects,
                content: body,
                added_lines: None,
            };
            inspect_source(&target, &mut env)?;
        }
        None => {
            env.out
                .notes
                .push("no PR body supplied; PR-body scan did not run".to_string());
        }
    }

    // 2. Commit messages in range
    for commit in ctx.git.commit_details()? {
        if commit.parent_count > 1 {
            continue;
        }
        let short_id: String = commit.sha.chars().take(7).collect();
        let label = format!("commit {short_id}");
        let subjects = vec![
            short_id,
            commit.sha.clone(),
            "commit".to_string(),
            "commits".to_string(),
        ];
        let target = SourceTarget {
            label: &label,
            override_subjects: &subjects,
            content: &commit.message,
            added_lines: None,
        };
        inspect_source(&target, &mut env)?;
    }

    // 3. Changed (or tracked) Markdown files
    if settings.diff_only {
        for file in ctx.git.changed_files()? {
            if file.is_deleted()
                || !(file.path.ends_with(".md") || file.path.ends_with(".mdx"))
                || exempt.matches(&file.path)
            {
                continue;
            }
            let Some(content) = ctx.git.head_content(&file.path)? else {
                continue;
            };
            let basename = file
                .path
                .rsplit('/')
                .next()
                .unwrap_or(&file.path)
                .to_string();
            let subjects = if basename != file.path {
                vec![file.path.clone(), basename]
            } else {
                vec![file.path.clone()]
            };
            let target = SourceTarget {
                label: &file.path,
                override_subjects: &subjects,
                content: &content,
                added_lines: Some(&file.added_lines),
            };
            inspect_source(&target, &mut env)?;
        }
    } else {
        for path in ctx.git.tracked_files()? {
            if !(path.ends_with(".md") || path.ends_with(".mdx")) || exempt.matches(&path) {
                continue;
            }
            let Some(content) = ctx.git.head_content(&path)? else {
                continue;
            };
            let basename = path.rsplit('/').next().unwrap_or(&path).to_string();
            let subjects = if basename != path {
                vec![path.clone(), basename]
            } else {
                vec![path.clone()]
            };
            let target = SourceTarget {
                label: &path,
                override_subjects: &subjects,
                content: &content,
                added_lines: None,
            };
            inspect_source(&target, &mut env)?;
        }
    }

    if unchecked_run_labels > 0 {
        out.notes.push(format!(
            "{unchecked_run_labels} RUN label(s) could not be tied to a vouched verification command (not recorded in active hook session state or `[gates.command]`)"
        ));
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DisciplineConfig, Severity};

    fn enabled_config() -> DisciplineConfig {
        DisciplineConfig::from_toml_str(
            r#"
[meta]
version = 1
name = "test"

[gates.claim-labels]
enabled = true

[gates.command]
command = "cargo test"
"#,
        )
        .unwrap()
    }

    fn eval_pr(
        cfg: &DisciplineConfig,
        head_files: &[(&str, &str)],
        pr_body: Option<&str>,
        directives: Vec<tokens::ParsedDirective>,
    ) -> GateOutcome {
        let (_dir, git) = crate::gitctx::test_support::repo_with_files(&[], head_files);
        let mut ctx = crate::guards::test_support::context(cfg, &git);
        ctx.pr_body = pr_body.map(str::to_string);
        ctx.directives = directives;
        evaluate_claim_labels(&ctx).unwrap()
    }

    #[test]
    fn defaults_are_opt_in_and_warning_severity() {
        let cfg = DisciplineConfig::default_for_repo("test");
        assert!(!cfg.gates.claim_labels.enabled);
        assert_eq!(cfg.gates.claim_labels.severity, Severity::Warning);
        assert!(cfg.gates.claim_labels.diff_only);
    }

    #[test]
    fn positive_and_negative_controls_for_each_phrase_class() {
        let cfg = enabled_config();
        let files = [("src/lib.rs", "pub fn a() {}\npub fn b() {}\n")];

        // Positive controls: unlabelled claims in each of the 3 classes must report.
        for unlabelled in [
            "- All unit tests passed across the workspace.",
            "- Verified locally on the reproduction branch.",
            "- Confirmed that zero clippy warnings remain.",
        ] {
            let out = eval_pr(&cfg, &files, Some(unlabelled), vec![]);
            assert_eq!(
                out.violations.len(),
                1,
                "expected 1 finding for `{unlabelled}`, got {:?}",
                out.violations
            );
            assert_eq!(out.violations[0].code, "claim-labels/claim-without-label");
        }

        // Negative controls: the same claims with `(RUN: ...)` or `(READ: ...)` in the same item pass.
        for labelled in [
            "- All unit tests passed across the workspace (RUN: cargo test).",
            "- Verified locally on the reproduction branch (`RUN`: `cargo test`).",
            "- Confirmed that zero clippy warnings remain (READ: src/lib.rs:2).",
        ] {
            let out = eval_pr(&cfg, &files, Some(labelled), vec![]);
            assert!(
                out.violations.is_empty(),
                "expected 0 findings for `{labelled}`, got {:?}",
                out.violations
            );
        }
    }

    #[test]
    fn quoted_fenced_code_span_question_and_todo_are_not_claims() {
        let cfg = enabled_config();
        let body = r#"
> All unit tests passed in the old report.

```text
All integration tests pass.
Confirmed by running the suite.
zero regressions
```

- The error message says "all tests passed" or `zero warnings`.
- Did all tests pass?
- [ ] Confirm that all tests pass before release.
- Each archive's attestation is verified with `--signer-workflow`.
"#;
        let out = eval_pr(&cfg, &[], Some(body), vec![]);
        assert!(
            out.violations.is_empty(),
            "unexpected findings: {:?}",
            out.violations
        );
    }

    #[test]
    fn marker_in_another_paragraph_does_not_satisfy() {
        let cfg = enabled_config();
        let files = [("src/lib.rs", "pub fn f() {}\n")];

        // First paragraph has a valid READ marker; second paragraph has an unlabelled claim.
        let body = "Inspected the parser implementation (READ: src/lib.rs:1).\n\nAll unit tests passed with zero regressions.\n";
        let out = eval_pr(&cfg, &files, Some(body), vec![]);
        assert_eq!(out.violations.len(), 1, "{:?}", out.violations);
        assert_eq!(out.violations[0].code, "claim-labels/claim-without-label");
        assert_eq!(out.violations[0].line, Some(3));
    }

    #[test]
    fn placeholder_label_is_rejected_and_does_not_satisfy_claim() {
        let cfg = enabled_config();
        let body =
            "- All unit tests passed (RUN: <command>).\n- Zero warnings (READ: file:line).\n";
        let out = eval_pr(&cfg, &[], Some(body), vec![]);
        let codes: Vec<&str> = out.violations.iter().map(|v| v.code.as_str()).collect();
        assert!(
            codes.contains(&"claim-labels/label-placeholder"),
            "expected label-placeholder in {codes:?}"
        );
        assert!(
            codes.contains(&"claim-labels/claim-without-label"),
            "placeholder must not satisfy the claim: {codes:?}"
        );
    }

    #[test]
    fn read_label_resolution_checks_missing_path_line_past_end_and_outside_repo() {
        let cfg = enabled_config();
        let files = [("src/lib.rs", "line 1\nline 2\n")];

        for bad_body in [
            "- Confirmed that the guard rejects empty input (READ: src/missing.rs:1).",
            "- Confirmed that the guard rejects empty input (READ: src/lib.rs:99).",
            "- Confirmed that the guard rejects empty input (READ: ../outside.rs:1).",
        ] {
            let out = eval_pr(&cfg, &files, Some(bad_body), vec![]);
            let codes: Vec<&str> = out.violations.iter().map(|v| v.code.as_str()).collect();
            assert!(
                codes.contains(&"claim-labels/read-label-unresolvable"),
                "expected read-label-unresolvable for `{bad_body}`, got {codes:?}"
            );
        }

        // Valid READ target resolves cleanly:
        let ok_body = "- Confirmed that the guard rejects empty input (READ: src/lib.rs:2).";
        let out = eval_pr(&cfg, &files, Some(ok_body), vec![]);
        assert!(out.violations.is_empty(), "{:?}", out.violations);
    }

    #[test]
    fn unchecked_run_labels_are_counted_in_notes_and_vouched_commands_are_not() {
        let cfg = enabled_config();

        // `cargo test` is vouched by `[gates.command]`: no note.
        let vouched_body = "- All tests pass (RUN: cargo test).";
        let out = eval_pr(&cfg, &[], Some(vouched_body), vec![]);
        assert!(out.violations.is_empty());
        assert!(
            !out.notes.iter().any(|n| n.contains("RUN label(s)")),
            "vouched RUN command should not be counted as unchecked: {:?}",
            out.notes
        );

        // `custom-check.sh` is not a vouched command: accepted, counted in note.
        let custom_body = "- All tests pass (RUN: ./scripts/custom-check.sh).";
        let out2 = eval_pr(&cfg, &[], Some(custom_body), vec![]);
        assert!(out2.violations.is_empty());
        assert!(
            out2.notes
                .iter()
                .any(|n| n.contains("1 RUN label(s) could not be tied")),
            "expected unchecked RUN note, got {:?}",
            out2.notes
        );
    }

    #[test]
    fn directive_lifts_pr_body_and_markdown_findings() {
        let cfg = enabled_config();
        let directives = tokens::parse_directives(
            "allow-claim-label: pr-body historical summary\n\
             allow-claim-label: docs/notes.md prose example\n",
            tokens::OverrideSource::PrBody,
        );
        let out = eval_pr(
            &cfg,
            &[("docs/notes.md", "# Notes\nAll unit tests passed.\n")],
            Some("All tests pass."),
            directives,
        );
        assert!(out.violations.is_empty(), "{:?}", out.violations);
        assert_eq!(out.overrides.len(), 2);
    }
}
