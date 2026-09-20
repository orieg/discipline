//! Whole-tree hygiene gates. They sweep every tracked file (not just the
//! diff), plus the PR body when one is supplied.

use super::{exempt_filter, line_allows, Context, GateOutcome, PathFilter};
use crate::config::{GateSettings, PiiGate};
use anyhow::{Context as _, Result};
use regex::Regex;

pub fn agents_md(ctx: &Context) -> Result<GateOutcome> {
    const GATE: &str = "agents-md";
    let settings = &ctx.config.gates.agents_md;
    let exempt = exempt_filter(settings)?;
    let mut out = GateOutcome::new(GATE);
    out.examined = 1;

    if !ctx.git.is_tracked("AGENTS.md")? {
        out.push(
            settings.severity(),
            "Missing AGENTS.md",
            Some("AGENTS.md"),
            None,
            "The repository tracks no canonical AGENTS.md to govern AI agent behavior.".to_string(),
            "Create AGENTS.md and symlink CLAUDE.md / GEMINI.md to it.",
        );
        return Ok(out);
    }

    let canonical = ctx.git.head_content("AGENTS.md")?;
    for alias in ["CLAUDE.md", "GEMINI.md"] {
        if exempt.matches(alias) || !ctx.git.is_tracked(alias)? || ctx.git.is_symlink(alias)? {
            continue;
        }
        out.examined += 1;
        if ctx.git.head_content(alias)? != canonical {
            out.push(
                settings.severity(),
                "Forked Agent Guide",
                Some(alias),
                None,
                format!("`{alias}` is a regular file whose content differs from AGENTS.md."),
                "Replace it with a symlink to AGENTS.md so agents read one set of rules.",
            );
        }
    }
    Ok(out)
}

/// Base patterns for calendar / duration estimates. Kept as data so the
/// self-test and the unit tests exercise exactly what ships.
pub fn time_estimate_patterns() -> Vec<&'static str> {
    vec![
        // "3 weeks", "1-2 days", "~10 engineer-days", "2-hour"
        r"(?i)\b\d+(?:\.\d+)?(?:\s*[-–]\s*\d+(?:\.\d+)?)?\s*-?\s*(?:(?:business|working|calendar)\s+|(?:engineer|person|man|dev)[- ])?(?:minutes?|mins?|hours?|hrs?|days?|weeks?|wks?|months?|quarters?|sprints?|years?)\b",
        // Number words: "two weeks", "three sprints"
        r"(?i)\b(?:one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve|thirteen|fourteen|fifteen|sixteen|seventeen|eighteen|nineteen|twenty|thirty|forty|fifty|sixty|seventy|eighty|ninety|hundred)(?:\s*[-–]\s*(?:one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve|thirteen|fourteen|fifteen|sixteen|seventeen|eighteen|nineteen|twenty|thirty|forty|fifty|sixty|seventy|eighty|ninety|hundred))?\s*-?\s*(?:(?:business|working|calendar)\s+|(?:engineer|person|man|dev)[- ])?(?:minutes?|mins?|hours?|hrs?|days?|weeks?|wks?|months?|quarters?|sprints?|years?)\b",
        // a/an + unit: "a month", "a week"
        r"(?i)\b(?:a|an)\s+-?\s*(?:(?:business|working|calendar)\s+|(?:engineer|person|man|dev)[- ])?(?:minute|min|hour|hr|day|week|wk|month|quarter|sprint|year)\b",
        r"(?i)\b(?:next|this|following|coming)\s+(?:sprint|week|month|quarter|(?:mon|tues|wednes|thurs|fri|satur|sun)day)\b",
        r"\bQ[1-4]\b",
        r"(?i)\b(?:engineer|person|man|dev)[- ](?:hours?|days?|weeks?|months?)\b",
        r"(?i)\bby\s+(?:mon|tues|wednes|thurs|fri|satur|sun)day\b",
        r"(?i)\bover\s+the\s+weekend\b",
    ]
}

fn compile(patterns: impl IntoIterator<Item = impl AsRef<str>>, what: &str) -> Result<Vec<Regex>> {
    patterns
        .into_iter()
        .map(|p| {
            Regex::new(p.as_ref()).with_context(|| format!("invalid {what} regex `{}`", p.as_ref()))
        })
        .collect()
}

#[derive(Default)]
struct MarkdownTableTracker {
    header_line: Option<String>,
    exempt_cols: Vec<bool>,
    in_table: bool,
}

impl MarkdownTableTracker {
    fn feed_line(&mut self, line: &str) {
        let trimmed = line.trim();
        if !trimmed.contains('|') {
            self.header_line = None;
            self.exempt_cols.clear();
            self.in_table = false;
            return;
        }

        let is_sep = {
            let cells = trimmed
                .split('|')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>();
            !cells.is_empty()
                && cells.iter().all(|c| {
                    !c.is_empty()
                        && c.chars()
                            .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
                })
        };

        if is_sep {
            if let Some(hdr) = self.header_line.take() {
                self.exempt_cols = Self::parse_exempt_columns(&hdr);
                self.in_table = true;
                return;
            }
        }

        if !self.in_table {
            self.header_line = Some(line.to_string());
        }
    }

    fn is_cell_exempt(&self, line: &str, hit_start: usize, hit_end: usize) -> bool {
        if !self.in_table || self.exempt_cols.is_empty() {
            return false;
        }
        let pipe_count = line[..hit_start].chars().filter(|&c| c == '|').count();
        let col_idx = pipe_count.saturating_sub(1);
        if !self.exempt_cols.get(col_idx).copied().unwrap_or(false) {
            return false;
        }
        if has_plan_vocabulary(line) {
            return false;
        }
        let hit = &line[hit_start..hit_end];
        let cal_re =
            Regex::new(r"(?i)\b(?:weeks?|wks?|months?|sprints?|quarters?|years?|days?)\b").unwrap();
        if cal_re.is_match(hit) {
            return false;
        }
        true
    }

