//! Pull request checklist reconciliation sentinel (`pr-checklist`).
//!
//! Reconciles ticked checklist items in PR descriptions against actual diff contents
//! to catch vacuous checkoffs (e.g. claiming tests/docs were added when 0 were modified).
//!
//! A test claim is backed by a changed test file, or by a test function the
//! change adds or extends in any file the language packs analyse: most Rust
//! unit tests live in a `mod tests` inside the source file they cover.
//!
//! The same test claim written as a plain bullet or sentence ("Added regression
//! tests for X") is read too, with a closed vocabulary, and reported at
//! `warning` at most: it is a heuristic over prose (#835).

use crate::ast::{default_registry, TestFn};
use crate::gitctx::ChangeKind;
use crate::guards::{Context, GateOutcome};
use crate::tokens::ALLOW_PR_CHECKLIST;
use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

pub const GATE: &str = "pr-checklist";

pub fn evaluate_pr_checklist(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.pr_checklist;
    let mut out = GateOutcome::new(GATE);

    let Some(pr_body) = &ctx.pr_body else {
        out.notes
            .push("no PR body supplied; PR checklist reconciliation skipped".to_string());
        return Ok(out);
    };

    let changed = ctx.git.changed_files()?;
    let changed_paths: Vec<&str> = changed.iter().map(|f| f.path.as_str()).collect();

    let mut has_tests = changed_paths
        .iter()
        .any(|p| p.contains("test") || p.contains("spec") || p.starts_with("tests/"));
    if !has_tests {
        let added = count_added_test_functions(ctx, &changed, &mut out.notes)?;
        if added > 0 {
            out.notes.push(format!(
                "test claim backed by {added} test function(s) added or extended in changed source files"
            ));
            has_tests = true;
        }
    }
    let has_docs = changed_paths
        .iter()
        .any(|p| p.ends_with(".md") || p.starts_with("docs/"));
    let has_benches = changed_paths
        .iter()
        .any(|p| p.contains("bench") || p.starts_with("benches/"));

    let box_re = Regex::new(r"(?i)^[ \t]*-[ \t]*\[[xX]\][ \t]*(.*)$").expect("static regex");
    let checked_count = pr_body.lines().filter(|l| box_re.is_match(l)).count();
    out.examined = checked_count;

    let claims = find_unsupported_claims(pr_body, has_tests, has_docs, has_benches);
    for claim in claims {
        let (kind, desc) = match claim.subject {
            "test" => (
                &crate::findings::CHECKLIST_CLAIMS_TESTS,
                "PR checklist claims tests added or extended, but diff adds or extends no test file or test function",
            ),
            "docs" => (
                &crate::findings::CHECKLIST_CLAIMS_DOCS,
                "PR checklist claims documentation updated, but diff contains zero documentation files",
            ),
            "bench" => (
                &crate::findings::CHECKLIST_CLAIMS_BENCHMARKS,
                "PR checklist claims benchmarks updated, but diff contains zero benchmark files",
            ),
            _ => (
                &crate::findings::CHECKLIST_CLAIM_UNSUPPORTED,
                "PR checklist claims unsupported change",
            ),
        };
        if let Some(ov) = ctx.find_override(GATE, kind, ALLOW_PR_CHECKLIST, claim.subject) {
            out.overrides.push(ov.clone());
            out.notes.push(format!(
                "override applied: `{}: {}` for {} checklist claim ({})",
                ov.directive, ov.reason, claim.subject, ov.source
            ));
        } else {
            out.add_violation(
                ctx.overridable(settings.severity),
                kind,
                "PR body",
                claim.line,
                desc,
                format!(
                    "checked item: `{}`; use `discipline:allow(pr-checklist): <reason>` to waive",
                    claim.item_text
                ),
            );
        }
    }

    let prose = find_unsupported_prose_test_claims(pr_body, has_tests);
    for claim in &prose.claims {
        let kind = &crate::findings::PROSE_CLAIMS_TESTS;
        if let Some(ov) = ctx.find_override(GATE, kind, ALLOW_PR_CHECKLIST, "test") {
            out.overrides.push(ov.clone());
            out.notes.push(format!(
                "override applied: `{}: {}` for test claim in prose ({})",
                ov.directive, ov.reason, ov.source
            ));
            continue;
        }
        out.add_violation(
            ctx.overridable(settings.severity).capped_at_warning(),
            kind,
            "PR body",
            claim.line,
            format!(
                "PR body says tests were added or changed (`{}`), but the diff adds or extends no test file or test function",
                claim.phrase
            ),
            "add the tests the description names or reword the claim; use `allow-pr-checklist: test <reason>` to waive",
        );
    }

    out.examined = checked_count + prose.units_read;
    Ok(out)
}

