//! `pii`: home paths, LAN addresses, denylisted host names and fixed-format credentials in
//! tracked text.

use super::{compile, scan_pr_body};
use crate::config::{GateSettings, PiiGate};
use crate::guards::token_formats::{compile_token_classes, is_literal_hit, TokenClass};
use crate::guards::{exempt_filter, line_allows, Context, GateOutcome};
use anyhow::{Context as _, Result};
use regex::Regex;

/// Configuration entries an agent tool documents under its home directory: settings,
/// hooks, plugins, MCP servers, keybindings. A reference to one of them (or to the
/// directory itself) documents the tool. A maintainer's own content there (instruction
/// files such as `CLAUDE.md`, skills, agents, commands, rules, session history, notes)
/// is still reported: that is the leak the rule exists for.
pub const STANDARD_AGENT_ENTRIES: &[&str] = &[
    "settings.json",
    "settings.local.json",
    "config.json",
    "config.toml",
    "hooks",
    "hooks.json",
    "plugins",
    "mcp.json",
    "mcp_config.json",
    "keybindings.json",
];

/// Whether a `home agent-config path` match names the directory itself or a
/// [`STANDARD_AGENT_ENTRIES`] entry, which `agent_config_standard_paths` allows.
pub fn standard_agent_path(settings: &PiiGate, rule: &PiiRule, caps: &regex::Captures) -> bool {
    rule.label == "home agent-config path"
        && settings.agent_config_standard_paths
        && caps.get(1).is_none_or(|c| {
            c.as_str().is_empty()
                || STANDARD_AGENT_ENTRIES.contains(&c.as_str().to_ascii_lowercase().as_str())
        })
}

