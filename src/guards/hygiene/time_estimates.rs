//! `time-estimates`: durations and calendar projections in tracked text and in the pull
//! request body.

use super::{compile, scan_pr_body};
use crate::config::GateSettings;
use crate::guards::{exempt_filter, line_allows, Context, GateOutcome, PathFilter};
use anyhow::Result;
use regex::Regex;

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
        if !line.is_char_boundary(hit_start)
            || !line.is_char_boundary(hit_end)
            || hit_start > hit_end
            || hit_end > line.len()
        {
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
        let cal_re = Regex::new(r"(?i)\b(?:weeks?|wks?|months?|sprints?|quarters?|years?|days?)\b")
            .expect("valid regex");
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
        let mut delim_bytes = ch.len_utf8();
        let mut delim_chars = 1;

        if ch == ';' || ch == '—' {
            is_delim = true;
        } else if ch == '-' && i + 1 < len && chars[i + 1].1 == '-' {
            is_delim = true;
            delim_bytes = 2;
            delim_chars = 2;
        } else if ch == '.' || ch == '!' || ch == '?' {
            if i + 1 == len {
                is_delim = true;
            } else {
                let next_ch = chars[i + 1].1;
                if (next_ch.is_whitespace()
                    || next_ch == '"'
                    || next_ch == '\''
                    || next_ch == ')'
                    || next_ch == ']')
                    && start <= byte_idx
                {
                    let before = &line[start..byte_idx];
                    let is_num = before.chars().last().is_some_and(|c| c.is_ascii_digit());
                    let next_is_num = chars.get(i + 2).is_some_and(|(_, c)| c.is_ascii_digit());
                    if !(is_num && next_is_num) {
                        is_delim = true;
                    }
                }
            }
        } else if ch == ',' && start <= byte_idx {
            let before = &line[start..byte_idx];
            let is_num = before.chars().last().is_some_and(|c| c.is_ascii_digit());
            let next_is_num = chars.get(i + 1).is_some_and(|(_, c)| c.is_ascii_digit());
            if !(is_num && next_is_num) {
                is_delim = true;
            }
        }

        if is_delim {
            if start <= byte_idx {
                let raw_slice = &line[start..byte_idx];
                let trimmed = raw_slice.trim();
                if !trimmed.is_empty() {
                    let offset = trimmed.as_ptr() as usize - line.as_ptr() as usize;
                    clauses.push((offset, trimmed));
                }
            }
            start = byte_idx + delim_bytes;
            if delim_chars > 1 {
                i += delim_chars - 1;
            }
        }
        i += 1;
    }

    if start < line.len() {
        let raw_tail = &line[start..];
        let trimmed = raw_tail.trim();
        if !trimmed.is_empty() {
            let offset = trimmed.as_ptr() as usize - line.as_ptr() as usize;
            clauses.push((offset, trimmed));
        }
    }

    if clauses.is_empty() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            let offset = trimmed.as_ptr() as usize - line.as_ptr() as usize;
            clauses.push((offset, trimmed));
        }
    }

    clauses
}

pub(crate) fn has_plan_vocabulary(text: &str) -> bool {
    let plan_re = Regex::new(
        r"(?i)\b(?:ship(?:s|ped|ping)?|deliver(?:s|ed|y|ing|ables?)?|land(?:s|ed|ing)?|rollout|eta|estimate(?:s|d|ing)?|effort|deadline(?:s)?|due|will|should|expect(?:s|ed|ing)?|plan(?:s|ned|ning)?|phase(?:s)?|milestone(?:s)?|sprint(?:s)?|scop(?:e|es|ed|ing)|capacity|roadmap|target(?:s)?|rewrite|\d+\s+[a-z]+\s+of\s+work|capacity\s+work|work\s+limit|working\s+for|working\s+on)\b",
    )
    .expect("valid regex");
    plan_re.is_match(text)
}

fn is_exempt_question(clause: &str, line: &str) -> bool {
    let target_re = Regex::new(
        r"(?i)\b(?:target|due|done|roadmap|release|launch|schedule|timeline|deliver|ship|plan)\b",
    )
    .expect("valid regex");
    if target_re.is_match(clause) || target_re.is_match(line) {
        return false;
    }
    let q_re = Regex::new(
        r"(?i)(?:\b(?:question|ask|answering|quoting|faq)\b|Q[1-4]\s*[-—–:)?.]|\(Q[1-4]\)|Q[1-4]['\u{2019}]s|\*\*Q[1-4]|\[Q[1-4]\]|###?\s*Q[1-4])",
    )
    .expect("valid regex");
    q_re.is_match(clause) || q_re.is_match(line)
}