/// Test functions the change adds or extends across every changed file a
/// language pack analyses. A file the pack cannot extract adds nothing and is
/// named in `notes`, so an unreadable file never backs a claim.
fn count_added_test_functions(
    ctx: &Context,
    changed: &[crate::gitctx::ChangedFile],
    notes: &mut Vec<String>,
) -> Result<usize> {
    let registry = default_registry();
    let vocab = crate::guards::agent_diff::assert_vocabulary(ctx.config);
    let mut total = 0;
    for file in changed.iter().filter(|f| f.kind != ChangeKind::Deleted) {
        let Some(pack) = registry.find_pack(&file.path) else {
            continue;
        };
        let Some(head_src) = ctx.git.head_content(&file.path)? else {
            continue;
        };
        let head = match pack.extract(&file.path, &head_src, &vocab) {
            Ok(facts) => facts.tests,
            Err(e) => {
                notes.push(format!(
                    "{}: could not extract tests ({e}); not counted as test evidence",
                    file.path
                ));
                continue;
            }
        };
        let base = match ctx.git.base_content(&file.old_path)? {
            Some(src) => match registry.find_pack(&file.old_path) {
                Some(base_pack) => match base_pack.extract(&file.old_path, &src, &vocab) {
                    Ok(facts) => facts.tests,
                    Err(e) => {
                        notes.push(format!(
                            "{}: could not extract base tests ({e}); not counted as test evidence",
                            file.old_path
                        ));
                        continue;
                    }
                },
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        total += added_or_extended_tests(&base, &head);
    }
    Ok(total)
}

/// Tests the head side adds or extends relative to the base side of one file.
///
/// A head test with no base test of the same name is added; a same-named test
/// whose effective assertions grew is extended. Renames are netted out: a
/// test renamed without new assertions adds nothing.
pub fn added_or_extended_tests(base: &[TestFn], head: &[TestFn]) -> usize {
    let mut unmatched_base: Vec<&TestFn> = base.iter().collect();
    let mut unmatched_head = 0usize;
    let mut extended = 0usize;
    for h in head {
        match unmatched_base.iter().position(|b| b.name == h.name) {
            Some(i) => {
                let b = unmatched_base.swap_remove(i);
                if h.effective_asserts() > b.effective_asserts() {
                    extended += 1;
                }
            }
            None => unmatched_head += 1,
        }
    }
    unmatched_head.saturating_sub(unmatched_base.len()) + extended
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecklistClaim {
    pub line: usize,
    pub subject: &'static str,
    pub item_text: String,
}

pub fn find_unsupported_claims(
    pr_body: &str,
    has_tests: bool,
    has_docs: bool,
    has_benches: bool,
) -> Vec<ChecklistClaim> {
    let box_re = Regex::new(r"(?i)^[ \t]*-[ \t]*\[[xX]\][ \t]*(.*)$").expect("static regex");
    let mut claims = Vec::new();
    for (idx, line) in pr_body.lines().enumerate() {
        if let Some(caps) = box_re.captures(line) {
            let item_text = caps.get(1).map_or("", |m| m.as_str()).trim();
            let lower = item_text.to_ascii_lowercase();

            if (lower.contains("test") || lower.contains("tests"))
                && !lower.contains("no test")
                && !lower.contains("n/a")
                && !has_tests
            {
                claims.push(ChecklistClaim {
                    line: idx + 1,
                    subject: "test",
                    item_text: item_text.to_string(),
                });
            }

            if (lower.contains("doc") || lower.contains("docs") || lower.contains("documentation"))
                && !lower.contains("no doc")
                && !lower.contains("n/a")
                && !has_docs
            {
                claims.push(ChecklistClaim {
                    line: idx + 1,
                    subject: "docs",
                    item_text: item_text.to_string(),
                });
            }

            if (lower.contains("bench") || lower.contains("benchmark"))
                && !lower.contains("no bench")
                && !lower.contains("n/a")
                && !has_benches
            {
                claims.push(ChecklistClaim {
                    line: idx + 1,
                    subject: "bench",
                    item_text: item_text.to_string(),
                });
            }
        }
    }
    claims
}

/// A plain bullet or sentence of a PR body that says tests were added or changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProseTestClaim {
    /// 1-based line of the PR body the claim starts on.
    pub line: usize,
    /// The words that make it a claim, as written.
    pub phrase: String,
}

/// What [`find_prose_test_claims`] read: the claims, and how many paragraphs and list
/// items it looked at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProseTestClaims {
    pub claims: Vec<ProseTestClaim>,
    pub units_read: usize,
}

/// Verbs that say tests were added or changed, every tense (`docs/GATES.md`, `pr-checklist`).
const CLAIM_VERBS: &str = "add|adds|added|adding|write|writes|wrote|written|writing|extend|extends|extended|extending|update|updates|updated|updating|expand|expands|expanded|expanding";
/// Verbs that say tests were added only with `test coverage` as their object.
const COVERAGE_VERBS: &str =
    "improve|improves|improved|improving|increase|increases|increased|increasing";
/// The participles of [`CLAIM_VERBS`] in a passive claim (`tests were added`).
const PASSIVE_VERBS: &str = "added|written|extended|updated|expanded";
/// Words that may stand between a verb and the test noun (`added two new regression tests`).
const MODIFIERS: &str = r"a|an|the|new|more|extra|additional|some|several|missing|regression|unit|integration|e2e|end-to-end|property|property-based|snapshot|golden|fuzz|focused|targeted|dedicated|corresponding|matching|basic|existing|its|their|our|one|two|three|four|five|six|seven|eight|nine|ten|\d+";
/// The test nouns.
const NOUN: &str = r"test[ -]cases?|test coverage|tests?";

fn claim_re(template: &str) -> Regex {
    let pattern = template
        .replace("{VERB}", CLAIM_VERBS)
        .replace("{COVER}", COVERAGE_VERBS)
        .replace("{PASSIVE}", PASSIVE_VERBS)
        .replace("{MOD}", &format!(r"(?:(?:{MODIFIERS})\s+){{0,4}}"))
        .replace("{NOUN}", NOUN);
    Regex::new(&format!("(?i){pattern}")).expect("static claim regex")
}

/// `added regression tests`, `improves test coverage`.
static VERB_CLAIM_RE: LazyLock<Regex> = LazyLock::new(|| {
    claim_re(r"\b(?:(?:{VERB})\s+{MOD}(?:{NOUN})|(?:{COVER})\s+(?:the\s+)?test coverage)\b")
});
/// `added a flag and tests`.
static COORDINATED_CLAIM_RE: LazyLock<Regex> =
    LazyLock::new(|| claim_re(r"\b(?:{VERB})\b.{0,60}?\s(?:and|plus)\s+{MOD}(?:{NOUN})\b"));
/// `regression tests were added`, `tests added`.
static PASSIVE_CLAIM_RE: LazyLock<Regex> = LazyLock::new(|| {
    claim_re(
        r"\b{MOD}(?:{NOUN})\s+(?:(?:were|was|have\s+been|has\s+been|are|is|got)\s+)?(?:{PASSIVE})\b",
    )
});
/// `Tests for negative inputs` at the start of a bullet or sentence.
static NOMINAL_CLAIM_RE: LazyLock<Regex> =
    LazyLock::new(|| claim_re(r"^{MOD}(?:{NOUN})\s+(?:for|covering)\b"));
/// `with regression tests`, closing the clause or followed by what they cover, after the
/// change they come with: a clause that opens with `with` describes, it does not claim
/// (`(lib, integration) with two test cases`).
static WITH_CLAIM_RE: LazyLock<Regex> = LazyLock::new(|| {
    claim_re(r"\b(?:with|plus)\s+{MOD}(?:{NOUN})\s*(?:$|(?:for|covering|that|included|added)\b)")
});

/// Where one claim ends and the next begins inside a paragraph or list item.
static CLAUSE_SPLIT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[.;!?:]+(?:\s+|$)|\s[—–]\s|\s--?\s|[()|]").expect("static regex")
});
static BULLET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:[-*+•]|\d+[.)])\s+").expect("static regex"));
static CHECKBOX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:[-*+]|\d+[.)])\s*\[[ xX]\]").expect("static regex"));
/// Another change named as the one that added the tests: `tests added by #505`,
/// `#505 added tests`, `tests added in 1a2b3c4`.
static OTHER_CHANGE_AFTER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:by|in|from|since|via)\s+(?:pr\s+|pull request\s+|commit\s+)?(?:#\d+|[0-9a-f]{7,40}\b)")
        .expect("static regex")
});
static OTHER_CHANGE_BEFORE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:#\d+|\b[0-9a-f]{7,40})\s*$").expect("static regex"));