pub struct PiiRule {
    pub re: Regex,
    pub label: &'static str,
    /// Capture group holding a user name to test against `allowed_users`.
    pub user_group: bool,
    /// Echoing the match would repeat the secret in CI logs.
    pub redact: bool,
    /// The shared fixed-format credential class this rule is, when it is one. A match
    /// whose captured value is a variable reference or placeholder is not a hit.
    pub token_class: Option<&'static TokenClass>,
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
                token_class: None,
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
            redact: settings.redact_lan_ips,
            token_class: None,
        });
    }
    if settings.secrets {
        // One fixed-format table, shared with `shell-secrets`.
        for tc in compile_token_classes()? {
            rules.push(PiiRule {
                re: tc.re,
                label: tc.class.label,
                user_group: false,
                redact: true,
                token_class: Some(tc.class),
            });
        }
    }
    if settings.agent_config_refs {
        let agent_dirs = [
            "claude", "gemini", "codex", "cursor", "aider", "copilot", "continue",
        ]
        .join("|");
        rules.push(PiiRule {
            // The first component under the directory is captured for
            // `standard_agent_path`.
            re: Regex::new(&format!(
                r"(?i)(?:~|\$HOME)/\.(?:{agent_dirs})\b(?:/([A-Za-z0-9_.-]*))?"
            ))?,
            label: "home agent-config path",
            user_group: false,
            redact: false,
            token_class: None,
        });
        let res_disc = format!(r"\b{}{}\b", "RESEARCH_DISCIPLINES", r"\.md");
        rules.push(PiiRule {
            re: Regex::new(&res_disc)?,
            label: "personal methodology doc",
            user_group: false,
            redact: false,
            token_class: None,
        });
        let playbook = format!(r"\b{}{}\b", r"[A-Z0-9_]*_PLAYBOOK", r"\.md");
        rules.push(PiiRule {
            re: Regex::new(&playbook)?,
            label: "personal playbook",
            user_group: false,
            redact: false,
            token_class: None,
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
            token_class: None,
        });
    }
    for term in &settings.term_denylist {
        let term = term.trim();
        if term.is_empty() {
            continue;
        }
        let (case_sensitive, clean_term) = if let Some(t) = term.strip_prefix("case-sensitive:") {
            (true, t.trim())
        } else if let Some(t) = term.strip_prefix("case:") {
            (true, t.trim())
        } else {
            (false, term)
        };
        if clean_term.is_empty() {
            continue;
        }
        let words: Vec<&str> = clean_term.split_whitespace().collect();
        let pattern = words
            .iter()
            .map(|w| regex::escape(w))
            .collect::<Vec<_>>()
            .join(r"\s+");
        let flags = if case_sensitive { "" } else { "(?i)" };
        rules.push(PiiRule {
            re: Regex::new(&format!(
                r"{flags}(?:^|[^A-Za-z0-9-]){pattern}(?:$|[^A-Za-z0-9-])"
            ))?,
            label: "denylisted term",
            user_group: false,
            redact: true,
            token_class: None,
        });
    }
    for p in &settings.extra_patterns {
        rules.push(PiiRule {
            re: Regex::new(p).with_context(|| format!("invalid pii extra_patterns regex `{p}`"))?,
            label: "configured pattern",
            user_group: false,
            redact: true,
            token_class: None,
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
            if opts.is_active_config
                && (rule.label == "denylisted hostname" || rule.label == "denylisted term")
            {
                continue;
            }
            for caps in rule.re.captures_iter(token) {
                let m = caps.get(0).unwrap();
                if rule.label == "private LAN address"
                    && is_exempt_lan_ip(m.as_str(), token, m.start(), m.end())
                {
                    continue;
                }
                if rule
                    .token_class
                    .is_some_and(|class| !is_literal_hit(class, &caps))
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
                if allowed_user || standard_agent_path(opts.settings, rule, &caps) {
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
                    let is_denylisted_path = opts.rules.iter().any(|r| {
                        (r.label == "denylisted hostname" || r.label == "denylisted term")
                            && r.re.is_match(opts.label)
                    });
                    let (rep_file, rep_line) = if is_denylisted_path {
                        (None, None)
                    } else {
                        (Some(opts.label), Some(line_num))
                    };
                    out.push(
                        opts.settings.severity(),
                        &crate::findings::HOST_OR_PII_LEAK,
                        rep_file,
                        rep_line,
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

    // Lines inside a function the repository declares as a test entry point, or in a file
    // it declares as test scope, hold fixtures by definition and are not leaks.
    let declared = &ctx.config.tests;
    let registry = crate::ast::default_registry();
    let vocab = crate::ast::AssertVocabulary {
        test_functions: declared.functions.clone(),
        test_paths: declared.paths.clone(),
        ..Default::default()
    };
    type Spans = (bool, Vec<(usize, usize)>, Option<String>);
    let declared_test_spans = |path: &str, text: &str| -> Spans {
        if declared.functions.is_empty() && declared.paths.is_empty() {
            return (false, Vec::new(), None);
        }
        if crate::ast::functions::declared_test_path(path, &declared.paths) {
            return (true, Vec::new(), None);
        }
        // A file the pack cannot parse has no function to exempt: each of its lines is
        // scanned, and the report says why.
        let (facts, unparsed) = match registry
            .find_pack(path)
            .map(|pack| pack.extract(path, text, &vocab))
        {
            Some(Ok(facts)) => (Some(facts), None),
            Some(Err(e)) => (
                None,
                Some(format!(
                    "{e}, so no line of it is read as inside a declared test function"
                )),
            ),
            None => (None, None),
        };
        let spans = facts
            .map(|facts| {
                facts
                    .tests
                    .iter()
                    .filter(|t| {
                        let leaf = t.name.rsplit("::").next().unwrap_or(&t.name);
                        declared.functions.iter().any(|f| f == leaf)
                    })
                    .map(|t| (t.line, t.end_line.max(t.line)))
                    .collect()
            })
            .unwrap_or_default();
        (false, spans, unparsed)
    };
    let scan = |label: &str,
                text: &str,
                added_lines: Option<&std::collections::BTreeSet<usize>>,
                out: &mut GateOutcome| {
        let active_cfg = is_active_config(label);
        let (whole_file_is_test, test_spans, unparsed) = declared_test_spans(label, text);
        if whole_file_is_test {
            return;
        }
        out.notes.extend(unparsed);
        for (idx, line) in text.lines().enumerate() {
            let line_num = idx + 1;
            if let Some(lines) = added_lines {
                if !lines.contains(&line_num) {
                    continue;
                }
            }
            if test_spans
                .iter()
                .any(|(a, b)| *a <= line_num && line_num <= *b)
            {
                continue;
            }
            let hit = rules.iter().find_map(|rule| {
                if active_cfg
                    && (rule.label == "denylisted hostname" || rule.label == "denylisted term")
                {
                    return None;
                }
                rule.re.captures_iter(line).find_map(|caps| {
                    let m = caps.get(0).unwrap();
                    if rule.label == "private LAN address"
                        && is_exempt_lan_ip(m.as_str(), line, m.start(), m.end())
                    {
                        return None;
                    }
                    if rule
                        .token_class
                        .is_some_and(|class| !is_literal_hit(class, &caps))
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
                    (!allowed_user && !standard_agent_path(settings, rule, &caps))
                        .then(|| (rule, caps.get(0).map(|m| m.as_str().to_string())))
                })
            });
            let Some((rule, matched)) = hit else { continue };
            if line_allows(line, GATE) {
                out.inline_exemptions += 1;
                out.overrides.push(crate::tokens::OverrideRecord {
                    gate: GATE.to_string(),
                    code: Some(crate::findings::full_code(
                        GATE,
                        &crate::findings::HOST_OR_PII_LEAK,
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
                continue;
            }
            if allowed.iter().any(|re| re.is_match(line)) {
                continue;
            }
            let detail = match (rule.redact, matched) {
                (false, Some(m)) => format!("{} `{m}`", rule.label),
                _ => format!("{} (match not echoed)", rule.label),
            };
            let is_denylisted_path = rules.iter().any(|r| {
                (r.label == "denylisted hostname" || r.label == "denylisted term")
                    && r.re.is_match(label)
            });
            let (rep_file, rep_line) = if is_denylisted_path {
                (None, None)
            } else {
                (Some(label), Some(idx + 1))
            };
            out.push(
                settings.severity(),
                &crate::findings::HOST_OR_PII_LEAK,
                rep_file,
                rep_line,
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
                None => out.notes.push(super::unread_note(&f.path)),
            }
        }
    } else {
        // Binary files this change touched are named; the rest of the tree is counted.
        let changed: std::collections::HashSet<String> = ctx
            .git
            .changed_files()?
            .into_iter()
            .filter(|f| !f.is_deleted())
            .map(|f| f.path)
            .collect();
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
                None if changed.contains(&path) => out.notes.push(super::unread_note(&path)),
                None => binary += 1,
            }
        }
    }
    if binary > 0 {
        out.notes
            .push(format!("{binary} other binary file(s) not scanned"));
    }
    let mut scan_body = |label: &str, text: &str, out: &mut GateOutcome| {
        scan(label, text, None, out);
    };
    scan_pr_body(ctx, settings.scan_pr_body, &mut out, &mut scan_body);

    let denylist_rules: Vec<&PiiRule> = rules
        .iter()
        .filter(|r| r.label == "denylisted hostname" || r.label == "denylisted term")
        .collect();

    if !denylist_rules.is_empty() {
        let changed = ctx.git.changed_files()?;
        for f in &changed {
            if f.kind == crate::gitctx::ChangeKind::Added
                || f.kind == crate::gitctx::ChangeKind::Renamed
            {
                if exempt.matches(&f.path) || is_active_config(&f.path) {
                    continue;
                }
                if allowed.iter().any(|re| re.is_match(&f.path)) {
                    continue;
                }
                for rule in &denylist_rules {
                    if rule.re.is_match(&f.path) {
                        let source = if f.kind == crate::gitctx::ChangeKind::Renamed {
                            "path of a renamed file"
                        } else {
                            "path of an added file"
                        };
                        out.push(
                            settings.severity(),
                            &crate::findings::HOST_OR_PII_LEAK,
                            None,
                            None,
                            format!("Found a {} in {source} (match not echoed).", rule.label),
                            "Rename the file to avoid the denylisted term or hostname.",
                        );
                        out.anchor_last(format!("file-path:{}", f.path));
                        break;
                    }
                }
            }
        }

        let commits = ctx.git.commit_details().unwrap_or_default();
        for commit in &commits {
            let sha7 = &commit.sha[..7.min(commit.sha.len())];
            if allowed.iter().any(|re| re.is_match(&commit.message)) {
                continue;
            }
            for rule in &denylist_rules {
                if rule.re.is_match(&commit.message) {
                    out.push(
                        settings.severity(),
                        &crate::findings::HOST_OR_PII_LEAK,
                        None,
                        None,
                        format!(
                            "Found a {} in commit {sha7} message (match not echoed).",
                            rule.label
                        ),
                        "Rewrite git history (git commit --amend or git rebase -i) to remove the denylisted term or hostname from the commit message.",
                    );
                    out.anchor_last(format!("commit:{}", commit.sha));
                    break;
                }
            }
        }

        if let Some(title) = ctx
            .pr_title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            if !allowed.iter().any(|re| re.is_match(title)) {
                for rule in &denylist_rules {
                    if rule.re.is_match(title) {
                        out.push(
                            settings.severity(),
                            &crate::findings::HOST_OR_PII_LEAK,
                            None,
                            None,
                            format!("Found a {} in pull request title (match not echoed).", rule.label),
                            "Edit the pull request title to remove the denylisted term or hostname.",
                        );
                        out.anchor_last("pr-title");
                        break;
                    }
                }
            }
        }

        if let Some(branch) = ctx
            .git
            .head_branch()
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty())
        {
            if !allowed.iter().any(|re| re.is_match(branch)) {
                for rule in &denylist_rules {
                    if rule.re.is_match(branch) {
                        out.push(
                            settings.severity(),
                            &crate::findings::HOST_OR_PII_LEAK,
                            None,
                            None,
                            format!("Found a {} in branch name (match not echoed).", rule.label),
                            "Rename the branch to remove the denylisted term or hostname.",
                        );
                        out.anchor_last("branch");
                        break;
                    }
                }
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule_hits(settings: &PiiGate, line: &str) -> bool {
        pii_rules(settings).unwrap().iter().any(|r| {
            r.re.captures_iter(line).any(|c| {
                !standard_agent_path(settings, r, &c)
                    && !(r.user_group
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
    fn term_denylist_is_whole_token_and_case_insensitive_by_default() {
        let s = PiiGate {
            term_denylist: vec!["project".into()],
            ..PiiGate::default()
        };
        assert!(rule_hits(&s, "this is our Project"));
        assert!(rule_hits(&s, "PROJECT"));
        assert!(rule_hits(&s, "project"));
        assert!(!rule_hits(&s, "the projector is on"));
        assert!(!rule_hits(&s, "subproject"));
        assert!(!rule_hits(&PiiGate::default(), "this is our Project"));
    }

    #[test]
    fn term_denylist_supports_case_sensitive_prefix() {
        let s = PiiGate {
            term_denylist: vec!["case:Codename".into(), "case-sensitive:AlphaOne".into()],
            ..PiiGate::default()
        };
        assert!(rule_hits(&s, "launch Codename today"));
        assert!(!rule_hits(&s, "launch codename today"));
        assert!(!rule_hits(&s, "launch CODENAME today"));

        assert!(rule_hits(&s, "target is AlphaOne"));
        assert!(!rule_hits(&s, "target is alphaone"));
        assert!(!rule_hits(&s, "target is ALPHAONE"));
    }

    #[test]
    fn term_denylist_supports_phrase_entries() {
        let s = PiiGate {
            term_denylist: vec!["Secret Project".into()],
            ..PiiGate::default()
        };
        assert!(rule_hits(&s, "this is Secret Project"));
        assert!(rule_hits(&s, "this is secret   project"));
        assert!(rule_hits(&s, "SECRET PROJECT"));
        assert!(!rule_hits(&s, "Project Secret"));
        assert!(!rule_hits(&s, "Secret Other Project"));
        assert!(!rule_hits(&s, "SecretProject"));
    }

    /// One live-format line per fixed-format class, assembled at run time so this file
    /// holds no token-shaped literal. Each must be reported by `pii` and by `shell-secrets`.
    fn fixed_format_samples() -> Vec<(&'static str, String)> {
        vec![
            (
                "GitHub token",
                format!("t = \"{}_{}\"", "ghp", "a1B2".repeat(9)),
            ),
            (
                "GitHub token",
                format!("t = \"{}_{}\"", "github_pat", "A1b2C3d4E5".repeat(8) + "xy"),
            ),
            (
                "AWS access key ID",
                format!("id = {}ABCDEF0123456789", "AKIA"),
            ),
            (
                "AWS access key ID",
                format!("id = {}ABCDEF0123456789", "ASIA"),
            ),
            (
                "AWS access key ID",
                format!("id = {}ABCDEF0123456789", "ABIA"),
            ),
            (
                "AWS access key ID",
                format!("id = {}ABCDEF0123456789", "ACCA"),
            ),
            (
                "Slack token",
                format!("t = {}-123456789012-123456789012-abcdefABCDEF", "xoxb"),
            ),
            (
                "OpenAI or Anthropic API key",
                format!("k = {}-{}", "sk", "abcdefghijklmnopqrstuvwx"),
            ),
            (
                "OpenAI or Anthropic API key",
                format!("k = {}-ant-api03-{}", "sk", "abcdefghijklmnopqrstuvwx"),
            ),
            (
                "private key header",
                format!("{}BEGIN OPENSSH PRIVATE KEY{}", "-----", "-----"),
            ),
            (
                "literal Authorization Bearer token",
                format!("{}: Bearer {}", "Authorization", "abcdef0123456789"),
            ),
        ]
    }

    #[test]
    fn pii_checks_every_fixed_format_class_shell_secrets_knows() {
        use crate::config::ShellSecretsGate;
        use crate::guards::shell_secrets::ShellSecretScanner;
        let rules = pii_rules(&PiiGate::default()).unwrap();
        let scanner = ShellSecretScanner::new(&ShellSecretsGate::default()).unwrap();
        for (label, line) in fixed_format_samples() {
            let rule = rules
                .iter()
                .filter(|r| r.label == label)
                .find(|r| {
                    let class = r.token_class.unwrap();
                    r.re.captures_iter(&line)
                        .any(|c| crate::guards::token_formats::is_literal_hit(class, &c))
                })
                .unwrap_or_else(|| panic!("pii misses the {label} sample: {line}"));
            assert!(rule.redact, "{label} must not be echoed");
            assert!(
                scanner.check_line(&line).is_some(),
                "shell-secrets misses the {label} sample: {line}"
            );
        }
    }

    #[test]
    fn pii_ignores_placeholders_variables_and_hashes() {
        let rules = pii_rules(&PiiGate::default()).unwrap();
        let hits = |line: &str| {
            rules.iter().filter(|r| r.token_class.is_some()).any(|r| {
                let class = r.token_class.unwrap();
                r.re.captures_iter(line)
                    .any(|c| crate::guards::token_formats::is_literal_hit(class, &c))
            })
        };
        for line in [
            "export OPENAI_API_KEY=sk-...",
            "export OPENAI_API_KEY=<your-key>",
            "curl -H \"Authorization: Bearer $TOKEN\" https://example.com",
            "curl -H \"Authorization: Bearer ${API_TOKEN}\" https://example.com",
            "Authorization: Bearer ${{ secrets.API_TOKEN }}",
            "Authorization: Bearer <token>",
            "Authorization: Bearer xxxxxxxxxxxxxxxx",
            "checksum = \"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08\"",
            "integrity sha512-Zm9vYmFyYmF6cXV4Zm9vYmFyYmF6cXV4Zm9vYmFyYmF6cXV4==",
            "id = 123e4567-e89b-12d3-a456-426614174000",
            "gh_token_name = \"ghp_<token>\"",
        ] {
            assert!(!hits(line), "false positive on: {line}");
        }
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
        assert_eq!(
            pii_rules(&secrets_only).unwrap().len(),
            crate::guards::token_formats::TOKEN_CLASSES.len()
        );
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

    /// A tool's own configuration locations are documentation; a maintainer's content
    /// under the same directory is not, and `agent_config_standard_paths = false`
    /// reports both.
    #[test]
    fn standard_agent_config_locations_are_documentation_not_leaks() {
        let narrowed = PiiGate {
            home_paths: false,
            lan_ips: false,
            secrets: false,
            ..PiiGate::default()
        };
        assert!(narrowed.agent_config_refs && narrowed.agent_config_standard_paths);
        let standard = [
            format!("writes {}{}", "~", "/.copilot/hooks/discipline.json"),
            format!("trust it in {}{}", "~", "/.copilot/config.json"),
            format!("edit {}{}", "$HOME", "/.claude/settings.json"),
            format!("the {}{} directory", "~", "/.codex"),
            format!("see {}{} for servers", "~", "/.gemini/mcp_config.json"),
        ];
        for line in &standard {
            assert!(!rule_hits(&narrowed, line), "a tool location: {line}");
        }
        let personal = [
            format!("with unit tests in {}{}", "~", "/.claude/CLAUDE.md"),
            format!(
                "see {}{}",
                "~", "/.claude/projects/-Users-me-repo/memory/MEMORY.md"
            ),
            format!("my {}{}", "~", "/.claude/skills/review/SKILL.md"),
            format!("notes in {}{}", "$HOME", "/.gemini/NOTES.md"),
        ];
        for line in &personal {
            assert!(rule_hits(&narrowed, line), "personal content: {line}");
        }
        let strict = PiiGate {
            agent_config_standard_paths: false,
            ..narrowed.clone()
        };
        for line in standard.iter().chain(&personal) {
            assert!(rule_hits(&strict, line), "strict reports every one: {line}");
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

    #[test]
    fn test_lan_ip_redaction_config() {
        let json_text = "{\"target\": \"192.168.1.42\"}"; // discipline:allow(pii)

        // Default: redact_lan_ips = false -> IP is echoed for triage
        let default_settings = PiiGate::default();
        assert!(!default_settings.redact_lan_ips);
        let rules_default = pii_rules(&default_settings).unwrap();
        let mut out_default = GateOutcome::new("pii");
        let allowed = vec![];
        let opts_default = PiiScanOptions {
            label: "test.json",
            rules: &rules_default,
            settings: &default_settings,
            allowed: &allowed,
            is_active_config: false,
            added_lines: None,
        };
        assert!(scan_json(&opts_default, json_text, &mut out_default));
        assert_eq!(out_default.violations.len(), 1);
        assert!(out_default.violations[0].message.contains("192.168.1.42")); // discipline:allow(pii)

        // Opt-in: redact_lan_ips = true -> IP is masked
        let masked_settings = PiiGate {
            redact_lan_ips: true,
            ..Default::default()
        };
        let rules_masked = pii_rules(&masked_settings).unwrap();
        let mut out_masked = GateOutcome::new("pii");
        let opts_masked = PiiScanOptions {
            label: "test.json",
            rules: &rules_masked,
            settings: &masked_settings,
            allowed: &allowed,
            is_active_config: false,
            added_lines: None,
        };
        assert!(scan_json(&opts_masked, json_text, &mut out_masked));
        assert_eq!(out_masked.violations.len(), 1);
        assert!(!out_masked.violations[0].message.contains("192.168.1.42")); // discipline:allow(pii)
        assert!(out_masked.violations[0]
            .message
            .contains("(match not echoed)"));
    }
}
