//! The end of a turn: did the agent announce an action and stop without taking it?
//!
//! A check of `discipline hook run` only (docs/ROADMAP.md, Phase 17). It is not a gate:
//! a gate judges a diff, and CI never sees an agent's final message. It runs after the
//! change check has passed, on the final assistant message the agent's stop payload
//! carries, and is off unless `[hooks.premature-stop]` enables it on the base ref.
//!
//! Two things are read, and they are kept apart (AGENTS.md §2.3):
//! * That the turn ended with no tool call after the message is the stop event itself,
//!   the agent's own structured record. It is never inferred from text.
//! * What the last sentence says is prose. No grammar parses it, so the matcher is a
//!   closed list of openings anchored at the start of the last sentence outside fenced
//!   code, and `tests/fixtures/premature_stop_corpus.json` is its contract, as
//!   `tests/fixtures/time_estimates_corpus.json` is for `time-estimates`.
//!
//! The matcher reads English only; the corpus records announcements in other languages
//! as misses.

use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// One kind of end-of-turn defect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnKind {
    /// The final message ends by announcing the agent's own next step.
    AnnouncedAction,
    /// The final message ends in tool-call markup written out as text, so no call ran.
    ToolCallAsText,
}

/// The first segment of every code here. Neither a gate id nor `policy`
/// (`tests::codes_are_in_their_own_namespace`).
pub const NAMESPACE: &str = "turn";

impl TurnKind {
    pub const ALL: [TurnKind; 2] = [TurnKind::AnnouncedAction, TurnKind::ToolCallAsText];

    /// The stable code, frozen once released (docs/ARCHITECTURE.md §3.2).
    pub fn code(self) -> &'static str {
        match self {
            TurnKind::AnnouncedAction => "turn/announced-action-not-taken",
            TurnKind::ToolCallAsText => "turn/tool-call-written-as-text",
        }
    }
}

/// What the final message of a turn was found to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub kind: TurnKind,
    /// The sentence that announced the action, on one line. Empty for a tool call
    /// written as text.
    pub sentence: String,
}

impl Verdict {
    /// The text handed back to the model. The sentence is the model's own, quoted as
    /// data on one line with its backticks removed.
    pub fn refusal(&self) -> String {
        match self.kind {
            TurnKind::AnnouncedAction => format!(
                "discipline [{}]: you ended your turn after announcing an action and did not take it: `{}`. Do it now, or say in one sentence why you cannot.\n",
                self.kind.code(),
                self.sentence
            ),
            TurnKind::ToolCallAsText => format!(
                "discipline [{}]: your last message contains a tool call written out as text, so it did not run. Make the call with the tool, or say in one sentence why you cannot.\n",
                self.kind.code()
            ),
        }
    }
}

/// Fillers a sentence may open with before the announcement.
const LEAD: &str = r"(?:(?:ok(?:ay)?|alright|right|good|great|perfect|so|now|next|then|first|finally|and|but|also)[,.!:]?\s+)*";

/// Openings that announce the speaker's own next step as immediate. A plain "I'll ..."
/// is not one: it is as often a promise to report on work that is still running.
const INTENT: &str = r"(?:let me|let['’]s|lets|i['’]m (?:now )?going to|i am (?:now )?going to|time to|now to|(?:now|next),? (?:i['’]ll|i will|i need to|i have to|i can|we)|i['’]ll (?:now|next|go ahead and|start by|begin by|proceed to|first)|i will (?:now|next|go ahead and|start by|begin by|proceed to|first)|proceeding to|moving on to|starting with|i need to (?:check|look|read|run|find|fix|see|examine|verify|investigate|inspect|update|add|write))";

static ANNOUNCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"(?i)^{LEAD}{INTENT}\b")).expect("the announcement pattern compiles")
});

/// The sentence hands the turn to the person, or defers the action to a condition.
static HANDS_OVER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\?\s*$|let me know|\bif you\b|\bif that\b|\bif so\b|\bonce\b|\bwhen\b|\bafter (?:you|that|the|it|ci)\b|\buntil\b|\bunless\b|would you like|want me to|should i\b|shall i\b|say (?:the word|go|yes)|your call|go-ahead|\bapprov|\bconfirm\b|\bwait(?:ing)? for\b|\bnext (?:session|time|step is yours)\b|\blater\b|\btomorrow\b|\bin a follow-up\b|\bfollow-up pr\b|\bwhenever\b|\bplease\b|\bfeel free\b|\bas soon as\b|\bthen i['’]ll\b",
    )
    .expect("the hand-over pattern compiles")
});