/// A word anywhere before the claim that negates it (`no tests added`, `did not add tests`).
const NEGATIONS: &[&str] = &[
    "no", "not", "never", "without", "cannot", "nor", "todo", "tbd", "fixme", "n/a", "skip",
    "skipped", "skipping",
];
/// The word just before the claim when the claim is deferred, asked for, or describes
/// something else (`to add tests`, `will add tests`, `a change that adds tests`).
const DEFERRALS: &[&str] = &[
    "to", "will", "would", "should", "could", "can", "may", "might", "must", "shall", "need",
    "needs", "please", "that", "which", "who", "when", "whenever", "if", "whether", "unless",
    "once", "until", "lets", "let's",
];
/// Words after the claim that say it is not done (`tests to follow`, `tests not needed`).
const AFTER_NEGATIONS: &[&str] = &[
    "not needed",
    "not required",
    "not necessary",
    "not applicable",
    "to follow",
    "follow-up",
    "follow up",
    "followup",
    "later",
    "pending",
    "todo",
    "tbd",
    "n/a",
    "unnecessary",
    "deferred",
    "out of scope",
];
/// Words after a test noun that make it an adjective (`added test fixtures`, `the tests
/// directory`) or a statement about tests that already run (`tests pass`).
const NOUN_FOLLOWERS: &[&str] = &[
    "directory",
    "dir",
    "dirs",
    "folder",
    "helper",
    "helpers",
    "util",
    "utils",
    "utility",
    "utilities",
    "harness",
    "infra",
    "infrastructure",
    "data",
    "fixture",
    "fixtures",
    "config",
    "configuration",
    "workflow",
    "workflows",
    "job",
    "jobs",
    "step",
    "steps",
    "target",
    "targets",
    "runner",
    "runners",
    "framework",
    "dependency",
    "dependencies",
    "command",
    "commands",
    "script",
    "scripts",
    "mode",
    "env",
    "environment",
    "server",
    "account",
    "user",
    "users",
    "plan",
    "matrix",
    "tooling",
    "output",
    "outputs",
    "result",
    "results",
    "log",
    "logs",
    "name",
    "names",
    "id",
    "ids",
    "flag",
    "flags",
    "feature",
    "gate",
    "gates",
    "crate",
    "crates",
    "workspace",
    "package",
    "pass",
    "passes",
    "passed",
    "passing",
    "run",
    "runs",
    "ran",
    "fail",
    "fails",
    "failed",
    "failing",
    "still",
    "already",
];
/// Verbs after a nominal `Tests for X` that make it a statement about tests that exist.
const NOMINAL_STATES: &[&str] = &[
    "pass",
    "passes",
    "passed",
    "passing",
    "exist",
    "exists",
    "existed",
    "live",
    "lives",
    "run",
    "runs",
    "ran",
    "fail",
    "fails",
    "failed",
    "failing",
    "remain",
    "remains",
    "already",
    "still",
    "is",
    "are",
    "was",
    "were",
    "cover",
    "covers",
    "covered",
    "unchanged",
];

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .replace('’', "'")
        .split(|c: char| !(c.is_alphanumeric() || matches!(c, '\'' | '/' | '-')))
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

