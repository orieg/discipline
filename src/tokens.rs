//! Override directive parser shared by every gate that accepts an escape hatch.
//!
//! Grammar rules:
//!   - a directive must **begin its own line** (optionally inside `<!-- -->`);
//!     a mention mid-sentence, in a table cell, or in a code span never arms it
//!   - lines inside fenced code blocks are ignored
//!   - the reason must be non-empty and must not be a template placeholder
//!   - an override is **scoped**: it only covers a subject that its reason names
//!
//! The defect this prevents: a PR that *described* the override
//! mechanism in a markdown table silently approved every regression in the run.

use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "detail")]
pub enum OverrideSource {
    PrBody,
    Commit(String),
    Inline { file: String, line: usize },
}

impl std::fmt::Display for OverrideSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OverrideSource::PrBody => write!(f, "PR body"),
            OverrideSource::Commit(sha) => write!(f, "commit {sha}"),
            OverrideSource::Inline { file, line } => write!(f, "inline {file}:{line}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverrideRecord {
    pub gate: String,
    pub subject: String,
    pub directive: String,
    pub reason: String,
    pub source: OverrideSource,
    pub hidden: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedDirective {
    pub directive: String,
    pub reason: String,
    pub source: OverrideSource,
    pub hidden: bool,
}

impl ParsedDirective {
    pub fn covers(&self, subject: &str) -> bool {
        reason_names(&self.reason, subject)
    }
}

pub fn find_override(
    directives: &[ParsedDirective],
    gate: &str,
    names: &[&str],
    subject: &str,
) -> Option<OverrideRecord> {
    for d in directives {
        if names.iter().any(|n| n.eq_ignore_ascii_case(&d.directive)) && d.covers(subject) {
            return Some(OverrideRecord {
                gate: gate.to_string(),
                subject: subject.to_string(),
                directive: d.directive.clone(),
                reason: d.reason.clone(),
                source: d.source.clone(),
                hidden: d.hidden,
            });
        }
    }
    None
}

pub fn find_gate_or_subject_override(
    directives: &[ParsedDirective],
    gate: &str,
    names: &[&str],
    subject: &str,
) -> Option<OverrideRecord> {
    for d in directives {
        if names.iter().any(|n| n.eq_ignore_ascii_case(&d.directive))
            && (subject.is_empty() || d.covers(subject) || d.covers(gate))
        {
            return Some(OverrideRecord {
                gate: gate.to_string(),
                subject: subject.to_string(),
                directive: d.directive.clone(),
                reason: d.reason.clone(),
                source: d.source.clone(),
                hidden: d.hidden,
            });
        }
    }
    None
}

pub const REMOVES: &[&str] = &[
    "removes",
    "deletes",
    "remove",
    "delete",
    "discipline:allow(deletion-rationale)",
    "allow(deletion-rationale)",
];
pub const ALLOW_ASSERTION_DROP: &[&str] = &[
    "allow-assertion-drop",
    "discipline:allow(assertion-reduction)",
    "allow(assertion-reduction)",
];
pub const ALLOW_IGNORE: &[&str] = &[
    "allow-ignore",
    "discipline:allow(ignored-tests)",
    "allow(ignored-tests)",
];
pub const ALLOW_GATE_WEAKENING: &[&str] = &[
    "allow-gate-weakening",
    "discipline:allow(config-integrity)",
    "allow(config-integrity)",
];
pub const ALLOW_GOLDEN_UPDATE: &[&str] = &[
    "allow-golden-update",
    "discipline:allow(golden-output)",
    "allow(golden-output)",
];
pub const ALLOW_NUL: &[&str] = &[
    "allow-nul",
    "allow-nul-byte",
    "allow-corrupt",
    "allow-assertion-drop",
    "discipline:allow(assertion-reduction)",
    "allow(assertion-reduction)",
    "discipline:allow(vacuous-tests)",
    "allow(vacuous-tests)",
];

pub const ALLOW_REGRESSION: &[&str] = &[
    "allow-regression",
    "discipline:allow(bench-regression)",
    "allow(bench-regression)",
];

pub const ALLOW_COMMAND: &[&str] = &[
    "allow-command",
    "discipline:allow(command)",
    "allow(command)",
];

pub const ALLOW_DEPENDENCY: &[&str] = &[
    "allow-dependency",
    "discipline:allow(dependency-delta)",
    "allow(dependency-delta)",
];

pub const ALLOW_TEST_SHRINK: &[&str] = &[
    "allow-gate-weakening",
    "allow-test-shrink",
    "allow-test-budget",
    "allow-floor-drop",
    "discipline:allow(test-budget)",
    "allow(test-budget)",
    "discipline:allow(test-floor)",
    "allow(test-floor)",
];

pub const ALLOW_CI_WEAKENING: &[&str] = &[
    "allow-gate-weakening",
    "allow-ci-weakening",
    "allow-unpinned-action",
    "allow-ci-change",
    "discipline:allow(ci-integrity)",
    "allow(ci-integrity)",
];

pub const SECRETS_ARGV_OK: &[&str] = &[
    "secrets-argv-ok",
    "discipline:allow(shell-secrets)",
    "allow(shell-secrets)",
];

pub const NO_ISSUE: &[&str] = &[
    "no-issue",
    "discipline:allow(issue-link)",
    "allow(issue-link)",
];

pub const ALLOW_PROVENANCE: &[&str] = &[
    "allow-provenance",
    "discipline:allow(provenance-tags)",
    "allow(provenance-tags)",
    "allow-unpaired-figures",
    "docs-lint: allow",
    "docs-lint:allow",
];

pub const ALL_DIRECTIVE_NAMES: &[&str] = &[
    "removes",
    "deletes",
    "remove",
    "delete",
    "discipline:allow(deletion-rationale)",
    "allow(deletion-rationale)",
    "allow-assertion-drop",
    "discipline:allow(assertion-reduction)",
    "allow(assertion-reduction)",
    "allow-ignore",
    "discipline:allow(ignored-tests)",
    "allow(ignored-tests)",
    "allow-gate-weakening",
    "discipline:allow(config-integrity)",
    "allow(config-integrity)",
    "allow-golden-update",
    "discipline:allow(golden-output)",
    "allow(golden-output)",
    "allow-regression",
    "discipline:allow(bench-regression)",
    "allow(bench-regression)",
    "allow-command",
    "discipline:allow(command)",
    "allow(command)",
    "allow-dependency",
    "discipline:allow(dependency-delta)",
    "allow(dependency-delta)",
    "allow-test-shrink",
    "allow-test-budget",
    "allow-floor-drop",
    "discipline:allow(test-budget)",
    "allow(test-budget)",
    "discipline:allow(test-floor)",
    "allow(test-floor)",
    "allow-ci-weakening",
    "allow-unpinned-action",
    "allow-ci-change",
    "discipline:allow(ci-integrity)",
    "allow(ci-integrity)",
    "allow-nul",
    "allow-nul-byte",
    "allow-corrupt",
    "secrets-argv-ok",
    "discipline:allow(shell-secrets)",
    "allow(shell-secrets)",
    "no-issue",
    "discipline:allow(issue-link)",
    "allow(issue-link)",
    "allow-provenance",
    "discipline:allow(provenance-tags)",
    "allow(provenance-tags)",
    "allow-unpaired-figures",
    "docs-lint: allow",
    "docs-lint:allow",
];

const PLACEHOLDERS: &[&str] = &[
    "todo", "tbd", "none", "n/a", "na", "reason", "why", "...", "xxx", "fixme", "-",
];

/// Parses all directives from `text` matching any names in `names`.
pub fn parse_directives_with_names(
    text: &str,
    names: &[&str],
    source: OverrideSource,
) -> Vec<ParsedDirective> {
    let patterns = names
        .iter()
        .map(|n| {
            let esc = regex::escape(n);
            if n.ends_with(')') {
                format!("{esc}(?::|[ \\t])")
            } else {
                format!("{esc}:")
            }
        })
        .collect::<Vec<_>>()
        .join("|");
    let re = Regex::new(&format!(
        r"(?i)^[ \t]*(<!--[ \t]*)?(?:discipline:[ \t]+)?({patterns})[ \t]*(.*)$"
    ))
    .expect("directive regex is static");

    let mut directives = Vec::new();
    let mut fence: Option<&str> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        let marker = ["```", "~~~"].into_iter().find(|m| trimmed.starts_with(m));
        match (fence, marker) {
            (None, Some(m)) => {
                fence = Some(m);
                continue;
            }
            (Some(open), Some(m)) if open == m => {
                fence = None;
                continue;
            }
            (Some(_), _) => continue,
            (None, None) => {}
        }
        if let Some(caps) = re.captures(line) {
            let reason = clean_reason(&caps[3]);
            if !is_placeholder(&reason) {
                let directive_str = caps[2].trim_end_matches(':').trim().to_string();
                directives.push(ParsedDirective {
                    directive: directive_str,
                    reason,
                    source: source.clone(),
                    hidden: caps.get(1).is_some(),
                });
            }
        }
    }
    directives
}