/// An opening that reads like an announcement and is not an action on the work.
static NOT_AN_ACTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)^{LEAD}(?:let me|i['’]ll|i will|let['’]s)\s+(?:know|explain|summari[sz]e|be (?:clear|honest|direct)|recap|clarify|answer|address|break (?:this|that|it) down|walk you|note|flag|stop here|leave|hold|wait|pause|stand by|keep an eye|be here|assume|say|put it)\b"
    ))
    .expect("the non-action pattern compiles")
});

/// Tool-call markup at the end of a message: a closing tag of the call formats models
/// emit as text, or an opening one with nothing after it but its arguments.
static TOOL_MARKUP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)</?(?:tool_call|function_calls?|invoke|parameter)\b[^>]*>\s*$|<function=|<parameter=|<\|tool_call",
    )
    .expect("the tool-markup pattern compiles")
});

static LIST_MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[-*+]|\d+[.)])\s+").expect("the list-marker pattern compiles")
});

/// Longest last sentence read as an announcement, in bytes. A longer one is a paragraph
/// without sentence breaks, which this matcher does not judge.
const MAX_SENTENCE: usize = 400;

/// How much of the end of a message is searched for tool-call markup, in characters.
const MARKUP_TAIL: usize = 300;

/// `text` with every fenced block replaced by a space. An unclosed fence runs to the end.
fn without_fences(text: &str) -> String {
    text.split("```")
        .enumerate()
        .filter(|(i, _)| i % 2 == 0)
        .map(|(_, part)| part)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The last sentence of a message, outside fenced code: the last non-empty line with its
/// list marker and emphasis characters removed, cut after the last sentence end that is
/// followed by whitespace and an upper-case letter, a quote or a parenthesis.
pub fn tail_sentence(text: &str) -> String {
    let outside = without_fences(text);
    let Some(last) = outside.lines().map(str::trim).rfind(|l| !l.is_empty()) else {
        return String::new();
    };
    let last = LIST_MARKER.replace(last, "");
    let last: String = last
        .chars()
        .filter(|c| !matches!(c, '*' | '_' | '`'))
        .collect();
    let last = last.trim();
    let chars: Vec<(usize, char)> = last.char_indices().collect();
    let mut start = 0;
    for (n, &(_, c)) in chars.iter().enumerate() {
        if !matches!(c, '.' | '!' | '?') {
            continue;
        }
        let mut next = n + 1;
        while chars.get(next).is_some_and(|(_, c)| c.is_whitespace()) {
            next += 1;
        }
        if next == n + 1 {
            continue;
        }
        if let Some(&(at, c)) = chars.get(next) {
            if c.is_ascii_uppercase() || matches!(c, '"' | '\'' | '(') {
                start = at;
            }
        }
    }
    last[start..].trim().to_string()
}

/// Whether the message ends by announcing the speaker's own next step.
pub fn announces_action(text: &str) -> bool {
    let sentence = tail_sentence(text);
    !sentence.is_empty()
        && sentence.len() <= MAX_SENTENCE
        && ANNOUNCE.is_match(&sentence)
        && !NOT_AN_ACTION.is_match(&sentence)
        && !HANDS_OVER.is_match(&sentence)
}

/// Whether the message ends in tool-call markup written out as text.
pub fn tool_call_as_text(text: &str) -> bool {
    let trimmed = text.trim();
    let from = trimmed
        .char_indices()
        .rev()
        .nth(MARKUP_TAIL - 1)
        .map(|(i, _)| i)
        .unwrap_or(0);
    TOOL_MARKUP.is_match(&trimmed[from..])
}

/// Judges a turn's final assistant message. `tool_markup` is the
/// `hooks.premature-stop.tool_call_as_text` switch.
pub fn judge(last_message: &str, tool_markup: bool) -> Option<Verdict> {
    if tool_markup && tool_call_as_text(last_message) {
        return Some(Verdict {
            kind: TurnKind::ToolCallAsText,
            sentence: String::new(),
        });
    }
    announces_action(last_message).then(|| Verdict {
        kind: TurnKind::AnnouncedAction,
        sentence: tail_sentence(last_message),
    })
}

/// Where a session's count of refused stops lives: `<git dir>/discipline/turn-<session>`
/// (never tracked). `None` without a session id that leaves a name once reduced to
/// letters, digits and dashes.
pub fn counter_path(git_dir: &Path, session: &str) -> Option<PathBuf> {
    let name: String = session
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(96)
        .collect();
    (!name.is_empty()).then(|| git_dir.join("discipline").join(format!("turn-{name}")))
}

/// Stops refused so far in the session whose counter is `path`. A counter that is
/// missing or unreadable counts as none.
pub fn refused_so_far(path: &Path) -> u32 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(0)
}