/// A count written in digits or as a word: `5`, `14.5`, `five`, `hundred`.
const COUNT: &str = r"(?:\d+(?:\.\d+)?|one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve|thirteen|fourteen|fifteen|sixteen|seventeen|eighteen|nineteen|twenty|thirty|forty|fifty|sixty|seventy|eighty|ninety|hundred)";

fn has_exemption_cue(clause: &str, line: &str, _matched: &str) -> bool {
    // 1. Operational limits / timeouts / caps / budgets / TTL / retention in setting forms
    let setting_re = Regex::new(
        r"(?i)\b(?:timeout(?:-minutes)?\s*[:=]\s*\d+|\d+[- ](?:second|sec|minute|min|hour|hr)[- ]timeout|capped\s+at\s+\d+|\d+[- ](?:minute|min|hour|hr|sec)[- ]cap|cap\s+of\s+\d+|limit\s+is\s+\d+|\d+[- ](?:minute|min|hour|hr|sec)[- ]limit|rate\s+limit\s+is\s+\d+|budget\s+of\s+\d+|\d+[- ](?:minute|min|hour|hr|sec)[- ]budget|retention\s+(?:is|of)\s+\d+|\d+[- ](?:day|hour|month)[- ]retention|ttl\s+(?:is\s+set\s+to|is|set\s+to)\s+\d+|\d+[- ](?:hour|day|min)[- ]ttl|interval\s+is\s+(?:every\s+)?\d+|\d+[- ](?:minute|hour|day)[- ](?:run|test|burn-in)|\d+[- ](?:hour|minute|day)[- ]default|default\s+(?:is|of)\s+\d+|gap\s+between\s+runs|retention|retained|expires?|expired|cache(?:d)?|ttl|soak|uptime|window|timeout|sleep|24-hour)\b",
    )
    .expect("valid regex");
    if setting_re.is_match(clause) {
        return true;
    }

    // 2. Frequency
    let freq_re = Regex::new(&format!(
        r"(?i)\b(?:every\s+{COUNT}\s+(?:seconds?|secs?|minutes?|mins?|hours?|hrs?|days?|weeks?|months?)|once\s+a\s+(?:day|week|month|year)|twice\s+a\s+(?:day|week|month|year)|triggers\s+every\s+\d+|per\s+(?:day|week|month|year)|within\s+{COUNT}\s+(?:seconds?|secs?|minutes?|mins?))\b",
    ))
    .expect("valid regex");
    if freq_re.is_match(clause) {
        return true;
    }

    // 3. Performance / measurement / runtimes / latency / benchmarks / loadavg / metrics
    let meas_re = Regex::new(
        r"(?i)\b(?:took\s+\d+|ran\s+for\s+(?:over\s+)?(?:\d+|two)\s+|elapsed[:\s]+\d+|finished\s+in\s+\d+|measured\s+elapsed\s+time[:\s]+\d+|execution\s+took\s+\d+|mean\s+runtime\s+of\s+\d+|runtime\s+(?:was|of)\s+\d+|wall[- ]clock\s+time\s+was\s+\d+|wall\s+time\s+on\s+the\s+same\s+core|nightly\s+run\s+took\s+\d+|nightly|p\d{2,3}\s+latency|load1|loadavg|load\s+average|1-min\s+(?:load\s+)?average|one-minute\s+(?:load\s+)?average|\d+[- ](?:min|minute|sec|hour)\s+\d+(?:\.\d+)?)\b",
    )
    .expect("valid regex");
    if meas_re.is_match(clause) {
        return true;
    }

    // 4. Historical durations / ages / production stability / historical narration / commit ordering
    let hist_re = Regex::new(&format!(
        r"(?i)\b(?:{COUNT}[- ](?:years?|months?|days?|hours?|mins?)[- ]old|(?:a|an)\s+(?:years?|months?|days?)[- ]old|{COUNT}\s+(?:years?|months?|days?|weeks?|months?)\s+ago|a\s+day\s+ago|shipped\s+a\s+day|for\s+(?:the\s+past|the\s+last|about|over|~)?\s*\d+\s*(?:years?|months?)|stable\s+for\s+\d+|compatibility\s+for\s+(?:over\s+)?\d+|history\s+spans\s+\d+|survived\s+\d+\s+years|undetected\s+for\s+[~]?\d+\s+years|invariants?|unchecked\s+for\s+\d+|written\s+\d+\s+years\s+ago|issue\s+was\s+resolved|production\s+history\s+spans|commit\s+ordering|(?:minutes?|hours?|days?|weeks?)\s+later|(?:minutes?|hours?|days?|weeks?)\s+earlier|\d+[- ]?(?:minutes?|hours?|days?|weeks?|months?)[- ]gap\s+between|gap\s+of\s+\d+\s+(?:minutes?|hours?|days?|weeks?|months?)|\d+\s+(?:minutes?|hours?|days?|weeks?|months?)\s+between\s+(?:the|two|each|its))\b",
    ))
    .expect("valid regex");
    if hist_re.is_match(clause)
        || (hist_re.is_match(line) && line.to_lowercase().contains("commit ordering"))
    {
        return true;
    }

    // 5. Operational wrap windows / bitfield / epoch
    let wrap_re =
        Regex::new(r"(?i)\b(?:active\s+window|wrap\s+window|wrap\s+duration|bitfield|epoch)\b")
            .expect("valid regex");
    if wrap_re.is_match(clause) || wrap_re.is_match(line) {
        return true;
    }

    // 6. Reading / media time
    let media_re = Regex::new(
        r"(?i)\b\d+[- ](?:minute|min|hour|hr)[- ](?:read|overview|talk|presentation|paper|video|podcast|description)\b",
    )
    .expect("valid regex");
    if media_re.is_match(clause) {
        return true;
    }

    false
}