/// Parses all directives in `text` against all known directive names.
pub fn parse_directives(text: &str, source: OverrideSource) -> Vec<ParsedDirective> {
    parse_directives_with_names(text, ALL_DIRECTIVE_NAMES, source)
}

/// Extracts active valid directives according to the policy in `policy`.
/// Directives from disallowed sources or hidden when `allow_hidden` is false
/// are dropped, and explanatory notes are emitted.
pub fn extract_directives(
    pr_body: Option<&str>,
    commits: &[(String, String)],
    policy: &crate::config::DirectivesConfig,
) -> (Vec<ParsedDirective>, Vec<String>) {
    let mut active = Vec::new();
    let mut notes = Vec::new();

    let pr_body_allowed = policy.sources.iter().any(|s| s == "pr-body");
    let commits_allowed = policy.sources.iter().any(|s| s == "commits");

    if let Some(body) = pr_body {
        let parsed = parse_directives(body, OverrideSource::PrBody);
        if pr_body_allowed {
            for d in parsed {
                if d.hidden && !policy.allow_hidden {
                    notes.push(format!(
                        "hidden directive `{}: {}` in PR body ignored (directives.allow_hidden is false)",
                        d.directive, d.reason
                    ));
                } else {
                    active.push(d);
                }
            }
        } else if !parsed.is_empty() {
            notes.push("PR-body directives are disabled by policy; ignored".to_string());
        }
    }

    for (oid, msg) in commits {
        let parsed = parse_directives(msg, OverrideSource::Commit(oid.clone()));
        if commits_allowed {
            for d in parsed {
                if d.hidden && !policy.allow_hidden {
                    notes.push(format!(
                        "hidden directive `{}: {}` in commit {oid} ignored (directives.allow_hidden is false)",
                        d.directive, d.reason
                    ));
                } else {
                    active.push(d);
                }
            }
        } else if !parsed.is_empty() {
            notes.push(format!(
                "commit-message directives are disabled by policy; ignored directive from commit {oid}"
            ));
        }
    }

    (active, notes)
}

