//! Text from outside the binary, made safe for the format it is written into.
//!
//! A report quotes text its reader did not write and the binary did not choose: what a
//! forge answered (a refusal message, a login, a title), a commit message, a file name,
//! a line of a changed file, the output of a tool a gate ran. Whoever wrote that text
//! must not be able to write the report: add a line that reads as the report's own, hide
//! or restyle what the report says, notify someone, or drive the terminal.
//!
//! The rule is one function per output format, applied where the text is written:
//!
//! | Format | Function | What it does |
//! |---|---|---|
//! | terminal, one line | [`terminal_line`] | every control character becomes U+FFFD |
//! | terminal, a block | [`terminal_block`] | the same per line; a continuation line is indented |
//! | terminal, a whole text | [`terminal_text`] | the same, line breaks kept as they are |
//! | Markdown, running text | [`markdown`] | see below |
//! | Markdown, a table cell | [`markdown_cell`] | the same, and `\|` for a pipe |
//! | Markdown, a code span | [`code_span`], [`code_span_cell`] | a span the text cannot close |
//!
//! The machine formats (JSON, SARIF, GitLab code quality) carry the text as it is,
//! encoded by their serialiser; JUnit XML drops what XML 1.0 forbids (`junit.rs`), and
//! the audit page escapes for HTML (`audit_html::esc`).
//!
//! A message with none of the characters named here is written byte for byte.

/// What a control character is replaced with: the reader sees that something was there.
pub const REPLACEMENT: char = '\u{FFFD}';

/// Whether `c` can change how a terminal or a renderer lays text out without being
/// visible itself: the C0 and C1 controls and DEL (ESC, CSI, the bell, backspace,
/// carriage return), the bidirectional embedding, override and isolate controls (text
/// shown in another order than it is stored), and the Unicode line and paragraph
/// separators (a line break that is not `\n`). A tab is one too; callers keep it.
fn is_control(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{061C}' | '\u{2028}' | '\u{2029}'
        )
}

/// `text` for one line of a terminal report: it stays one line, and carries no escape
/// sequence. Every control character but a tab becomes [`REPLACEMENT`], a line break
/// included.
pub fn terminal_line(text: &str) -> String {
    if !text.chars().any(|c| c != '\t' && is_control(c)) {
        return text.to_string();
    }
    text.chars()
        .map(|c| {
            if c != '\t' && is_control(c) {
                REPLACEMENT
            } else {
                c
            }
        })
        .collect()
}

/// `text` for a block of a terminal report that may run over several lines (a finding's
/// message can quote a tool's output): each line as [`terminal_line`] gives it, and every
/// line after the first starts with `indent`, so none starts where the report's own lines
/// do. An empty line stays empty.
pub fn terminal_block(text: &str, indent: &str) -> String {
    if !text.chars().any(|c| c != '\t' && is_control(c)) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 8);
    for (i, line) in text.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if i > 0 {
            out.push('\n');
            if !line.is_empty() {
                out.push_str(indent);
            }
        }
        out.push_str(&terminal_line(line));
    }
    out
}

/// `text` whose line breaks are its own (an error with a quoted excerpt under it, a
/// fenced block): `\n` and tabs are kept, every other control character becomes
/// [`REPLACEMENT`].
pub fn terminal_text(text: &str) -> String {
    if !text
        .chars()
        .any(|c| c != '\t' && c != '\n' && is_control(c))
    {
        return text.to_string();
    }
    text.chars()
        .map(|c| {
            if c != '\t' && c != '\n' && is_control(c) {
                REPLACEMENT
            } else {
                c
            }
        })
        .collect()
}

/// Line breaks become a space and every other control character [`REPLACEMENT`]: what a
/// Markdown paragraph or table row needs to stay one.
fn flat(text: &str) -> Vec<char> {
    text.chars()
        .map(|c| match c {
            '\n' | '\r' => ' ',
            '\t' => '\t',
            c if is_control(c) => REPLACEMENT,
            c => c,
        })
        .collect()
}

/// Characters [`inline`] rewrites outside a code span.
fn is_rewritten(c: char, cell: bool) -> bool {
    matches!(c, '&' | '<' | '>' | '@' | ']') || (cell && c == '|')
}