fn is_negation(word: &str) -> bool {
    NEGATIONS.contains(&word) || word.ends_with("n't")
}

fn is_deferral(word: &str) -> bool {
    DEFERRALS.contains(&word) || word.ends_with("'ll")
}

/// Whether the text before a claim in its clause negates or defers it.
fn negated_before(prefix: &str) -> bool {
    let w = words(prefix);
    w.iter().any(|x| is_negation(x)) || w.last().is_some_and(|x| is_deferral(x))
}

/// Whether the claim is about another change: one named right before or right after it.
fn names_other_change(prefix: &str, suffix: &str) -> bool {
    OTHER_CHANGE_BEFORE_RE.is_match(prefix) || OTHER_CHANGE_AFTER_RE.is_match(suffix)
}

/// Whether the text after a claim in its clause says it is not done.
fn negated_after(suffix: &str) -> bool {
    let lower = format!(" {} ", words(suffix).join(" "));
    AFTER_NEGATIONS
        .iter()
        .any(|n| lower.contains(&format!(" {n} ")))
}

/// Whether the test noun at the end of a match is used as an adjective or in a statement
/// about tests that already exist: followed by a hyphen, an underscore or a listed word.
fn noun_continues(suffix: &str) -> bool {
    if suffix.starts_with(['-', '_']) {
        return true;
    }
    words(suffix)
        .first()
        .is_some_and(|w| NOUN_FOLLOWERS.contains(&w.as_str()))
}