/// Extracts active valid directives taking into account both global policy and per-gate settings.
pub fn extract_directives_for_config(
    pr_body: Option<&str>,
    commits: &[(String, String)],
    config: &crate::config::DisciplineConfig,
) -> (Vec<ParsedDirective>, Vec<String>) {
    let policy = &config.directives;
    let mut active = Vec::new();
    let mut notes = Vec::new();

    let pr_body_allowed = policy.sources.iter().any(|s| s == "pr-body");
    let commits_allowed = policy.sources.iter().any(|s| s == "commits");

    let is_hidden_allowed = |d: &ParsedDirective| -> bool {
        if policy.allow_hidden {
            return true;
        }
        if REMOVES.iter().any(|n| n.eq_ignore_ascii_case(&d.directive)) {
            if let Some(gate_hidden) = config.gates.deletion_rationale.allow_hidden {
                return gate_hidden;
            }
        }
        false
    };

    if let Some(body) = pr_body {
        let parsed = parse_directives(body, OverrideSource::PrBody);
        if pr_body_allowed {
            for d in parsed {
                if d.hidden && !is_hidden_allowed(&d) {
                    notes.push(format!(
                        "hidden directive `{}: {}` in PR body ignored (directives.allow_hidden is false)",
                        d.directive, d.reason
                    ));
                } else {
                    active.push(d);
                }
            }
        } else if !parsed.is_empty() {
            notes.push("PR-body directives are disabled by policy; ignored".to_string());
        }
    }

    for (oid, msg) in commits {
        let parsed = parse_directives(msg, OverrideSource::Commit(oid.clone()));
        if commits_allowed {
            for d in parsed {
                if d.hidden && !is_hidden_allowed(&d) {
                    notes.push(format!(
                        "hidden directive `{}: {}` in commit {oid} ignored (directives.allow_hidden is false)",
                        d.directive, d.reason
                    ));
                } else {
                    active.push(d);
                }
            }
        } else if !parsed.is_empty() {
            notes.push(format!(
                "commit-message directives are disabled by policy; ignored directive from commit {oid}"
            ));
        }
    }

    (active, notes)
}