    fn parse_exempt_columns(header: &str) -> Vec<bool> {
        let cells = header
            .split('|')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        let col_re = Regex::new(
            r"(?i)\b(latency|elapsed|wall(?:[- ]clock)?|runtime|run[- ]time|p\d{2,3}|(?:hosted\s+)?runner)\b",
        )
        .expect("valid regex");
        cells.into_iter().map(|c| col_re.is_match(c)).collect()
    }
}

pub(crate) fn split_into_clauses(line: &str) -> Vec<(usize, &str)> {
    let mut clauses = Vec::new();
    let mut start = 0;
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let len = chars.len();

    let mut i = 0;
    while i < len {
        let (byte_idx, ch) = chars[i];
        let mut is_delim = false;
        let mut delim_len = ch.len_utf8();

        if ch == ';' || ch == '—' {
            is_delim = true;
        } else if ch == '-' && i + 1 < len && chars[i + 1].1 == '-' {
            is_delim = true;
            delim_len = 2;
        } else if ch == '.' || ch == '!' || ch == '?' {
            if i + 1 == len {
                is_delim = true;
            } else {
                let next_ch = chars[i + 1].1;
                if next_ch.is_whitespace()
                    || next_ch == '"'
                    || next_ch == '\''
                    || next_ch == ')'
                    || next_ch == ']'
                {
                    let before = &line[start..byte_idx];
                    let is_num = before.chars().last().is_some_and(|c| c.is_ascii_digit());
                    let next_is_num = chars.get(i + 2).is_some_and(|(_, c)| c.is_ascii_digit());
                    if !(is_num && next_is_num) {
                        is_delim = true;
                    }
                }
            }
        } else if ch == ',' {
            let before = &line[start..byte_idx];
            let is_num = before.chars().last().is_some_and(|c| c.is_ascii_digit());
            let next_is_num = chars.get(i + 1).is_some_and(|(_, c)| c.is_ascii_digit());
            if !(is_num && next_is_num) {
                is_delim = true;
            }
        }

        if is_delim {
            let slice = line[start..byte_idx].trim();
            if !slice.is_empty() {
                clauses.push((start, slice));
            }
            start = byte_idx + delim_len;
            if delim_len > 1 {
                i += delim_len - 1;
            }
        }
        i += 1;
    }

    let tail = line[start..].trim();
    if !tail.is_empty() {
        clauses.push((start, tail));
    }

    if clauses.is_empty() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            clauses.push((0, trimmed));
        }
    }

    clauses
}

pub(crate) fn has_plan_vocabulary(text: &str) -> bool {
    let plan_re = Regex::new(
        r"(?i)\b(?:ship(?:s|ped|ping)?|deliver(?:s|ed|y|ing|ables?)?|land(?:s|ed|ing)?|rollout|eta|estimate(?:s|d|ing)?|effort|deadline(?:s)?|due|will|should|expect(?:s|ed|ing)?|plan(?:s|ned|ning)?|phase(?:s)?|milestone(?:s)?|sprint(?:s)?|scope|capacity|roadmap|target(?:s)?|rewrite|\d+\s+[a-z]+\s+of\s+work|capacity\s+work|work\s+limit|working\s+for|working\s+on)\b",
    )
    .unwrap();
    plan_re.is_match(text)
}

fn is_exempt_question(clause: &str, line: &str) -> bool {
    let target_re = Regex::new(
        r"(?i)\b(?:target|due|done|roadmap|release|launch|schedule|timeline|deliver|ship|plan)\b",
    )
    .unwrap();
    if target_re.is_match(clause) || target_re.is_match(line) {
        return false;
    }
    let q_re = Regex::new(
        r"(?i)(?:\b(?:question|ask|answering|quoting|faq)\b|Q[1-4]\s*[-—–:)?.]|\(Q[1-4]\)|Q[1-4]['’]s|\*\*Q[1-4]|\[Q[1-4]\]|###?\s*Q[1-4])",
    )
    .unwrap();
    q_re.is_match(clause) || q_re.is_match(line)
}