/// The claim one clause makes, if any: the matched words.
fn clause_claim(clause: &str) -> Option<String> {
    let checks: [(&Regex, bool); 4] = [
        (&VERB_CLAIM_RE, true),
        (&COORDINATED_CLAIM_RE, true),
        (&WITH_CLAIM_RE, false),
        (&PASSIVE_CLAIM_RE, false),
    ];
    for (re, check_noun) in checks {
        for m in re.find_iter(clause) {
            let (prefix, suffix) = (&clause[..m.start()], &clause[m.end()..]);
            if negated_before(prefix) || negated_after(suffix) || names_other_change(prefix, suffix)
            {
                continue;
            }
            if check_noun && noun_continues(suffix) {
                continue;
            }
            if std::ptr::eq(re, &*WITH_CLAIM_RE) && prefix.trim().is_empty() {
                continue;
            }
            return Some(m.as_str().split_whitespace().collect::<Vec<_>>().join(" "));
        }
    }
    let m = NOMINAL_CLAIM_RE.find(clause)?;
    let suffix = &clause[m.end()..];
    let suffix_words = words(suffix);
    let passive = PASSIVE_VERBS
        .split('|')
        .any(|v| suffix_words.iter().any(|w| w == v));
    let state = suffix_words
        .iter()
        .any(|w| NOMINAL_STATES.contains(&w.as_str()));
    (!negated_after(suffix) && (passive || !state))
        .then(|| m.as_str().split_whitespace().collect::<Vec<_>>().join(" "))
}