/// Records one more refused stop. An error means the cap cannot be kept.
pub fn record_refusal(path: &Path, so_far: u32) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, (so_far + 1).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct Case {
        id: String,
        text: String,
        is_premature_stop: bool,
        kind: Option<String>,
        category: String,
        lang: String,
    }

    fn corpus() -> Vec<Case> {
        serde_json::from_str(include_str!("../tests/fixtures/premature_stop_corpus.json"))
            .expect("valid corpus json")
    }

    /// The corpus is the matcher's contract: every English final message is judged as
    /// labelled, with the labelled kind. An announcement in another language is a
    /// recorded miss, pinned so that a matcher which starts reading one says so here.
    #[test]
    fn corpus_is_judged_as_labelled_and_other_languages_are_recorded_misses() {
        let cases = corpus();
        assert_eq!(cases.len(), 102, "corpus size changed; update this count");
        let english_positives = cases
            .iter()
            .filter(|c| c.is_premature_stop && c.lang == "en")
            .count();
        let negatives = cases.iter().filter(|c| !c.is_premature_stop).count();
        assert!(english_positives >= 25 && negatives >= 60, "corpus thinned");

        let mut wrong = Vec::new();
        for c in &cases {
            let verdict = judge(&c.text, true);
            let expected = c.is_premature_stop && c.lang == "en";
            let kind = verdict.as_ref().map(|v| match v.kind {
                TurnKind::AnnouncedAction => "announced_action",
                TurnKind::ToolCallAsText => "tool_call_as_text",
            });
            if verdict.is_some() != expected || (expected && kind != c.kind.as_deref()) {
                wrong.push(format!(
                    "{} ({}, {}): judged {kind:?}, labelled {:?}",
                    c.id, c.category, c.lang, c.kind
                ));
            }
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    #[test]
    fn an_announcement_is_read_from_the_last_sentence_only() {
        // Positive control.
        assert!(announces_action("Tests pass. Now let me run clippy."));
        // The same words earlier in the message, with the work reported after them.
        assert!(!announces_action(
            "Now let me run clippy.\n\nClippy is clean."
        ));
        // Inside a fence it is quoted text, not the message's own last sentence.
        assert!(!announces_action(
            "The model wrote:\n\n```text\nNow let me run clippy.\n```"
        ));
        // A question, a hand-over and a condition each clear it.
        assert!(!announces_action("Now, shall I run clippy?"));
        assert!(!announces_action("Let me know if you want clippy run."));
        assert!(!announces_action("Let me run clippy once CI reports."));
        // A plain promise is not an announcement.
        assert!(!announces_action("I'll report when the run lands."));
    }

    #[test]
    fn the_refusal_quotes_the_sentence_on_one_line_without_backticks() {
        let v = judge(
            "Found it in `parse_header`.\n\n- **Now let me fix `parse_header`.**",
            true,
        )
        .expect("an announcement");
        assert_eq!(v.kind, TurnKind::AnnouncedAction);
        assert_eq!(v.sentence, "Now let me fix parseheader.");
        let text = v.refusal();
        assert!(
            text.contains("[turn/announced-action-not-taken]")
                && text.contains("`Now let me fix parseheader.`")
                && text.ends_with("why you cannot.\n"),
            "{text}"
        );
        assert_eq!(text.lines().count(), 1, "{text}");
    }

    #[test]
    fn tool_markup_is_judged_only_when_switched_on_and_only_at_the_end() {
        let written = "Let me fix it.\n</parameter>\n</function>\n</tool_call>";
        assert_eq!(
            judge(written, true).map(|v| v.kind),
            Some(TurnKind::ToolCallAsText)
        );
        assert_eq!(judge(written, false), None);
        // Markup the message only talks about, with prose after it.
        let mentioned = format!(
            "The model printed </tool_call> as text. {}",
            "x".repeat(400)
        );
        assert_eq!(judge(&mentioned, true), None);
    }

    #[test]
    fn codes_are_in_their_own_namespace() {
        let gates: Vec<&str> = crate::config::GATES.iter().map(|g| g.id).collect();
        assert!(!gates.contains(&NAMESPACE));
        assert_ne!(NAMESPACE, crate::refusals::NAMESPACE);
        for kind in TurnKind::ALL {
            let (ns, name) = kind.code().split_once('/').expect("namespaced code");
            assert_eq!(ns, NAMESPACE);
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "{name}"
            );
        }
    }

    #[test]
    fn the_session_counter_keeps_its_count_and_refuses_a_path_escape() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = counter_path(dir.path(), "../../s-1").expect("a counter path");
        assert_eq!(path, dir.path().join("discipline").join("turn-s-1"));
        assert_eq!(counter_path(dir.path(), "../.."), None);
        assert_eq!(refused_so_far(&path), 0);
        record_refusal(&path, 0).expect("the counter is written");
        record_refusal(&path, 1).expect("the counter is written");
        assert_eq!(refused_so_far(&path), 2);
    }
}