fn has_exemption_cue(clause: &str, line: &str, _matched: &str) -> bool {
    // 1. Operational limits / timeouts / caps / budgets / TTL / retention in setting forms
    let setting_re = Regex::new(
        r"(?i)\b(?:timeout(?:-minutes)?\s*[:=]\s*\d+|\d+[- ](?:second|sec|minute|min|hour|hr)[- ]timeout|capped\s+at\s+\d+|\d+[- ](?:minute|min|hour|hr|sec)[- ]cap|cap\s+of\s+\d+|limit\s+is\s+\d+|\d+[- ](?:minute|min|hour|hr|sec)[- ]limit|rate\s+limit\s+is\s+\d+|budget\s+of\s+\d+|\d+[- ](?:minute|min|hour|hr|sec)[- ]budget|retention\s+(?:is|of)\s+\d+|\d+[- ](?:day|hour|month)[- ]retention|ttl\s+(?:is\s+set\s+to|is|set\s+to)\s+\d+|\d+[- ](?:hour|day|min)[- ]ttl|interval\s+is\s+(?:every\s+)?\d+|\d+[- ](?:hour|minute|day)[- ]default|default\s+(?:is|of)\s+\d+|gap\s+between\s+runs|retention|retained|expires?|expired|cache(?:d)?|ttl|soak|uptime|window|timeout|sleep|24-hour)\b",
    )
    .unwrap();
    if setting_re.is_match(clause) {
        return true;
    }

    // 2. Frequency
    let freq_re = Regex::new(
        r"(?i)\b(?:every\s+\d+\s+(?:seconds?|secs?|minutes?|mins?|hours?|hrs?|days?|weeks?|months?)|once\s+a\s+(?:day|week|month|year)|twice\s+a\s+(?:day|week|month|year)|triggers\s+every\s+\d+|per\s+(?:day|week|month|year))\b",
    )
    .unwrap();
    if freq_re.is_match(clause) {
        return true;
    }

    // 3. Performance / measurement / runtimes / latency / benchmarks / loadavg / metrics
    let meas_re = Regex::new(
        r"(?i)\b(?:took\s+\d+|ran\s+for\s+(?:over\s+)?(?:\d+|two)\s+|elapsed[:\s]+\d+|finished\s+in\s+\d+|measured\s+elapsed\s+time[:\s]+\d+|execution\s+took\s+\d+|mean\s+runtime\s+of\s+\d+|runtime\s+(?:was|of)\s+\d+|wall[- ]clock\s+time\s+was\s+\d+|wall\s+time\s+on\s+the\s+same\s+core|nightly\s+run\s+took\s+\d+|nightly|p\d{2,3}\s+latency|load1|loadavg|load\s+average|1-min\s+(?:load\s+)?average|one-minute\s+(?:load\s+)?average|\d+[- ](?:min|minute|sec|hour)\s+\d+(?:\.\d+)?)\b",
    )
    .unwrap();
    if meas_re.is_match(clause) {
        return true;
    }

    // 4. Historical durations / ages / production stability / historical narration / commit ordering
    let hist_re = Regex::new(
        r"(?i)\b(?:\d+[- ](?:years?|months?|days?|hours?|mins?)[- ]old|(?:a|an)\s+(?:years?|months?|days?)[- ]old|\d+\s+(?:years?|months?|days?|weeks?|months?)\s+ago|a\s+day\s+ago|shipped\s+a\s+day|for\s+(?:the\s+past|the\s+last|about|over|~)?\s*\d+\s*(?:years?|months?)|stable\s+for\s+\d+|compatibility\s+for\s+(?:over\s+)?\d+|history\s+spans\s+\d+|survived\s+\d+\s+years|undetected\s+for\s+[~]?\d+\s+years|invariants?|unchecked\s+for\s+\d+|written\s+\d+\s+years\s+ago|issue\s+was\s+resolved|production\s+history\s+spans|commit\s+ordering|(?:minutes?|hours?|days?|weeks?)\s+later|(?:minutes?|hours?|days?|weeks?)\s+earlier)\b",
    )
    .unwrap();
    if hist_re.is_match(clause)
        || (hist_re.is_match(line) && line.to_lowercase().contains("commit ordering"))
    {
        return true;
    }

    // 5. Operational wrap windows / bitfield / epoch
    let wrap_re =
        Regex::new(r"(?i)\b(?:active\s+window|wrap\s+window|wrap\s+duration|bitfield|epoch)\b")
            .unwrap();
    if wrap_re.is_match(clause) || wrap_re.is_match(line) {
        return true;
    }

    // 6. Reading / media time
    let media_re = Regex::new(
        r"(?i)\b\d+[- ](?:minute|min|hour|hr)[- ](?:read|overview|talk|presentation|paper|video|podcast|description)\b",
    )
    .unwrap();
    if media_re.is_match(clause) {
        return true;
    }

    false
}

pub(crate) fn is_time_estimate_violation(
    clause: &str,
    line: &str,
    matched: &str,
    in_exempt_table_cell: bool,
) -> bool {
    if in_exempt_table_cell {
        return false;
    }
    if line.contains("://") || line.contains(".htm") || line.contains(".html") {
        return false;
    }
    if matches!(matched, "Q1" | "Q2" | "Q3" | "Q4") {
        return !is_exempt_question(clause, line);
    }
    if has_exemption_cue(clause, line, matched) {
        return false;
    }
    if has_plan_vocabulary(clause) {
        return true;
    }
    let lower_matched = matched.to_lowercase();
    let is_a_an = lower_matched.starts_with("a ")
        || lower_matched.starts_with("a-")
        || lower_matched.starts_with("an ")
        || lower_matched.starts_with("an-");
    if is_a_an && has_plan_vocabulary(line) {
        return true;
    }
    if is_a_an {
        let in_re =
            Regex::new(r"(?i)\b(?:in|takes?|taking|about|approx(?:\.|imately)?|~)\s+(?:a|an)\b")
                .unwrap();
        return in_re.is_match(clause) || in_re.is_match(line);
    }
    true
}

pub(crate) fn is_exempt_time_estimate(
    line: &str,
    hit_start: usize,
    hit_end: usize,
    matched: &str,
) -> bool {
    let clauses = split_into_clauses(line);
    for (clause_start, clause) in &clauses {
        let clause_end = clause_start + clause.len();
        if hit_start >= *clause_start && hit_end <= clause_end {
            return !is_time_estimate_violation(clause, line, matched, false);
        }
    }
    !is_time_estimate_violation(line, line, matched, false)
}

pub(crate) fn scan_text_for_time_estimates(
    text: &str,
    banned: &[Regex],
    allowed: &[Regex],
) -> Vec<(usize, String)> {
    let mut violations = Vec::new();
    let mut fence: Option<&str> = None;
    let mut table = MarkdownTableTracker::default();

    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if let Some(m) = ["```", "~~~"].into_iter().find(|m| trimmed.starts_with(m)) {
            fence = match fence {
                None => Some(m),
                Some(open) if open == m => None,
                keep => keep,
            };
            continue;
        }
        if fence.is_some() {
            continue;
        }

        table.feed_line(line);

        if line_allows(line, "time-estimates") {
            continue;
        }
        if allowed.iter().any(|re| re.is_match(line)) {
            continue;
        }

        let clauses = split_into_clauses(line);
        let mut seen_spans: Vec<(usize, usize)> = Vec::new();
        for (clause_start, clause) in clauses {
            for re in banned {
                for hit in re.find_iter(clause) {
                    let abs_start = clause_start + hit.start();
                    let abs_end = clause_start + hit.end();
                    if seen_spans
                        .iter()
                        .any(|(s, e)| !(abs_end <= *s || abs_start >= *e))
                    {
                        continue;
                    }
                    let in_exempt_cell = table.is_cell_exempt(line, abs_start, abs_end);
                    if is_time_estimate_violation(clause, line, hit.as_str(), in_exempt_cell) {
                        seen_spans.push((abs_start, abs_end));
                        violations.push((idx + 1, hit.as_str().to_string()));
                    }
                }
            }
        }
    }
    violations
}