/// `body` with HTML comments, directive lines and checkbox items blanked, line for line:
/// none of them is a claim the description makes in prose. A ticked checkbox is the
/// checkbox rule's, an unticked one is a to-do.
fn mask_non_prose(body: &str) -> String {
    let directive_lines: Vec<usize> =
        crate::tokens::directive_lines(body, crate::tokens::OverrideSource::PrBody)
            .into_iter()
            .map(|(i, _)| i)
            .collect();
    let mut in_comment = false;
    let mut out = Vec::new();
    for (idx, line) in body.lines().enumerate() {
        let mut kept = String::new();
        let mut rest = line;
        loop {
            if in_comment {
                match rest.find("-->") {
                    Some(end) => {
                        in_comment = false;
                        rest = &rest[end + 3..];
                    }
                    None => break,
                }
            } else {
                match rest.find("<!--") {
                    Some(start) => {
                        kept.push_str(&rest[..start]);
                        in_comment = true;
                        rest = &rest[start + 4..];
                    }
                    None => {
                        kept.push_str(rest);
                        break;
                    }
                }
            }
        }
        if directive_lines.contains(&idx) || CHECKBOX_RE.is_match(&kept) {
            kept.clear();
        }
        out.push(kept);
    }
    out.join("\n")
}

/// Plain bullets and sentences of a PR body that say tests were added or changed
/// (`docs/GATES.md`, `pr-checklist`): a claim verb before a test noun (`added regression
/// tests`, `Add tests for`), the passive (`tests were added`), a bullet or sentence that
/// opens with `Tests for` or `Test coverage for`, and `with tests`. Code spans, fenced
/// and indented code, quoted lines (`>`), double-quoted text, HTML comments, directive
/// lines, checkbox items, headings, questions and `TODO:` lines are not read. A claim
/// negated (`no tests`, `without tests`, `tests not needed`) or deferred (`will add
/// tests`, `tests to follow`) is not one. One claim per paragraph or list item.
pub fn find_prose_test_claims(pr_body: &str) -> ProseTestClaims {
    use crate::guards::claim_labels::{
        extract_prose_units, is_question_or_todo, strip_non_claim_spans,
    };
    let masked = mask_non_prose(pr_body);
    let mut result = ProseTestClaims::default();
    for unit in extract_prose_units(&masked) {
        result.units_read += 1;
        // The unit as one text, wrapped lines joined, with where each line starts.
        let mut text = String::new();
        let mut starts: Vec<(usize, usize)> = Vec::new();
        for (ln, line) in &unit.lines {
            if is_question_or_todo(line) {
                continue;
            }
            let cleaned = strip_non_claim_spans(line)
                .replace("**", "")
                .replace("__", "");
            let cleaned = BULLET_RE.replace(&cleaned, "");
            if !text.is_empty() {
                text.push(' ');
            }
            starts.push((text.len(), *ln));
            text.push_str(&cleaned);
        }
        let mut offset = 0;
        let mut boundaries: Vec<(usize, usize)> = CLAUSE_SPLIT_RE
            .find_iter(&text)
            .map(|m| (m.start(), m.end()))
            .collect();
        boundaries.push((text.len(), text.len()));
        for (end, next) in boundaries {
            let raw = &text[offset..end];
            let lead = raw.len() - raw.trim_start().len();
            let clause = raw.trim();
            let clause_start = offset + lead;
            offset = next;
            let Some(phrase) = clause_claim(clause) else {
                continue;
            };
            let first = phrase.split(' ').next().unwrap_or_default();
            let at = clause_start + clause.find(first).unwrap_or(0);
            let line = starts
                .iter()
                .rev()
                .find(|(s, _)| *s <= at)
                .map_or(unit.lines[0].0, |(_, ln)| *ln);
            result.claims.push(ProseTestClaim { line, phrase });
            break;
        }
    }
    result
}