/// Reasons of every well-formed directive named in `names` found in `text`.
pub fn directive_reasons(text: &str, names: &[&str]) -> Vec<String> {
    parse_directives_with_names(text, names, OverrideSource::PrBody)
        .into_iter()
        .map(|d| d.reason)
        .collect()
}

/// True when some directive's reason names `subject` (or, for paths, a
/// directory prefix of it or its file name).
pub fn covers(reasons: &[String], subject: &str) -> bool {
    reasons.iter().any(|r| reason_names(r, subject))
}

fn reason_names(reason: &str, subject: &str) -> bool {
    let is_token_char = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/');
    let trimmed_subject = subject.trim();

    if !trimmed_subject.is_empty() {
        // 1. Quoted subject anywhere in the reason: "name", 'name', or `name`.
        for quote in ['"', '\'', '`'] {
            let quoted = format!("{quote}{trimmed_subject}{quote}");
            if reason.contains(&quoted) {
                return true;
            }
        }

        // 2. Multi-word subject at the beginning of the reason (e.g. `allow-ignore: my test name <reason>`).
        if trimmed_subject.contains(' ') {
            let unquoted_reason = reason.trim_start_matches(['"', '\'', '`']);
            if let Some(rest) = unquoted_reason.strip_prefix(trimmed_subject) {
                if rest.is_empty() || rest.starts_with(|c: char| !is_token_char(c)) {
                    return true;
                }
            }
        }
    }

    let raw_tokens = reason
        .split(|c: char| !is_token_char(c))
        .filter(|t| !t.is_empty());
    let file_name = subject.rsplit('/').next().unwrap_or(subject);
    for raw_token in raw_tokens {
        let has_slash = raw_token.contains('/');
        let trimmed_leading = raw_token.strip_prefix("./").unwrap_or(raw_token);
        let token = trimmed_leading.trim_end_matches('.').trim_matches('/');
        if token.is_empty() {
            continue;
        }
        if token == subject || token == file_name {
            return true;
        }
        // Directory prefix: only when written with a slash (e.g. `tests/legacy` or `tests/`).
        // A bare word like `tests` in ordinary prose never acts as a directory prefix.
        if has_slash && subject.contains('/') && subject.starts_with(&format!("{token}/")) {
            return true;
        }
    }
    false
}

fn clean_reason(raw: &str) -> String {
    let mut r = raw.trim();
    // `--!>` also closes an HTML comment; left on the reason it once let a
    // placeholder through.
    for closer in ["--!>", "-->"] {
        if let Some(stripped) = r.strip_suffix(closer) {
            r = stripped.trim_end();
        }
    }
    r.to_string()
}