pub fn time_estimates(ctx: &Context) -> Result<GateOutcome> {
    const GATE: &str = "time-estimates";
    let settings = &ctx.config.gates.time_estimates;
    let exempt = exempt_filter(settings)?;
    let include = PathFilter::new(&settings.include)?;
    let mut banned = compile(time_estimate_patterns(), "built-in")?;
    banned.extend(compile(&settings.extra_patterns, "extra_patterns")?);
    let allowed = compile(&settings.allow_patterns, "allow_patterns")?;
    let mut out = GateOutcome::new(GATE);

    let scan = |label: &str,
                text: &str,
                added_lines: Option<&std::collections::BTreeSet<usize>>,
                out: &mut GateOutcome| {
        let mut fence: Option<&str> = None;
        for (idx, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if let Some(m) = ["```", "~~~"].into_iter().find(|m| trimmed.starts_with(m)) {
                fence = match fence {
                    None => Some(m),
                    Some(open) if open == m => None,
                    keep => keep,
                };
                continue;
            }
            if fence.is_some() {
                continue;
            }
            let line_num = idx + 1;
            let in_scope = added_lines.is_none_or(|lines| lines.contains(&line_num));
            if in_scope && line_allows(line, GATE) && banned.iter().any(|re| re.is_match(line)) {
                out.inline_exemptions += 1;
                out.overrides.push(crate::tokens::OverrideRecord {
                    gate: GATE.to_string(),
                    subject: format!("{label}:{}", idx + 1),
                    directive: if line.contains("docs-lint: allow") {
                        "docs-lint: allow".to_string()
                    } else {
                        format!("discipline:allow({GATE})")
                    },
                    reason: "inline exemption marker".to_string(),
                    source: crate::tokens::OverrideSource::Inline {
                        file: label.to_string(),
                        line: idx + 1,
                    },
                    hidden: true,
                });
            }
        }

        let violations = scan_text_for_time_estimates(text, &banned, &allowed);
        for (line_num, hit_str) in violations {
            if let Some(lines) = added_lines {
                if !lines.contains(&line_num) {
                    continue;
                }
            }
            out.push(
                settings.severity(),
                "Time Estimate",
                Some(label),
                Some(line_num),
                format!("Calendar / duration estimate `{}`.", hit_str),
                "Replace it with ordering, dependencies, or a gate criterion. A line that \
                 must quote the term can carry `<!-- discipline:allow(time-estimates) -->` or `docs-lint: allow`.",
            );
        }
    };

    if settings.diff_only {
        let changed = ctx.git.changed_files()?;
        for f in &changed {
            if f.is_deleted() || !include.matches(&f.path) || exempt.matches(&f.path) {
                continue;
            }
            match ctx.git.head_content(&f.path)? {
                Some(text) => {
                    out.examined += 1;
                    scan(&f.path, &text, Some(&f.added_lines), &mut out);
                }
                None => out
                    .notes
                    .push(format!("skipped `{}` (binary file)", f.path)),
            }
        }
    } else {
        for path in ctx.git.tracked_files()? {
            if !include.matches(&path) || exempt.matches(&path) {
                continue;
            }
            match ctx.git.head_content(&path)? {
                Some(text) => {
                    out.examined += 1;
                    scan(&path, &text, None, &mut out);
                }
                None => out.notes.push(format!("skipped `{path}` (binary file)")),
            }
        }
    }
    let mut scan_body = |label: &str, text: &str, out: &mut GateOutcome| {
        scan(label, text, None, out);
    };
    scan_pr_body(ctx, settings.scan_pr_body, &mut out, &mut scan_body);
    Ok(out)
}

fn scan_pr_body(
    ctx: &Context,
    wanted: bool,
    out: &mut GateOutcome,
    scan: &mut dyn FnMut(&str, &str, &mut GateOutcome),
) {
    if !wanted {
        return;
    }
    match &ctx.pr_body {
        Some(body) => {
            out.examined += 1;
            scan("<PR body>", body, out);
        }
        None => out
            .notes
            .push("no PR body supplied; PR-body scan did not run".to_string()),
    }
}

pub struct PiiRule {
    pub re: Regex,
    pub label: &'static str,
    /// Capture group holding a user name to test against `allowed_users`.
    pub user_group: bool,
    /// Echoing the match would repeat the secret in CI logs.
    pub redact: bool,
}