/// [`find_prose_test_claims`] reconciled against the diff: no claim stands when the
/// change adds or extends a test (`has_tests`, the notion the checkbox rule uses).
pub fn find_unsupported_prose_test_claims(pr_body: &str, has_tests: bool) -> ProseTestClaims {
    let mut read = find_prose_test_claims(pr_body);
    if has_tests {
        read.claims.clear();
    }
    read
}

#[cfg(test)]
mod tests {
    use super::added_or_extended_tests;
    use crate::ast::TestFn;
    use crate::config::{PrChecklistGate, Severity};

    fn t(name: &str, asserts: usize) -> TestFn {
        TestFn {
            name: name.to_string(),
            total_asserts: asserts,
            ..Default::default()
        }
    }

    #[test]
    fn added_or_extended_tests_counts_new_and_grown_tests_only() {
        let base = [t("tests::doubles", 1)];
        // Positive: a new test beside an existing one.
        assert_eq!(
            added_or_extended_tests(&base, &[t("tests::doubles", 1), t("tests::triples", 1)]),
            1
        );
        // Positive: an existing test gains an assertion.
        assert_eq!(added_or_extended_tests(&base, &[t("tests::doubles", 2)]), 1);
        // Negative: nothing about the tests changed.
        assert_eq!(added_or_extended_tests(&base, &[t("tests::doubles", 1)]), 0);
        // Negative: a rename is not a new test.
        assert_eq!(added_or_extended_tests(&base, &[t("tests::doubled", 1)]), 0);
        // Negative: a test removed.
        assert_eq!(added_or_extended_tests(&base, &[]), 0);
        // Positive: a new file's tests are all new.
        assert_eq!(added_or_extended_tests(&[], &[t("a", 0), t("b", 1)]), 2);
    }

    fn claims(body: &str) -> Vec<(usize, String)> {
        super::find_prose_test_claims(body)
            .claims
            .into_iter()
            .map(|c| (c.line, c.phrase))
            .collect()
    }

    #[test]
    fn prose_test_claim_bullet_and_sentence_forms() {
        // Positive: a bullet, first-person past tense.
        assert_eq!(
            claims("Fixes the doubling bug.\n\n- Added regression tests for negative inputs\n"),
            vec![(3, "Added regression tests".to_string())]
        );
        // Positive: a sentence after another sentence.
        assert_eq!(
            claims("Fixes the doubling bug. I added regression tests for negative inputs.\n"),
            vec![(1, "added regression tests".to_string())]
        );
        // Positive: imperative and present tense.
        assert_eq!(claims("- Add regression tests for the parser\n").len(), 1);
        assert_eq!(
            claims("This change adds unit tests for `parse`.\n").len(),
            1
        );
        assert_eq!(
            claims("- Updated the existing tests to the new API\n").len(),
            1
        );
        // Positive: passive, nominal, `with tests`, coverage, coordinated nouns.
        assert_eq!(
            claims("- Regression tests were added for the parser\n").len(),
            1
        );
        assert_eq!(claims("- Tests for negative inputs\n").len(), 1);
        assert_eq!(claims("- Test coverage for the reader\n").len(), 1);
        assert_eq!(
            claims("Fixes the reader, with regression tests.\n").len(),
            1
        );
        // Negative: a clause that opens with `with` describes something else.
        assert!(claims("Three documents (lib, doctest) with two test cases.\n").is_empty());
        assert_eq!(claims("Improves test coverage of the writer.\n").len(), 1);
        assert_eq!(claims("- Added the flag and tests for it\n").len(), 1);
        // Positive: a sentence wrapped over two lines is read whole, on its first line.
        assert_eq!(
            claims("Fixes the bug.\nI added regression\ntests for it.\n"),
            vec![(2, "added regression tests".to_string())]
        );
        // Negative: the same paragraph without a claim.
        assert!(claims("Fixes the doubling bug.\n\n- Negative inputs now double\n").is_empty());
    }