fn is_placeholder(reason: &str) -> bool {
    let r = reason.trim();
    if r.is_empty() {
        return true;
    }
    if r.starts_with('<') && r.ends_with('>') {
        return true;
    }
    PLACEHOLDERS.contains(&r.to_lowercase().as_str())
}

static CITATION_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"(?i)https?://[a-zA-Z0-9_.-]+/[a-zA-Z0-9_.-]+/[a-zA-Z0-9_.-]+/(?:actions/runs/\d+|pipelines/\d+|jobs/\d+)|(?:results|docs|crates|scripts|benches|tests|src|target)/[a-zA-Z0-9_./-]+\.(?:json|txt|csv|md|svg|log|out)").unwrap()
});

/// Extracts a verifiable citation (CI run URL or committed artifact path) from an override reason.
pub fn extract_citation(reason: &str) -> Option<String> {
    CITATION_RE.find(reason).map(|m| m.as_str().to_string())
}

/// Checks whether an override reason explicitly names a benchmark arm (in full, tail, or stem).
pub fn reason_cites_arm(reason: &str, arm: &str) -> bool {
    let haystack = reason.to_lowercase();
    let arm_lower = arm.to_lowercase();
    if haystack.contains(&arm_lower) {
        return true;
    }
    let tail = arm.rsplit("::").next().unwrap_or(arm).to_lowercase();
    if haystack.contains(&tail) {
        return true;
    }
    let stem = tail.split('/').next().unwrap_or(&tail);
    if haystack.contains(stem) {
        return true;
    }
    false
}