pub fn pii_rules(settings: &PiiGate) -> Result<Vec<PiiRule>> {
    let mut rules = Vec::new();
    if settings.home_paths {
        // Built from pieces so this source file does not match its own rule.
        let unix = format!(r"/(?:{}|{})/([A-Za-z0-9][A-Za-z0-9._-]*)", "Users", "home");
        let windows = format!(r"(?i)\b[A-Z]:\\{}\\([A-Za-z0-9][A-Za-z0-9._-]*)", "Users");
        for p in [unix, windows] {
            rules.push(PiiRule {
                re: Regex::new(&p)?,
                label: "home-directory path",
                user_group: true,
                redact: true,
            });
        }
    }
    if settings.lan_ips {
        rules.push(PiiRule {
            re: Regex::new(
                r"\b(?:10\.\d{1,3}|192\.168|172\.(?:1[6-9]|2\d|3[01]))\.\d{1,3}\.\d{1,3}\b",
            )?,
            label: "private LAN address",
            user_group: false,
            redact: false,
        });
    }
    if settings.secrets {
        rules.push(PiiRule {
            re: Regex::new(r"-----BEGIN (?:[A-Z0-9_-]+ )?PRIVATE KEY-----")?,
            label: "private key header",
            user_group: false,
            redact: true,
        });
        rules.push(PiiRule {
            re: Regex::new(r"\b(?:AKIA|ASIA|ABIA|ACCA)[0-9A-Z]{16}\b")?,
            label: "AWS access key ID",
            user_group: false,
            redact: true,
        });
        rules.push(PiiRule {
            re: Regex::new(r"\bgh[pousr]_[A-Za-z0-9_]{36,255}\b")?,
            label: "GitHub personal access token",
            user_group: false,
            redact: true,
        });
        rules.push(PiiRule {
            re: Regex::new(r"\bxox[baprs]-[0-9]{10,13}-[0-9]{10,13}-[a-zA-Z0-9]{24,32}\b")?,
            label: "Slack token",
            user_group: false,
            redact: true,
        });
    }
    if settings.agent_config_refs {
        let agent_dirs = [
            "claude", "gemini", "codex", "cursor", "aider", "copilot", "continue",
        ]
        .join("|");
        rules.push(PiiRule {
            re: Regex::new(&format!(r"(?i)(?:~|\$HOME)/\.(?:{agent_dirs})\b"))?,
            label: "home agent-config path",
            user_group: false,
            redact: false,
        });
        let res_disc = format!(r"\b{}{}\b", "RESEARCH_DISCIPLINES", r"\.md");
        rules.push(PiiRule {
            re: Regex::new(&res_disc)?,
            label: "personal methodology doc",
            user_group: false,
            redact: false,
        });
        let playbook = format!(r"\b{}{}\b", r"[A-Z0-9_]*_PLAYBOOK", r"\.md");
        rules.push(PiiRule {
            re: Regex::new(&playbook)?,
            label: "personal playbook",
            user_group: false,
            redact: false,
        });
    }
    for host in &settings.hostname_denylist {
        let host = host.trim();
        if host.is_empty() {
            continue;
        }
        rules.push(PiiRule {
            re: Regex::new(&format!(
                r"(?i)(?:^|[^A-Za-z0-9-]){}(?:$|[^A-Za-z0-9-])",
                regex::escape(host)
            ))?,
            label: "denylisted hostname",
            user_group: false,
            redact: true,
        });
    }
    for p in &settings.extra_patterns {
        rules.push(PiiRule {
            re: Regex::new(p).with_context(|| format!("invalid pii extra_patterns regex `{p}`"))?,
            label: "configured pattern",
            user_group: false,
            redact: true,
        });
    }
    Ok(rules)
}

pub(crate) fn is_exempt_lan_ip(
    ip_str: &str,
    line: &str,
    _match_start: usize,
    match_end: usize,
) -> bool {
    let parts: Vec<&str> = ip_str.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    let Ok(d) = parts[3].parse::<u8>() else {
        return false;
    };

    // If the last octet is 0 (network address) or 255 (broadcast address), it's a network definition, not a host IP.
    if d == 0 || d == 255 {
        return true;
    }

    // Check for CIDR mask (e.g. /8, /12, /16, /24)
    let after = &line[match_end..];
    if let Some(rest) = after.strip_prefix('/') {
        let mask_str = rest
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>();
        if let Ok(prefix_len) = mask_str.parse::<u32>() {
            if (1..=32).contains(&prefix_len) {
                if let (Ok(o0), Ok(o1), Ok(o2)) = (
                    parts[0].parse::<u32>(),
                    parts[1].parse::<u32>(),
                    parts[2].parse::<u32>(),
                ) {
                    let ip_num = (o0 << 24) | (o1 << 16) | (o2 << 8) | (d as u32);
                    let mask = if prefix_len == 0 {
                        0
                    } else {
                        !0u32 << (32 - prefix_len)
                    };
                    if (ip_num & !mask) == 0 {
                        return true;
                    }
                }
            }
        }
    }

    false
}

fn collect_json_strings<'a>(val: &'a serde_json::Value, out: &mut Vec<&'a str>) {
    match val {
        serde_json::Value::String(s) => out.push(s.as_str()),
        serde_json::Value::Array(arr) => {
            for item in arr {
                collect_json_strings(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            for (key, v) in map {
                out.push(key.as_str());
                collect_json_strings(v, out);
            }
        }
        _ => {}
    }
}

struct PiiScanOptions<'a> {
    label: &'a str,
    rules: &'a [PiiRule],
    settings: &'a PiiGate,
    allowed: &'a [Regex],
    is_active_config: bool,
    added_lines: Option<&'a std::collections::BTreeSet<usize>>,
}

fn scan_json(opts: &PiiScanOptions<'_>, text: &str, out: &mut GateOutcome) -> bool {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(text) else {
        return false;
    };

    let mut tokens = Vec::new();
    collect_json_strings(&val, &mut tokens);

    for token in tokens {
        for rule in opts.rules {
            if opts.is_active_config && rule.label == "denylisted hostname" {
                continue;
            }
            for caps in rule.re.captures_iter(token) {
                let m = caps.get(0).unwrap();
                if rule.label == "private LAN address"
                    && is_exempt_lan_ip(m.as_str(), token, m.start(), m.end())
                {
                    continue;
                }
                let allowed_user = rule.user_group
                    && caps.get(1).is_some_and(|u| {
                        opts.settings
                            .allowed_users
                            .iter()
                            .any(|a| a.eq_ignore_ascii_case(u.as_str()))
                    });
                if allowed_user {
                    continue;
                }
                if opts.allowed.iter().any(|re| re.is_match(token)) {
                    continue;
                }
                let escaped_token = token.replace('/', r"\/");
                let matching_lines: Vec<usize> = text
                    .lines()
                    .enumerate()
                    .filter(|(_, l)| l.contains(token) || l.contains(&escaped_token))
                    .map(|(idx, _)| idx + 1)
                    .collect();
                let candidate_lines = if matching_lines.is_empty() {
                    vec![1]
                } else {
                    matching_lines
                };
                let mut reported = false;
                for line_num in candidate_lines {
                    if let Some(line) = text.lines().nth(line_num.saturating_sub(1)) {
                        if line_allows(line, "pii") {
                            continue;
                        }
                    }
                    if let Some(lines) = opts.added_lines {
                        if !lines.contains(&line_num) {
                            continue;
                        }
                    }
                    let detail = match (rule.redact, m.as_str()) {
                        (false, s) => format!("{} `{s}`", rule.label),
                        _ => format!("{} (match not echoed)", rule.label),
                    };
                    out.push(
                        opts.settings.severity(),
                        "Host / PII Leak",
                        Some(opts.label),
                        Some(line_num),
                        format!("Found a {detail}."),
                        "Replace it with a placeholder such as `<home>` or `<host>`. A line that must \
                         keep it can carry `discipline:allow(pii)` or `docs-lint: allow`.",
                    );
                    reported = true;
                    break;
                }
                if reported {
                    break;
                }
            }
        }
    }
    true
}