    #[test]
    fn prose_test_claim_negated_or_deferred_is_not_a_claim() {
        for body in [
            "- No tests added: the change is a rename\n",
            "No tests: the change is a rename.\n",
            "- Refactor without tests\n",
            "Tests not needed for a comment change.\n",
            "- Did not add tests\n",
            "I didn't add tests.\n",
            "TODO: tests\n",
            "TODO: add tests for the parser\n",
            "- Will add tests in a follow-up\n",
            "- Regression tests to follow\n",
            "Should we add tests for this?\n",
            "The gate reports a change that adds tests without a test file.\n",
            "Remember to add tests.\n",
            "Two drift tests added by #505 compare the installers.\n",
            "- #505 added tests for the installers\n",
        ] {
            assert!(claims(body).is_empty(), "{body:?}: {:?}", claims(body));
        }
    }

    #[test]
    fn prose_test_claim_outside_prose_is_not_read() {
        for body in [
            "Run `added regression tests` as a phrase.\n",
            "```\nAdded regression tests\n```\n",
            "    Added regression tests\n",
            "> Added regression tests for negative inputs\n",
            "<!-- Added regression tests for negative inputs -->\n",
            "<!--\nDescribe the tests you added:\n- Added tests for X\n-->\n",
            "The reviewer wrote \"added regression tests\" in the template.\n",
            "## Added regression tests\n",
            "- [x] Added regression tests for negative inputs\n",
            "- [ ] Added regression tests for negative inputs\n",
            "allow-pr-checklist: test regression tests were added in a separate PR\n",
        ] {
            assert!(claims(body).is_empty(), "{body:?}: {:?}", claims(body));
        }
    }

    #[test]
    fn prose_tests_that_exist_or_pass_are_not_claims_of_change() {
        // A statement that tests exist or pass says nothing about what the change added;
        // a count of passing tests is a different claim (`docs/GATES.md`).
        for body in [
            "All 57 tests pass.\n",
            "- Existing tests cover this path\n",
            "- Tests for the parser pass\n",
            "Tests for the reader live in `tests/reader.rs`.\n",
            "- Added test fixtures for the reader\n",
            "- Added a tests directory to the ignore list\n",
            "- Added test-only feature flag\n",
            "- Updated test plan\n",
        ] {
            assert!(claims(body).is_empty(), "{body:?}: {:?}", claims(body));
        }
        // A nominal claim whose clause says the tests were added is one.
        assert_eq!(claims("- Tests for the parser were added\n").len(), 1);
    }

    #[test]
    fn prose_test_claim_backed_by_a_test_change_stands_down() {
        let body = "Fixes the doubling bug.\n\n- Added regression tests for negative inputs\n";
        // Positive: no test in the diff.
        assert_eq!(
            super::find_unsupported_prose_test_claims(body, false)
                .claims
                .len(),
            1
        );
        // Negative: the diff adds or extends a test.
        let backed = super::find_unsupported_prose_test_claims(body, true);
        assert!(backed.claims.is_empty());
        assert_eq!(backed.units_read, 2);
    }

    #[test]
    fn prose_test_claim_documented_misses() {
        // `docs/GATES.md`, `pr-checklist`, "What it does NOT catch".
        for body in [
            "- Covered the new branch\n",
            "- Tested the parser with a new case\n",
            "I made sure not to skip it, and added tests.\n",
        ] {
            assert!(claims(body).is_empty(), "{body:?}: {:?}", claims(body));
        }
        // A negation after the claim does not cancel it.
        assert_eq!(
            claims("- Added tests for the case where no input is given\n").len(),
            1
        );
    }

    #[test]
    fn prose_test_claim_one_per_item() {
        let found = claims("- Added tests for A and added tests for B\n- Added unit tests\n");
        assert_eq!(found.iter().map(|c| c.0).collect::<Vec<_>>(), vec![1, 2]);
    }

    #[test]
    fn test_pr_checklist_defaults() {
        let gate = PrChecklistGate::default();
        assert!(!gate.enabled);
        assert_eq!(gate.severity, Severity::Error);
    }
}