/// Returns the subset of regressed arms that the override reason does NOT name.
pub fn unapproved_regressed_arms<'a>(reason: &str, regressed: &'a [String]) -> Vec<&'a str> {
    regressed
        .iter()
        .filter(|arm| !reason_cites_arm(reason, arm))
        .map(|s| s.as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_anchored_directive_is_accepted() {
        let body = "Summary\n\nremoves: tests/old.rs superseded by tests/new.rs\n";
        let reasons = directive_reasons(body, REMOVES);
        assert_eq!(reasons, vec!["tests/old.rs superseded by tests/new.rs"]);
        assert!(covers(&reasons, "tests/old.rs"));
    }

    #[test]
    fn html_comment_wrapping_is_accepted_and_closer_stripped() {
        let reasons = directive_reasons("<!-- deletes: benches/x.rs obsolete -->", REMOVES);
        assert_eq!(reasons, vec!["benches/x.rs obsolete"]);
    }

    #[test]
    fn prose_table_and_code_mentions_do_not_arm_the_override() {
        // The verbatim shape of the defect: documentation *about*
        // the directive must never act as the directive.
        let body = "\
We use the removes: token to justify deletions.
| token | meaning |
| removes: tests/old.rs | justification |
`removes: tests/old.rs because`
```
removes: tests/old.rs inside a fence
```
";
        assert!(directive_reasons(body, REMOVES).is_empty());
    }

    #[test]
    fn placeholders_are_rejected() {
        for body in [
            "removes:",
            "removes: <reason>",
            "removes: TODO",
            "removes: n/a",
            "<!-- removes: <reason> -->",
            "<!-- removes: <reason> --!>",
            "removes: ...",
        ] {
            assert!(
                directive_reasons(body, REMOVES).is_empty(),
                "placeholder accepted: {body:?}"
            );
        }
    }

    #[test]
    fn override_is_scoped_to_named_subjects() {
        let reasons =
            directive_reasons("removes: tests/legacy replaced by proptest suite", REMOVES);
        assert!(covers(&reasons, "tests/legacy/a.rs"));
        assert!(covers(&reasons, "tests/legacy/deep/b.rs"));
        assert!(!covers(&reasons, "tests/other.rs"));
        assert!(!covers(&reasons, "tests/legacy_extra.rs"));

        let by_name = directive_reasons("removes: old.rs, moved", REMOVES);
        assert!(covers(&by_name, "tests/old.rs"));
        assert!(!covers(&by_name, "tests/very_old.rs"));

        // A bare word must NOT act as a directory prefix.
        let bare = directive_reasons("removes: tests were refactored into benchmarks", REMOVES);
        assert!(!covers(&bare, "tests/a.rs"));
        assert!(!covers(&bare, "tests/legacy/a.rs"));

        // But an explicit directory prefix with a slash DOES cover it.
        let with_slash =
            directive_reasons("removes: tests/ were refactored into benchmarks", REMOVES);
        assert!(covers(&with_slash, "tests/a.rs"));

        // Dotfile and dotdirectory paths preserve leading dot
        let dotfile = directive_reasons("removes: .github/workflows/pages.yml retired", REMOVES);
        assert!(covers(&dotfile, ".github/workflows/pages.yml"));
        assert!(!covers(&dotfile, ".github/workflows/ci.yml"));

        let dotdir = directive_reasons("removes: .github/workflows/ retired", REMOVES);
        assert!(covers(&dotdir, ".github/workflows/pages.yml"));
    }

    #[test]
    fn unrelated_directive_name_is_ignored() {
        assert!(directive_reasons("allow-ignore: flaky_test on CI", REMOVES).is_empty());
        let r = directive_reasons("allow-ignore: flaky_test on CI", ALLOW_IGNORE);
        assert!(covers(&r, "flaky_test"));
        assert!(!covers(&r, "flaky"));
    }

    #[test]
    fn discipline_allow_directive_form_accepted() {
        let bare = "<!-- discipline:allow(deletion-rationale) tests -->";
        let bare_reasons = directive_reasons(bare, REMOVES);
        assert_eq!(bare_reasons, vec!["tests"]);
        assert!(!covers(&bare_reasons, "tests/legacy/old.rs"));

        let scoped = "<!-- discipline:allow(deletion-rationale) tests/legacy/ -->";
        let scoped_reasons = directive_reasons(scoped, REMOVES);
        assert_eq!(scoped_reasons, vec!["tests/legacy/"]);
        assert!(covers(&scoped_reasons, "tests/legacy/old.rs"));
    }

    #[test]
    fn multi_word_and_quoted_subjects_are_covered() {
        let r1 = directive_reasons(
            "allow-ignore: skips this test skipped for refactoring",
            ALLOW_IGNORE,
        );
        assert!(covers(&r1, "skips this test"));
        assert!(!covers(&r1, "this test"));

        let r2 = directive_reasons(
            "allow-ignore: temporarily disabled 'skips this test' pending fix",
            ALLOW_IGNORE,
        );
        assert!(covers(&r2, "skips this test"));
        assert!(!covers(&r2, "other test"));
    }

    #[test]
    fn allow_nul_directives_parsed_and_discriminate() {
        let r1 = directive_reasons(
            "allow-nul: tests/string_to_entry_005.phpt binary cache payload",
            ALLOW_NUL,
        );
        assert!(covers(&r1, "tests/string_to_entry_005.phpt"));
        assert!(!covers(&r1, "tests/other.phpt"));

        let r1_by_name = directive_reasons(
            "allow-nul: string_to_entry_005.phpt binary cache payload",
            ALLOW_NUL,
        );
        assert!(covers(&r1_by_name, "tests/string_to_entry_005.phpt"));

        let r2 = directive_reasons(
            "allow-nul-byte: src/bad.rs test fixture with binary payload",
            ALLOW_NUL,
        );
        assert!(covers(&r2, "src/bad.rs"));
        assert!(!covers(&r2, "src/good.rs"));

        let r3 = directive_reasons("allow-corrupt: tests/fixture.bin raw fuzz input", ALLOW_NUL);
        assert!(covers(&r3, "tests/fixture.bin"));
    }

    #[test]
    fn secrets_argv_ok_and_no_issue_directives_parsed() {
        let r1 = directive_reasons(
            "secrets-argv-ok: deploy.sh legacy container entrypoint",
            SECRETS_ARGV_OK,
        );
        assert!(covers(&r1, "deploy.sh"));
        assert!(!covers(&r1, "build.sh"));

        let r2 = directive_reasons(
            "<!-- discipline:allow(shell-secrets) scripts/run.sh dev test runner -->",
            SECRETS_ARGV_OK,
        );
        assert!(covers(&r2, "scripts/run.sh"));

        let r3 = directive_reasons("no-issue: trivial documentation fix", NO_ISSUE);
        assert_eq!(r3, vec!["trivial documentation fix"]);
        assert!(!is_placeholder(&r3[0]));

        let r4 = directive_reasons("no-issue: <reason>", NO_ISSUE);
        assert!(r4.is_empty());
    }

    #[test]
    fn discipline_namespaced_directives_parsed() {
        let r1 = directive_reasons("discipline: removes: old_test.rs refactored", REMOVES);
        assert_eq!(r1, vec!["old_test.rs refactored"]);
        assert!(covers(&r1, "old_test.rs"));

        let r2 = directive_reasons(
            "<!-- discipline: allow-assertion-drop: test_sync removed redundant assert -->",
            ALLOW_ASSERTION_DROP,
        );
        assert_eq!(r2, vec!["test_sync removed redundant assert"]);
        assert!(covers(&r2, "test_sync"));

        let r3 = directive_reasons(
            "discipline: allow-regression: bench_run #822 verified",
            ALLOW_REGRESSION,
        );
        assert_eq!(r3, vec!["bench_run #822 verified"]);
        assert!(covers(&r3, "bench_run"));
    }

    #[test]
    fn test_extract_citation_and_arm_naming() {
        assert_eq!(
            extract_citation("#480 trade: reduces random lookup branch mispredictions by 45.6% (1.49 vs 2.74 per probe) via independent L1 loads"),
            None
        );
        assert_eq!(extract_citation("intentional SIMD trade-off"), None);
        assert_eq!(extract_citation("approved by review"), None);
        assert_eq!(extract_citation("measured at commit 4c4e852"), None);
        assert_eq!(
            extract_citation(
                "TLB win, run https://github.com/orieg/expanse/actions/runs/33325789949"
            ),
            Some("https://github.com/orieg/expanse/actions/runs/33325789949".to_string())
        );
        assert_eq!(
            extract_citation("paired CI in results/baseline_vs_libjudy.json"),
            Some("results/baseline_vs_libjudy.json".to_string())
        );
        assert_eq!(
            extract_citation(
                "fallback in docs/benchmarks/concurrency/results/fallback_maturity.json"
            ),
            Some("docs/benchmarks/concurrency/results/fallback_maturity.json".to_string())
        );

        let pr822 = "zero-sharing per-writer coordination under feature lock-padded (refs CI run https://github.com/orieg/expanse/actions/runs/34490311084)";
        assert!(extract_citation(pr822).is_some());
        let regressed = vec![
            "instructions::cost::sync_map_insert/random".to_string(),
            "instructions::cost::sync_set_insert/random".to_string(),
        ];
        assert_eq!(
            unapproved_regressed_arms(pr822, &regressed),
            vec![
                "instructions::cost::sync_map_insert/random",
                "instructions::cost::sync_set_insert/random"
            ]
        );

        let reason_named = "sync_map_insert pays for the OLC bracket, run https://x/actions/runs/1";
        assert_eq!(
            unapproved_regressed_arms(
                reason_named,
                &["instructions::cost::sync_map_insert/random".to_string()]
            ),
            Vec::<&str>::new()
        );
        assert_eq!(
            unapproved_regressed_arms(
                "sync_map_insert only, run https://x/actions/runs/1",
                &regressed
            ),
            vec!["instructions::cost::sync_set_insert/random"]
        );
    }
}