pub fn pii(ctx: &Context) -> Result<GateOutcome> {
    const GATE: &str = "pii";
    let settings = &ctx.config.gates.pii;
    let exempt = exempt_filter(settings)?;
    let rules = pii_rules(settings)?;
    let allowed = compile(&settings.allow_patterns, "allow_patterns")?;
    let mut out = GateOutcome::new(GATE);
    let mut binary = 0usize;

    if settings.hostname_denylist.is_empty() {
        out.notes.push(
            "hostname denylist is empty or unset; hostname leak check skipped \
             (set DISCIPLINE_HOSTNAME_DENYLIST or DOCS_HOSTNAME_DENYLIST as a repository secret)"
                .to_string(),
        );
    }

    let is_active_config = |label: &str| {
        let l = label.trim_start_matches("./");
        let c = ctx.config_path.trim_start_matches("./");
        l == c || l == "discipline.toml"
    };

    let scan = |label: &str,
                text: &str,
                added_lines: Option<&std::collections::BTreeSet<usize>>,
                out: &mut GateOutcome| {
        let active_cfg = is_active_config(label);
        for (idx, line) in text.lines().enumerate() {
            let line_num = idx + 1;
            if let Some(lines) = added_lines {
                if !lines.contains(&line_num) {
                    continue;
                }
            }
            let hit = rules.iter().find_map(|rule| {
                if active_cfg && rule.label == "denylisted hostname" {
                    return None;
                }
                rule.re.captures_iter(line).find_map(|caps| {
                    let m = caps.get(0).unwrap();
                    if rule.label == "private LAN address"
                        && is_exempt_lan_ip(m.as_str(), line, m.start(), m.end())
                    {
                        return None;
                    }
                    let allowed_user = rule.user_group
                        && caps.get(1).is_some_and(|u| {
                            settings
                                .allowed_users
                                .iter()
                                .any(|a| a.eq_ignore_ascii_case(u.as_str()))
                        });
                    (!allowed_user).then(|| (rule, caps.get(0).map(|m| m.as_str().to_string())))
                })
            });
            let Some((rule, matched)) = hit else { continue };
            if line_allows(line, GATE) {
                out.inline_exemptions += 1;
                out.overrides.push(crate::tokens::OverrideRecord {
                    gate: GATE.to_string(),
                    subject: format!("{label}:{}", idx + 1),
                    directive: if line.contains("docs-lint: allow") {
                        "docs-lint: allow".to_string()
                    } else {
                        format!("discipline:allow({GATE})")
                    },
                    reason: "inline exemption marker".to_string(),
                    source: crate::tokens::OverrideSource::Inline {
                        file: label.to_string(),
                        line: idx + 1,
                    },
                    hidden: true,
                });
                continue;
            }
            if allowed.iter().any(|re| re.is_match(line)) {
                continue;
            }
            let detail = match (rule.redact, matched) {
                (false, Some(m)) => format!("{} `{m}`", rule.label),
                _ => format!("{} (match not echoed)", rule.label),
            };
            out.push(
                settings.severity(),
                "Host / PII Leak",
                Some(label),
                Some(idx + 1),
                format!("Found a {detail}."),
                "Replace it with a placeholder such as `<home>` or `<host>`. A line that must \
                 keep it can carry `discipline:allow(pii)` or `docs-lint: allow`.",
            );
        }
    };

    if settings.diff_only {
        let changed = ctx.git.changed_files()?;
        for f in &changed {
            if f.is_deleted() || exempt.matches(&f.path) {
                continue;
            }
            match ctx.git.head_content(&f.path)? {
                Some(text) => {
                    out.examined += 1;
                    let opts = PiiScanOptions {
                        label: &f.path,
                        rules: &rules,
                        settings,
                        allowed: &allowed,
                        is_active_config: is_active_config(&f.path),
                        added_lines: Some(&f.added_lines),
                    };
                    if f.path.ends_with(".json") && scan_json(&opts, &text, &mut out) {
                        continue;
                    }
                    scan(&f.path, &text, Some(&f.added_lines), &mut out);
                }
                None => binary += 1,
            }
        }
    } else {
        for path in ctx.git.tracked_files()? {
            if exempt.matches(&path) {
                continue;
            }
            match ctx.git.head_content(&path)? {
                Some(text) => {
                    out.examined += 1;
                    let opts = PiiScanOptions {
                        label: &path,
                        rules: &rules,
                        settings,
                        allowed: &allowed,
                        is_active_config: is_active_config(&path),
                        added_lines: None,
                    };
                    if path.ends_with(".json") && scan_json(&opts, &text, &mut out) {
                        continue;
                    }
                    scan(&path, &text, None, &mut out);
                }
                None => binary += 1,
            }
        }
    }
    if binary > 0 {
        out.notes
            .push(format!("{binary} binary file(s) not scanned"));
    }
    let mut scan_body = |label: &str, text: &str, out: &mut GateOutcome| {
        scan(label, text, None, out);
    };
    scan_pr_body(ctx, settings.scan_pr_body, &mut out, &mut scan_body);
    Ok(out)
}