/// A duration narrated as something that happened or is on record ("has been running
/// for two days", "was raised after 14.5 min", "nine days with no passing drill"), not a
/// span of work ahead. Checked after plan vocabulary, and never for the `in N days`
/// form of an estimate.
fn is_observed_duration(clause: &str) -> bool {
    let estimate_form = Regex::new(&format!(
        r"(?i)\b(?:in|within|takes?|taking)\s+{COUNT}\s*-?\s*(?:hours?|hrs?|days?|weeks?|wks?|months?|quarters?|sprints?|years?)\b"
    ))
    .expect("valid regex");
    if estimate_form.is_match(clause) {
        return false;
    }
    let observed = Regex::new(&format!(
        r"(?i)\b(?:(?:has|have|had)\s+been|(?:was|were)\s+\w+ed|on\s+record\s+for|{COUNT}\s+(?:minutes?|hours?|days?|weeks?|months?)\s+(?:with\s+no|without))\b"
    ))
    .expect("valid regex");
    observed.is_match(clause)
}

/// A match that names a period of data rather than a span of work: a lookback
/// (`emails from the last 24 hours`, `the past two weeks`), `this <period>'s`
/// (`this month's invoice tab`), or a quarter followed by what it reports (`the Q3 invoice`).
/// `before` is the text before the match, including the previous line of the
/// paragraph; `after` is the rest of the line.
pub(crate) fn is_period_reference(before: &str, matched: &str, after: &str) -> bool {
    let lookback =
        Regex::new(r"(?i)\b(?:past|last|previous|prior|recent)\s*$").expect("valid regex");
    if lookback.is_match(before.trim_end_matches('~')) {
        return true;
    }
    // `this month's tab` names the month's data; `a day's work` is still an estimate.
    if matched.to_ascii_lowercase().starts_with("this ")
        && (after.starts_with("'s") || after.starts_with("\u{2019}s"))
    {
        return true;
    }
    let quarter_noun = Regex::new(
        r"(?i)^\s+(?:invoices?|reports?|earnings|results|revenue|sales|numbers|figures|filings?|statements?|close|financials)\b",
    )
    .expect("valid regex");
    matches!(matched, "Q1" | "Q2" | "Q3" | "Q4") && quarter_noun.is_match(after)
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
    if is_observed_duration(clause) {
        return false;
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
                .expect("valid regex");
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

/// Byte spans, per line, of the text matched by any `allow_pattern`.
///
/// Each pattern is matched twice: against every line on its own (so `^` and
/// `$` keep their per-line meaning) and against every paragraph with its
/// soft-wrapped lines joined by a single space (so a phrase that wraps across
/// a line break can still be matched). A paragraph ends at a blank line or a
/// code fence. A paragraph match is mapped back onto the part of each line it
/// covers; the exemption therefore binds to the matched text, never to the
/// whole line or paragraph.
fn allow_pattern_spans(text: &str, allowed: &[Regex]) -> Vec<Vec<(usize, usize)>> {
    let lines: Vec<&str> = text.lines().collect();
    let mut spans: Vec<Vec<(usize, usize)>> = vec![Vec::new(); lines.len()];
    if allowed.is_empty() {
        return spans;
    }

    for (idx, line) in lines.iter().enumerate() {
        for re in allowed {
            spans[idx].extend(re.find_iter(line).map(|m| (m.start(), m.end())));
        }
    }

    // (line index, start in joined text, leading-whitespace bytes, content length)
    let mut para: Vec<(usize, usize, usize, usize)> = Vec::new();
    let mut joined = String::new();
    let mut flush = |para: &mut Vec<(usize, usize, usize, usize)>, joined: &mut String| {
        if para.len() > 1 {
            for re in allowed {
                for m in re.find_iter(joined) {
                    for &(line_idx, seg_start, lead, len) in para.iter() {
                        let seg_end = seg_start + len;
                        let (a, b) = (m.start().max(seg_start), m.end().min(seg_end));
                        if a < b {
                            spans[line_idx].push((a - seg_start + lead, b - seg_start + lead));
                        }
                    }
                }
            }
        }
        para.clear();
        joined.clear();
    };

    let mut fence: Option<&str> = None;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if let Some(m) = ["```", "~~~"].into_iter().find(|m| trimmed.starts_with(m)) {
            flush(&mut para, &mut joined);
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
        let content = trimmed.trim_end();
        if content.is_empty() {
            flush(&mut para, &mut joined);
            continue;
        }
        if !joined.is_empty() {
            joined.push(' ');
        }
        para.push((idx, joined.len(), line.len() - trimmed.len(), content.len()));
        joined.push_str(content);
    }
    flush(&mut para, &mut joined);
    spans
}

pub(crate) fn scan_text_for_time_estimates(
    text: &str,
    banned: &[Regex],
    allowed: &[Regex],
) -> Vec<(usize, String)> {
    let mut violations = Vec::new();
    let mut fence: Option<&str> = None;
    let mut table = MarkdownTableTracker::default();
    let allowed_spans = allow_pattern_spans(text, allowed);

    // The previous line of the same paragraph: a soft-wrapped "from the last" ends it.
    let mut prev: &str = "";
    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if let Some(m) = ["```", "~~~"].into_iter().find(|m| trimmed.starts_with(m)) {
            fence = match fence {
                None => Some(m),
                Some(open) if open == m => None,
                keep => keep,
            };
            prev = "";
            continue;
        }
        if fence.is_some() {
            continue;
        }
        let before_line = prev;
        prev = if trimmed.is_empty() { "" } else { line };

        table.feed_line(line);

        if line_allows(line, "time-estimates") {
            continue;
        }
        let exempt_spans = allowed_spans.get(idx).map(Vec::as_slice).unwrap_or(&[]);

        let clauses = split_into_clauses(line);
        let mut seen_spans: Vec<(usize, usize)> = Vec::new();
        for (clause_start, clause) in clauses {
            for re in banned {
                for hit in re.find_iter(clause) {
                    let abs_start = clause_start + hit.start();
                    let abs_end = clause_start + hit.end();
                    // An allow_pattern exempts the text it matched, never the
                    // rest of the line or paragraph.
                    if exempt_spans
                        .iter()
                        .any(|(s, e)| abs_start < *e && abs_end > *s)
                    {
                        continue;
                    }
                    if seen_spans
                        .iter()
                        .any(|(s, e)| !(abs_end <= *s || abs_start >= *e))
                    {
                        continue;
                    }
                    if is_period_reference(
                        &format!("{before_line} {}", &line[..abs_start]),
                        hit.as_str(),
                        &line[abs_end..],
                    ) {
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
                    code: Some(crate::findings::full_code(
                        GATE,
                        &crate::findings::TIME_ESTIMATE,
                    )),
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
                &crate::findings::TIME_ESTIMATE,
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
        for (before, matched, after) in [
            ("emails from the last", "24 hours", " across all accounts"),
            ("the past ", "two weeks", " of logs"),
            ("last month's invoice tab → ", "this month", "'s tab"),
            ("the thread about the ", "Q3", " invoice"),
        ] {
            assert!(is_period_reference(before, matched, after), "{matched}");
        }
        for (before, matched, after) in [
            ("ships in ", "2 weeks", ""),
            ("that is ", "a day", "'s work"),
            ("target: ", "Q2", ""),
            ("done ", "next sprint", "'s end"),
        ] {
            assert!(!is_period_reference(before, matched, after), "{matched}");
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
    fn split_into_clauses_preserves_exact_byte_offsets_with_unicode() {
        let line = "  | latency | Phase 2 (≤ 2 weeks) |";
        let clauses = split_into_clauses(line);
        for (start, clause) in clauses {
            assert_eq!(
                &line[start..start + clause.len()],
                clause,
                "clause at {start} does not match slice"
            );
        }

        let emdash_line = "Step 1 — Phase 2 (≤ 2 weeks); note";
        let clauses = split_into_clauses(emdash_line);
        for (start, clause) in clauses {
            assert_eq!(
                &emdash_line[start..start + clause.len()],
                clause,
                "clause at {start} does not match slice in emdash line"
            );
        }
    }

    #[test]
    fn allow_pattern_exempts_a_phrase_wrapped_across_lines() {
        let banned = compile(time_estimate_patterns(), "t").unwrap();
        let wrapped = "The run held the one-minute load\naverage below 1.5.\n";
        // Control: without an allow pattern the wrapped term of art fires.
        let bare = scan_text_for_time_estimates(wrapped, &banned, &[]);
        assert_eq!(bare, vec![(1, "one-minute".to_string())]);

        let allowed = compile(["one-minute load average"], "allow").unwrap();
        assert_eq!(
            scan_text_for_time_estimates(wrapped, &banned, &allowed),
            Vec::<(usize, String)>::new(),
            "a multi-word allow_pattern must match across a soft wrap"
        );

        // An indented continuation (list item) is still one paragraph.
        let listed = "- The run held the one-minute load\n  average below 1.5.\n";
        assert!(scan_text_for_time_estimates(listed, &banned, &allowed).is_empty());

        // A blank line ends the paragraph: the phrase no longer exists.
        let split = "The run held the one-minute load\n\naverage below 1.5.\n";
        assert_eq!(
            scan_text_for_time_estimates(split, &banned, &allowed),
            vec![(1, "one-minute".to_string())]
        );
    }

    #[test]
    fn allow_pattern_binds_to_the_match_not_the_paragraph() {
        let banned = compile(time_estimate_patterns(), "t").unwrap();
        let allowed = compile(["one-minute load average"], "allow").unwrap();

        // Unrelated estimate on the wrapped line, after the exempted phrase.
        let para = "The run held the one-minute load\naverage below 1.5, so we ship in 3 weeks.\n";
        assert_eq!(
            scan_text_for_time_estimates(para, &banned, &allowed),
            vec![(2, "3 weeks".to_string())]
        );

        // Unrelated estimate on the SAME line as the exempted phrase.
        let same = "The one-minute load average held, so we ship in 3 weeks.\n";
        assert_eq!(
            scan_text_for_time_estimates(same, &banned, &allowed),
            vec![(1, "3 weeks".to_string())]
        );

        // Line anchors keep their per-line meaning.
        let anchored = compile([r"^timeout: \d+"], "allow").unwrap();
        let text = "Config notes\ntimeout: 30 minutes per shard\n";
        assert!(scan_text_for_time_estimates(text, &banned, &anchored).is_empty());
    }

    #[test]
    fn time_estimates_table_cell_with_unicode_does_not_panic() {
        let banned = compile(time_estimate_patterns(), "t").unwrap();
        let allowed = Vec::new();
        let text = "| Latency | Status |\n|---|---|\n  | 50ms | Phase 2 (≤ 2 weeks) |\n";
        let violations = scan_text_for_time_estimates(text, &banned, &allowed);
        assert!(!violations.is_empty());
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
            // The typographic apostrophe (U+2019), as an editor's smart
            // quotes write a possessive.
            ("only Q1\u{2019}s is ours", "Q1"),
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

        let corpus_raw = include_str!("../../../tests/fixtures/time_estimates_corpus.json");
        let cases: Vec<TestCase> = serde_json::from_str(corpus_raw).expect("valid corpus json");
        assert_eq!(cases.len(), 121, "corpus must contain exactly 121 cases");
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
}