/// A pipe inside a code span of a table cell, after `before` (the span's text so far).
/// `\|` keeps it in the cell. After an odd run of backslashes that would read as an
/// escaped backslash and a bare pipe, which ends the cell, and a backslash in a code
/// span cannot be escaped: the pipe becomes [`REPLACEMENT`].
fn push_code_pipe(out: &mut String, before: &[char]) {
    let backslashes = before.iter().rev().take_while(|c| **c == '\\').count();
    if backslashes % 2 == 1 {
        out.push(REPLACEMENT);
    } else {
        out.push_str("\\|");
    }
}

/// Markdown inline text that says what `text` says and does nothing else.
///
/// The text is read the way CommonMark reads it, since the binary's own messages put
/// names in code spans and those must keep rendering. A backtick run and the next run of
/// the same length are a code span: nothing inside one is active, so it is written as it
/// is. A run with no partner is escaped, so it cannot pair with a backtick written after
/// this text. Outside a span: `&`, `<`, `>` become entities (no tag, no comment, no
/// autolink); `@` becomes `&#64;` (no mention); `]` before `(` or `[` is escaped (no
/// link, no image); a backslash before any of these is itself escaped, so it cannot
/// undo the escape. In a table cell a pipe is escaped as well, inside a span or out.
fn inline(text: &str, cell: bool) -> String {
    let chars = flat(text);
    let mut out = String::with_capacity(text.len() + 16);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '`' => {
                let run = chars[i..].iter().take_while(|c| **c == '`').count();
                match closing_run(&chars, i + run, run) {
                    Some(close) => {
                        out.extend(std::iter::repeat_n('`', run));
                        for (k, c) in chars[i + run..close].iter().enumerate() {
                            if cell && *c == '|' {
                                push_code_pipe(&mut out, &chars[i + run..i + run + k]);
                            } else {
                                out.push(*c);
                            }
                        }
                        out.extend(std::iter::repeat_n('`', run));
                        i = close + run;
                    }
                    None => {
                        for _ in 0..run {
                            out.push_str("\\`");
                        }
                        i += run;
                    }
                }
            }
            '\\' => match chars.get(i + 1) {
                Some('`') => {
                    out.push_str("\\`");
                    i += 2;
                }
                Some('\\') => {
                    out.push_str("\\\\");
                    i += 2;
                }
                Some(n) if is_rewritten(*n, cell) => {
                    out.push_str("\\\\");
                    i += 1;
                }
                _ => {
                    out.push('\\');
                    i += 1;
                }
            },
            '&' => {
                out.push_str("&amp;");
                i += 1;
            }
            '<' => {
                out.push_str("&lt;");
                i += 1;
            }
            '>' => {
                out.push_str("&gt;");
                i += 1;
            }
            '@' => {
                out.push_str("&#64;");
                i += 1;
            }
            ']' if matches!(chars.get(i + 1), Some('(') | Some('[')) => {
                out.push_str("\\]");
                i += 1;
            }
            '|' if cell => {
                out.push_str("\\|");
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Where the backtick run of exactly `len` that closes a code span opened before `from`
/// starts, if there is one.
fn closing_run(chars: &[char], from: usize, len: usize) -> Option<usize> {
    let mut i = from;
    while i < chars.len() {
        if chars[i] == '`' {
            let run = chars[i..].iter().take_while(|c| **c == '`').count();
            if run == len {
                return Some(i);
            }
            i += run;
        } else {
            i += 1;
        }
    }
    None
}

/// `text` as Markdown running text (a paragraph of the job summary).
pub fn markdown(text: &str) -> String {
    inline(text, false)
}

/// `text` as the content of a Markdown table cell.
pub fn markdown_cell(text: &str) -> String {
    inline(text, true)
}

fn span(text: &str, cell: bool) -> String {
    let chars = flat(text);
    let longest = chars
        .split(|c| *c != '`')
        .map(<[char]>::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    // A span whose text starts or ends with a backtick needs a space to tell the text
    // from the fence (CommonMark strips one space on each side).
    let pad = if longest > 0 { " " } else { "" };
    let mut out = String::with_capacity(text.len() + 4);
    out.push_str(&fence);
    out.push_str(pad);
    for (k, c) in chars.iter().enumerate() {
        if cell && *c == '|' {
            push_code_pipe(&mut out, &chars[..k]);
        } else {
            out.push(*c);
        }
    }
    out.push_str(pad);
    out.push_str(&fence);
    out
}

/// `text` as one Markdown code span, backticks included: the fence is one backtick
/// longer than the longest run in the text, so the text cannot close it.
pub fn code_span(text: &str) -> String {
    span(text, false)
}

/// [`code_span`] inside a table cell: a pipe is escaped too.
pub fn code_span_cell(text: &str) -> String {
    span(text, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hostile text, by what it tries.
    const HOSTILE: &[(&str, &str)] = &[
        ("newline", "ok\nStatus: PASS"),
        ("carriage return", "FAILED\rStatus: PASS"),
        ("crlf", "ok\r\nStatus: PASS"),
        ("sgr", "\u{1b}[31mred\u{1b}[0m"),
        (
            "osc 8 link",
            "\u{1b}]8;;http://x.example.invalid\u{7}here\u{1b}]8;;\u{7}",
        ),
        ("bell", "ding\u{7}"),
        ("backspace", "FAILED\u{8}\u{8}\u{8}\u{8}\u{8}\u{8}PASS"),
        ("c1 csi", "\u{9b}31mred"),
        ("delete", "a\u{7f}b"),
        ("pipe", "a | b | c"),
        ("backtick", "a ` b"),
        ("mention", "thanks @someone"),
        ("script", "<script>alert(1)</script>"),
        ("image", "<img src=x onerror=alert(1)>"),
        ("link", "[click](http://x.example.invalid)"),
        ("image link", "![x](http://x.example.invalid/p.png)"),
        ("reference link", "[click][ref]"),
        ("comment", "<!-- hidden"),
        ("entity", "&lt;b&gt;"),
        ("bidi override", "safe\u{202e}txt.exe"),
        ("bidi isolate", "a\u{2066}b\u{2069}c"),
        ("line separator", "ok\u{2028}Status: PASS"),
        ("zero-width joiner", "a\u{200d}b"),
        ("escaped escape", "\\<b>"),
        ("escaped mention", "\\@someone"),
        ("unclosed span then tag", "` <img src=x>"),
        ("span closed early", "x` <img src=x> `y"),
        ("escaped backtick then tag", "\\`<img src=x>`"),
    ];

    fn long() -> String {
        "A<b>@x`|\n".repeat(4000)
    }

    /// The Markdown text outside every code span of `md`, read as CommonMark reads it:
    /// what a renderer treats as live. Independent of [`inline`]: it only finds spans.
    fn live(md: &str) -> String {
        let c: Vec<char> = md.chars().collect();
        let mut out = String::new();
        let mut i = 0;
        while i < c.len() {
            if c[i] == '\\' && i + 1 < c.len() && c[i + 1].is_ascii_punctuation() {
                // An escaped character is literal: it opens nothing.
                out.push('\u{1}');
                i += 2;
            } else if c[i] == '`' {
                let run = c[i..].iter().take_while(|x| **x == '`').count();
                match closing_run(&c, i + run, run) {
                    Some(close) => i = close + run,
                    None => {
                        out.extend(std::iter::repeat_n('`', run));
                        i += run;
                    }
                }
            } else {
                out.push(c[i]);
                i += 1;
            }
        }
        out
    }

    #[test]
    fn a_terminal_line_stays_one_line_and_carries_no_control_character() {
        let long = long();
        for (what, text) in HOSTILE.iter().copied().chain([("long", long.as_str())]) {
            let line = terminal_line(text);
            assert!(
                !line.chars().any(|c| c != '\t' && is_control(c)),
                "{what}: {line:?}"
            );
            assert_eq!(line.lines().count().max(1), 1, "{what}: {line:?}");
            assert_eq!(line.chars().count(), text.chars().count(), "{what}");
        }
        // The reader sees that something was there.
        assert_eq!(terminal_line("ok\nStatus: PASS"), "ok\u{fffd}Status: PASS");
        assert_eq!(terminal_line("\u{1b}[31mred"), "\u{fffd}[31mred");
        // A joiner is part of a word in several scripts and of an emoji: kept.
        assert_eq!(terminal_line("a\u{200d}b"), "a\u{200d}b");
        assert_eq!(terminal_line("a\tb"), "a\tb");
    }

    #[test]
    fn a_terminal_block_indents_every_line_after_the_first() {
        assert_eq!(
            terminal_block("exit 1:\nStatus: PASS\n\nlast\r\nend\u{1b}[0m", "   "),
            "exit 1:\n   Status: PASS\n\n   last\n   end\u{fffd}[0m"
        );
        for (what, text) in HOSTILE {
            let block = terminal_block(text, "   ");
            for line in block.lines().skip(1) {
                assert!(
                    line.is_empty() || line.starts_with("   "),
                    "{what}: {block:?}"
                );
            }
            assert!(
                !block
                    .chars()
                    .any(|c| c != '\t' && c != '\n' && is_control(c)),
                "{what}: {block:?}"
            );
        }
    }

    #[test]
    fn terminal_text_keeps_its_line_breaks_and_nothing_else_that_controls() {
        assert_eq!(
            terminal_text("a\n\tb\r\u{1b}[2Jc\u{7}"),
            "a\n\tb\u{fffd}\u{fffd}[2Jc\u{fffd}"
        );
    }

    /// Text the binary writes itself, which must come through byte for byte.
    const PLAIN: &[&str] = &[
        "",
        "1 directive(s) read from merged pull request #12 (author agent)",
        "continuing without it (`directives.degrade_offline`)",
        "`gates.x.old` is deprecated; use `gates.x.new`",
        "Raise ``a`b`` to *two* _three_ ~four~ #5 [six] (seven) {eight} 9% + 10 = 10!",
        "src/a b/c.rs:12",
        "caf\u{e9} \u{65e5}\u{672c}\u{8a9e} \u{1f600}",
        "a\tb",
        "C:\\dir\\file and a regex \\d+\\.\\d+",
        "see https://orieg.github.io/discipline/gates/#pii",
    ];

    #[test]
    fn text_with_nothing_to_neutralise_is_written_byte_for_byte() {
        for text in PLAIN {
            assert_eq!(terminal_line(text), *text);
            assert_eq!(terminal_block(text, "   "), *text);
            assert_eq!(terminal_text(text), *text);
            assert_eq!(markdown(text), *text);
            assert_eq!(markdown_cell(text), *text);
            if !text.contains('`') {
                assert_eq!(code_span(text), format!("`{text}`"));
                assert_eq!(code_span_cell(text), format!("`{text}`"));
            }
        }
    }

    #[test]
    fn markdown_leaves_nothing_live_but_plain_text() {
        let long = long();
        for (what, text) in HOSTILE.iter().copied().chain([("long", long.as_str())]) {
            for (cell, md) in [(false, markdown(text)), (true, markdown_cell(text))] {
                assert!(!md.contains(['\n', '\r']), "{what}: {md:?}");
                assert!(
                    !md.chars().any(|c| c != '\t' && is_control(c)),
                    "{what}: {md:?}"
                );
                let live = live(&md);
                assert!(!live.contains('<'), "{what}: a tag in {md:?}");
                assert!(!live.contains('>'), "{what}: a tag end in {md:?}");
                assert!(!live.contains('@'), "{what}: a mention in {md:?}");
                assert!(!live.contains('`'), "{what}: an open span in {md:?}");
                assert!(!live.contains("]("), "{what}: a link in {md:?}");
                assert!(!live.contains("]["), "{what}: a reference in {md:?}");
                // An ampersand left live starts one of the entities written here.
                for (at, _) in live.match_indices('&') {
                    assert!(
                        ["&amp;", "&lt;", "&gt;", "&#64;"]
                            .iter()
                            .any(|e| live[at..].starts_with(e)),
                        "{what}: a bare ampersand in {md:?}"
                    );
                }
                if cell {
                    assert!(!unescaped_pipe(&md), "{what}: a cell break in {md:?}");
                }
            }
        }
    }

    /// Whether `md` has a pipe a table reads as the end of a cell: one not preceded by
    /// an odd run of backslashes.
    fn unescaped_pipe(md: &str) -> bool {
        let c: Vec<char> = md.chars().collect();
        (0..c.len()).any(|i| {
            c[i] == '|' && c[..i].iter().rev().take_while(|x| **x == '\\').count() % 2 == 0
        })
    }

    #[test]
    fn markdown_writes_each_construct_as_text() {
        let cases: &[(&str, &str)] = &[
            ("thanks @someone", "thanks &#64;someone"),
            (
                "<script>alert(1)</script>",
                "&lt;script&gt;alert(1)&lt;/script&gt;",
            ),
            ("<!-- hidden", "&lt;!-- hidden"),
            (
                "[click](http://x.example.invalid)",
                "[click\\](http://x.example.invalid)",
            ),
            ("[click][ref]", "[click\\][ref]"),
            ("&lt;b&gt;", "&amp;lt;b&amp;gt;"),
            ("a\nb\rc", "a b c"),
            ("\u{1b}[31mred", "\u{fffd}[31mred"),
            // A code span is kept, and what is in it is not rewritten.
            ("in `Vec<u8>` and @x", "in `Vec<u8>` and &#64;x"),
            ("``a`<b>`` <c>", "``a`<b>`` &lt;c&gt;"),
            // A backtick with no partner is escaped: it cannot pair with a later one.
            ("` <img src=x>", "\\` &lt;img src=x&gt;"),
            ("a `` b ` c", "a \\`\\` b \\` c"),
            // Two backticks are a span whoever wrote them: the tag is inside it, inert.
            ("x` <img src=x> `y", "x` <img src=x> `y"),
            // The text closes the span the message opened; what follows is rewritten.
            ("`x` <img> `y", "`x` &lt;img&gt; \\`y"),
            // An escaped backtick opens nothing.
            ("\\`<img src=x>`", "\\`&lt;img src=x&gt;\\`"),
            // A backslash cannot undo an escape.
            ("\\<b>", "\\\\&lt;b&gt;"),
            ("\\@someone", "\\\\&#64;someone"),
            ("\\\\<b>", "\\\\&lt;b&gt;"),
        ];
        for (text, want) in cases {
            assert_eq!(markdown(text), *want, "{text:?}");
        }
        // A name the message put in a span, hostile text, another span: the tag is
        // between the two spans, and is rewritten.
        assert_eq!(markdown("`a` <img src=x> `b`"), "`a` &lt;img src=x&gt; `b`");
    }

    #[test]
    fn a_table_cell_keeps_its_pipes_in_the_cell() {
        let cases: &[(&str, &str)] = &[
            ("a | b", "a \\| b"),
            ("`a|b`", "`a\\|b`"),
            ("a \\| b", "a \\\\\\| b"),
            ("a \\\\| b", "a \\\\\\| b"),
            // In a span a backslash cannot be escaped, so the pipe after one is replaced.
            ("`a\\|<img src=x>`", "`a\\\u{fffd}<img src=x>`"),
            ("`a\\\\|b`", "`a\\\\\\|b`"),
        ];
        for (text, want) in cases {
            assert_eq!(markdown_cell(text), *want, "{text:?}");
            assert!(!unescaped_pipe(want), "{want:?}");
        }
        assert_eq!(markdown("a | b"), "a | b");
    }

    #[test]
    fn a_code_span_cannot_be_closed_by_its_text() {
        assert_eq!(code_span("src/a.rs:3"), "`src/a.rs:3`");
        assert_eq!(code_span("<pr-body>:42"), "`<pr-body>:42`");
        assert_eq!(code_span("a`b"), "`` a`b ``");
        assert_eq!(code_span("a``b` <img>"), "``` a``b` <img> ```");
        assert_eq!(code_span("a\nb\u{1b}"), "`a b\u{fffd}`");
        assert_eq!(code_span("a|b"), "`a|b`");
        assert_eq!(code_span_cell("a|b"), "`a\\|b`");
        assert_eq!(code_span_cell("a\\|b"), "`a\\\u{fffd}b`");
        let long = long();
        for (what, text) in HOSTILE.iter().copied().chain([("long", long.as_str())]) {
            for span in [code_span(text), code_span_cell(text)] {
                // The whole of it is one span: nothing is left live.
                assert_eq!(live(&span), "", "{what}: {span:?}");
                assert!(!span.contains(['\n', '\r', '\u{1b}']), "{what}: {span:?}");
            }
            assert!(!unescaped_pipe(&code_span_cell(text)), "{what}");
        }
    }
}