pub fn agent_scratch(ctx: &Context) -> Result<GateOutcome> {
    const GATE: &str = "agent-scratch";
    let settings = &ctx.config.gates.agent_scratch;
    let exempt = exempt_filter(settings)?;
    let forbidden = PathFilter::new(&settings.paths)?;
    let mut out = GateOutcome::new(GATE);

    for path in ctx.git.tracked_files()? {
        out.examined += 1;
        if forbidden.matches(&path) && !exempt.matches(&path) {
            out.push(
                settings.severity(),
                "Tracked Agent Scratch State",
                Some(&path),
                None,
                format!("`{path}` matches a forbidden agent-scratch pattern and is tracked."),
                "Untrack it with `git rm --cached` and add the pattern to .gitignore.",
            );
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn any_match(patterns: &[Regex], line: &str) -> bool {
        patterns.iter().any(|re| re.is_match(line))
    }

    #[test]
    fn time_estimate_patterns_discriminate() {
        let banned = compile(time_estimate_patterns(), "t").unwrap();
        for bad in [
            "Phase 2 (1 week)",
            "ships in 1-2 days",
            "~10 engineer-days",
            "roughly 3 weeks of work",
            "done next sprint",
            "target: Q2",
            "a 2-hour task",
            "finish by Friday",
            "over the weekend",
        ] {
            assert!(any_match(&banned, bad), "missed: {bad}");
        }
        for good in [
            "Phase 1 then Phase 2",
            "blocked on the AST engine",
            "sub-50ms startup (target)",
            "version 1.80.0",
            "SHA256SUMS",
            "smallest of the three",
        ] {
            assert!(!any_match(&banned, good), "false positive: {good}");
        }
    }

    #[test]
    fn contextual_exemptions_discriminate() {
        // Operational / measurement contexts are exempt
        let exempt_cases = [
            ("artifact retention is 30 days", "30 days"),
            ("each shard with its own 180-minute budget", "180-minute"),
            ("cancelled at its 60-minute cap", "60-minute"),
            ("GitHub's 6-hour default", "6-hour"),
            ("the 6-hour gap between runs", "6-hour"),
            ("ran for over two hours of wall time", "two hours"),
            ("that is a 1-min average decaying", "1-min"),
            ("5-min 1.32, 15-min 0.47", "5-min"),
            ("20-year-old C library", "20-year"),
            ("invariants unchecked for 20 years", "20 years"),
            ("the 20-year invariants hold", "20-year"),
            ("bug survived 19 years", "19 years"),
            ("went undetected for ~19 years", "19 years"),
            ("**Q1 — what do our patches buy?**", "Q1"),
            ("### Q2: Why judy?", "Q2"),
            ("Q3. How does this scale?", "Q3"),
            ("only Q1's is ours", "Q1"),
            ("Quoting Q2 as", "Q2"),
            (
                "A 10-Minute Description of How Judy Arrays Work",
                "10-Minute",
            ),
        ];
        for (line, hit) in exempt_cases {
            let start = line.find(hit).unwrap();
            let end = start + hit.len();
            assert!(
                is_exempt_time_estimate(line, start, end, hit),
                "expected exempt: {line} (hit: {hit})"
            );
        }

        // Planning / scheduling estimates are NOT exempt
        let non_exempt_cases = [
            ("Ship in 3 weeks; update cache ttl.", "3 weeks"),
            ("Target: Q2.", "Q2"),
            ("Done in Q3.", "Q3"),
            ("Working for 3 weeks on migration.", "3 weeks"),
            ("Phase 2 (1 week)", "1 week"),
            ("ships in 1-2 days", "1-2 days"),
            ("~10 engineer-days", "10 engineer-days"),
        ];
        for (line, hit) in non_exempt_cases {
            let start = line.find(hit).unwrap();
            let end = start + hit.len();
            assert!(
                !is_exempt_time_estimate(line, start, end, hit),
                "expected violation (not exempt): {line} (hit: {hit})"
            );
        }
    }

    #[test]
    fn time_estimates_corpus_has_zero_false_positives_and_zero_false_negatives() {
        #[derive(serde::Deserialize)]
        struct TestCase {
            id: String,
            text: String,
            is_violation: bool,
            category: String,
            description: String,
        }

        let corpus_raw = include_str!("../../tests/fixtures/time_estimates_corpus.json");
        let cases: Vec<TestCase> = serde_json::from_str(corpus_raw).expect("valid corpus json");
        assert_eq!(cases.len(), 109, "corpus must contain exactly 109 cases");
        let tp_count = cases.iter().filter(|c| c.is_violation).count();
        let tn_count = cases.iter().filter(|c| !c.is_violation).count();
        assert!(
            tp_count >= 40,
            "must have >= 40 true positives, got {tp_count}"
        );
        assert!(
            tn_count >= 40,
            "must have >= 40 true negatives, got {tn_count}"
        );

        let banned = compile(time_estimate_patterns(), "banned").unwrap();
        let allowed = compile(Vec::<String>::new(), "allowed").unwrap();

        let mut false_positives = Vec::new();
        let mut false_negatives = Vec::new();

        for case in &cases {
            let hits = scan_text_for_time_estimates(&case.text, &banned, &allowed);
            let detected = !hits.is_empty();
            if case.is_violation && !detected {
                false_negatives.push(format!(
                    "{} ({}): `{}` - {}",
                    case.id, case.category, case.text, case.description
                ));
            } else if !case.is_violation && detected {
                false_positives.push(format!(
                    "{} ({}): `{}` (hits: {:?}) - {}",
                    case.id, case.category, case.text, hits, case.description
                ));
            }
        }

        assert!(
            false_positives.is_empty() && false_negatives.is_empty(),
            "Corpus evaluation failed!\nFalse Positives ({}):\n{}\nFalse Negatives ({}):\n{}",
            false_positives.len(),
            false_positives.join("\n"),
            false_negatives.len(),
            false_negatives.join("\n")
        );
    }

    fn rule_hits(settings: &PiiGate, line: &str) -> bool {
        pii_rules(settings).unwrap().iter().any(|r| {
            r.re.captures_iter(line).any(|c| {
                !(r.user_group
                    && c.get(1).is_some_and(|u| {
                        settings
                            .allowed_users
                            .iter()
                            .any(|a| a.eq_ignore_ascii_case(u.as_str()))
                    }))
            })
        })
    }

    #[test]
    fn pii_rules_discriminate() {
        let s = PiiGate::default();
        let home = format!("/{}/alice/project", "Users");
        let linux = format!("/{}/bob/.cache", "home");
        let win = format!(r"C:\{}\carol\x", "Users");
        let ip = ["192", "168", "1", "20"].join(".");
        for bad in [home.as_str(), linux.as_str(), win.as_str(), ip.as_str()] {
            assert!(rule_hits(&s, bad), "missed: {bad}");
        }
        let runner = format!("/{}/runner/work", "home");
        let placeholder = format!("/{}/<user>/x", "Users");
        for good in [runner.as_str(), placeholder.as_str(), "8.8.8.8", "v10.2.3"] {
            assert!(!rule_hits(&s, good), "false positive: {good}");
        }
    }

    #[test]
    fn hostname_denylist_is_whole_token_and_case_insensitive() {
        let s = PiiGate {
            hostname_denylist: vec!["buildbox".into()],
            ..PiiGate::default()
        };
        assert!(rule_hits(&s, "measured on BuildBox, 8 cores"));
        assert!(rule_hits(&s, "buildbox"));
        assert!(!rule_hits(&s, "the buildboxes are idle"));
        assert!(!rule_hits(&s, "my-buildbox-2"));
        assert!(!rule_hits(&PiiGate::default(), "measured on buildbox"));
    }

    #[test]
    fn toggles_and_extra_patterns_change_the_rule_set() {
        let off = PiiGate {
            home_paths: false,
            lan_ips: false,
            secrets: false,
            agent_config_refs: false,
            ..PiiGate::default()
        };
        assert!(pii_rules(&off).unwrap().is_empty());
        let secrets_only = PiiGate {
            home_paths: false,
            lan_ips: false,
            secrets: true,
            agent_config_refs: false,
            ..PiiGate::default()
        };
        assert_eq!(pii_rules(&secrets_only).unwrap().len(), 4);
        let agent_cfg_only = PiiGate {
            home_paths: false,
            lan_ips: false,
            secrets: false,
            agent_config_refs: true,
            ..PiiGate::default()
        };
        assert_eq!(pii_rules(&agent_cfg_only).unwrap().len(), 3);
        let extra = PiiGate {
            extra_patterns: vec![r"[a-z]+@corp\.example".into()],
            ..off.clone()
        };
        assert!(rule_hits(&extra, "mail dana@corp.example"));
        let bad = PiiGate {
            extra_patterns: vec!["(".into()],
            ..off
        };
        assert!(pii_rules(&bad).is_err());
    }

    #[test]
    fn terms_of_art_and_historical_narration_exemptions() {
        let banned = compile(time_estimate_patterns(), "banned").unwrap();
        let allowed = compile(Vec::<String>::new(), "allowed").unwrap();

        let exempt = [
            "The nightly cache has a 7 days retention.",
            "Bitfield ~6.06 days active window",
            "forty minutes later — a commit ordering",
            "one-minute load average",
            "1-min average decaying",
            "`load1` metric",
            "shipped a day ago",
            "planned for 2 weeks docs-lint: allow",
        ];
        for line in exempt {
            let hits = scan_text_for_time_estimates(line, &banned, &allowed);
            assert!(
                hits.is_empty(),
                "expected line to be exempt, got hits {hits:?}: {line}"
            );
        }

        let non_exempt = [
            "Ship v0.1 (1-2 days).",
            "planned for 2 weeks",
            "delivery in 3 weeks",
        ];
        for line in non_exempt {
            let hits = scan_text_for_time_estimates(line, &banned, &allowed);
            assert!(
                !hits.is_empty(),
                "expected line to be flagged, got no hits: {line}"
            );
        }
    }

    #[test]
    fn agent_config_refs_detection() {
        let s = PiiGate {
            home_paths: false,
            lan_ips: false,
            secrets: false,
            agent_config_refs: true,
            ..PiiGate::default()
        };
        // Positives (must fail) - constructed at runtime per documented pattern
        let bad = [
            format!("with unit tests in {}{}{}", "~", "/.claude/", "CLAUDE.md"),
            format!("follow {}{}{}", "$HOME", "/.gemini/", "GEMINI.md for style"),
            format!("Per {}{}", "RESEARCH_DISCIPLINES", ".md Rule 1"),
            format!("see {}{}", "PAPER_PUBLISHING_PLAYBOOK", ".md"),
        ];
        for b in &bad {
            assert!(rule_hits(&s, b), "expected leak to be flagged: {b}");
        }
        // Negatives (must pass)
        let good = [
            "export PATH=$HOME/.cargo/bin:$PATH",
            "AGENTS.md is the canonical guide",
        ];
        for g in good {
            assert!(!rule_hits(&s, g), "expected clean line to pass: {g}");
        }
    }

    #[test]
    fn test_functions_are_scanned_for_pii() {
        let rules = pii_rules(&PiiGate::default()).unwrap();
        let py_test_path = format!("    fake_path = \"/{}/{}/repo/\"", "Users", "someone");
        let py_test_ip = format!("    fake_ip = \"{}.{}.1.20\"", "192", "168");
        assert!(rules.iter().any(|r| r.re.is_match(&py_test_path)));
        assert!(rules.iter().any(|r| r.re.is_match(&py_test_ip)));
    }

    #[test]
    fn json_escaped_slashes_scanned() {
        let text = "{\n  \"bin\": \"\\/home\\/someuser\\/bin\\/x\"\n}\n";
        let rules = pii_rules(&PiiGate::default()).unwrap();
        let allowed = vec![];
        let mut out = GateOutcome::new("pii");
        let opts = PiiScanOptions {
            label: "results/escaped.json",
            rules: &rules,
            settings: &PiiGate::default(),
            allowed: &allowed,
            is_active_config: false,
            added_lines: None,
        };
        assert!(scan_json(&opts, text, &mut out));
        assert_eq!(out.violations.len(), 1);
        assert_eq!(out.violations[0].line, Some(2));
    }
}
