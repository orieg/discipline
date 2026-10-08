//! Escaping of text for each output format that is written by hand.
//!
//! | Output | Function | What it does |
//! |---|---|---|
//! | HTML content and attribute values, the tool's own words (the audit page) | [`html`] | `&`, `<`, `>`, `"` and `'` become entities |
//! | HTML content and attribute values, text from outside the binary (the audit page) | [`html_author_text`] | [`html`], and a control, bidirectional or invisible character becomes U+FFFD |
//! | HTML content (the generated reference tables) | [`html_content`] | `&`, `<`, `>` and `"` become entities |
//! | XML 1.0 content and attribute values (JUnit) | [`xml`] | the five entities; forbidden characters are dropped |
//! | A Markdown table cell of generated documentation | [`docs_markdown_cell`] | a pipe, `<`, `>` and a line break; Markdown stays live |
//! | A Markdown table cell of the report | [`markdown_cell`] | see [`crate::report::text`] |
//! | A Markdown table cell of the pull-request comment | [`comment_markdown_cell`] | [`markdown_cell`], with no code span and no directive |
//!
//! The functions differ on purpose: each keeps the behaviour its output had before
//! they were gathered here, and the test below pins every one on the same inputs.

/// `text` as the content of a Markdown table cell of the report. It lives with the
/// rest of the Markdown rules in [`crate::report::text`].
pub use crate::report::text::markdown_cell;

/// Escape text for HTML content and attribute values, in either quote style: `'`
/// becomes `&#39;`.
pub fn html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Whether `c` is shown as U+FFFD in text the audit page quotes: what a report replaces
/// in a terminal line or a Markdown cell ([`crate::report::text`]: the C0 and C1
/// controls, DEL, the bidirectional embeddings, overrides and isolates, U+061C, the
/// line and paragraph separators), and what `instruction-smuggling` reports as
/// invisible (the zero-width characters, U+200E and U+200F among them, the soft hyphen,
/// the tag characters). A tab, a line feed and a carriage return are white space in
/// HTML and stay.
fn is_hidden(c: char) -> bool {
    !matches!(c, '\t' | '\n' | '\r')
        && (crate::report::text::is_control(c)
            || crate::guards::instruction_smuggling::invisible_class(c).is_some())
}

/// Escape text from outside the binary (a commit subject, a file name, a configuration
/// value, what a forge answered) for HTML content and attribute values. Differs from
/// [`html`]: every character [`is_hidden`] names becomes U+FFFD, so the text cannot
/// reorder or hide what the page shows around it. Text with none of them is written as
/// [`html`] writes it.
pub fn html_author_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c if is_hidden(c) => out.push(crate::report::text::REPLACEMENT),
            _ => out.push(c),
        }
    }
    out
}

/// Escape text for HTML content. Differs from [`html`]: `'` is left as it is, so the
/// result is not safe inside a single-quoted attribute value.
pub(crate) fn html_content(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Escape text for XML 1.0 content and attribute values. Differs from [`html`]: `'`
/// becomes `&apos;`, and the characters XML 1.0 forbids are dropped.
pub(crate) fn xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => {
                // Strip non-printable ASCII control characters forbidden in XML 1.0
            }
            // The two noncharacters XML 1.0 excludes from `Char` as well: a parser
            // refuses the document that carries one.
            '\u{FFFE}' | '\u{FFFF}' => {}
            _ => out.push(c),
        }
    }
    out
}

/// The five predefined XML entities read back as their characters, `&amp;` last so
/// that `&amp;lt;` reads as `&lt;`. Numeric character references are left as written.
pub(crate) fn unescape_xml(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Text of the tool's own documentation as a Markdown table cell. Differs from
/// [`markdown_cell`]: only a pipe, `<`, `>` and a line feed are changed, so `&`, a
/// backslash and Markdown markup (a code span the caller wrote) pass through.
pub(crate) fn docs_markdown_cell(s: &str) -> String {
    s.replace('|', "\\|")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\n', " ")
}

/// Text from the change, safe inside a table cell of the pull-request comment: what
/// [`markdown_cell`] neutralises (HTML, a mention, a link, emphasis, a pipe, a line
/// break, control characters; a word a renderer would link by itself becomes a code
/// span). Differs from [`markdown_cell`]: a backtick becomes an apostrophe, so the
/// cell has no code span for the text to open or close, and an override directive is
/// redacted.
pub fn comment_markdown_cell(text: &str) -> String {
    let escaped = markdown_cell(&text.replace('`', "'"));
    crate::report::scrub_override_directives(&escaped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn author_text_for_html_shows_each_hidden_character_as_the_replacement() {
        // One of each class, and the three white-space controls that stay.
        for (class, c) in [
            ("C0", '\u{1}'),
            ("C0 (ESC)", '\u{1b}'),
            ("DEL", '\u{7f}'),
            ("C1", '\u{9b}'),
            ("bidirectional embedding", '\u{202a}'),
            ("bidirectional override", '\u{202e}'),
            ("bidirectional isolate", '\u{2066}'),
            ("directional mark", '\u{200e}'),
            ("directional mark", '\u{200f}'),
            ("Arabic letter mark", '\u{61c}'),
            ("line separator", '\u{2028}'),
            ("paragraph separator", '\u{2029}'),
            ("zero-width space", '\u{200b}'),
            ("word joiner", '\u{2060}'),
            ("byte order mark", '\u{feff}'),
            ("soft hyphen", '\u{ad}'),
            ("tag character", '\u{e0041}'),
        ] {
            assert_eq!(
                html_author_text(&format!("a{c}b")),
                "a\u{fffd}b",
                "{class} U+{:04X}",
                c as u32
            );
        }
        assert_eq!(html_author_text("a\tb\nc\rd"), "a\tb\nc\rd");
        // Markup is escaped as `html` escapes it, and ordinary text is the same bytes.
        for text in [PUNCTUATION.trim_end_matches('\u{202e}'), HOSTILE, ""] {
            assert_eq!(html_author_text(text), html(text));
        }
        assert_eq!(
            html_author_text(CONTROLS),
            "a\u{fffd}b\u{fffd}c\rd\u{fffd}e\u{fffe}f\u{ffff}g"
        );
    }

    #[test]
    fn the_xml_entities_are_read_back_once() {
        assert_eq!(
            unescape_xml("&lt;a b=&quot;c&quot;&gt;&apos;&amp;"),
            "<a b=\"c\">'&"
        );
        assert_eq!(unescape_xml("&amp;lt; &#60; plain"), "&lt; &#60; plain");
    }

    /// Every ASCII punctuation character, a newline, a tab, a pipe, a backtick, a
    /// mention and some non-ASCII text.
    const PUNCTUATION: &str =
        "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~ a\nb\tc | `d` @name é ü 日本 \u{1f600} \u{202e}";
    /// Control characters and the two noncharacters XML 1.0 excludes.
    const CONTROLS: &str = "a\u{1}b\u{7f}c\rd\u{9b}e\u{fffe}f\u{ffff}g";
    /// Text that would forge the comment marker, open markup, mention a team, carry
    /// an override directive, and be linked by a renderer.
    const HOSTILE: &str =
        "<!-- discipline:report --> <b>x</b> @org/team allow-assertion-drop: y https://h.example.invalid/a|b #12";

    type Escape = fn(&str) -> String;

    #[test]
    fn each_escape_function_gives_its_recorded_output() {
        let functions: [(&str, Escape); 6] = [
            ("html", html),
            ("html_content", html_content),
            ("xml", xml),
            ("docs_markdown_cell", docs_markdown_cell),
            ("markdown_cell", markdown_cell),
            ("comment_markdown_cell", comment_markdown_cell),
        ];
        let inputs = [PUNCTUATION, CONTROLS, HOSTILE, ""];
        let mut got = Vec::new();
        for (name, f) in functions {
            for input in inputs {
                got.push((name, f(input)));
            }
        }
        let got: Vec<(&str, &str)> = got.iter().map(|(n, o)| (*n, o.as_str())).collect();
        // Recorded from the functions as they were before this module held them.
        #[rustfmt::skip]
        let want: Vec<(&str, &str)> = vec![
            ("html", "!&quot;#$%&amp;&#39;()*+,-./:;&lt;=&gt;?@[\\]^_`{|}~ a\nb\tc | `d` @name é ü 日本 \u{1f600} \u{202e}"),
            ("html", "a\u{1}b\u{7f}c\rd\u{9b}e\u{fffe}f\u{ffff}g"),
            ("html", "&lt;!-- discipline:report --&gt; &lt;b&gt;x&lt;/b&gt; @org/team allow-assertion-drop: y https://h.example.invalid/a|b #12"),
            ("html", ""),
            ("html_content", "!&quot;#$%&amp;'()*+,-./:;&lt;=&gt;?@[\\]^_`{|}~ a\nb\tc | `d` @name é ü 日本 \u{1f600} \u{202e}"),
            ("html_content", "a\u{1}b\u{7f}c\rd\u{9b}e\u{fffe}f\u{ffff}g"),
            ("html_content", "&lt;!-- discipline:report --&gt; &lt;b&gt;x&lt;/b&gt; @org/team allow-assertion-drop: y https://h.example.invalid/a|b #12"),
            ("html_content", ""),
            ("xml", "!&quot;#$%&amp;&apos;()*+,-./:;&lt;=&gt;?@[\\]^_`{|}~ a\nb\tc | `d` @name é ü 日本 \u{1f600} \u{202e}"),
            ("xml", "ab\u{7f}c\rd\u{9b}efg"),
            ("xml", "&lt;!-- discipline:report --&gt; &lt;b&gt;x&lt;/b&gt; @org/team allow-assertion-drop: y https://h.example.invalid/a|b #12"),
            ("xml", ""),
            ("docs_markdown_cell", "!\"#$%&'()*+,-./:;&lt;=&gt;?@[\\]^_`{\\|}~ a b\tc \\| `d` @name é ü 日本 \u{1f600} \u{202e}"),
            ("docs_markdown_cell", "a\u{1}b\u{7f}c\rd\u{9b}e\u{fffe}f\u{ffff}g"),
            ("docs_markdown_cell", "&lt;!-- discipline:report --&gt; &lt;b&gt;x&lt;/b&gt; @org/team allow-assertion-drop: y https://h.example.invalid/a\\|b #12"),
            ("docs_markdown_cell", ""),
            ("markdown_cell", "!\"#$%&amp;'()\\*+,-./:;&lt;=&gt;?&#64;\\[\\\\]^\\_`{\\|}~ a b\tc \\| `d\\` &#64;name é ü 日本 \u{1f600} \u{fffd}"),
            ("markdown_cell", "a\u{fffd}b\u{fffd}c d\u{fffd}e\u{fffe}f\u{ffff}g"),
            ("markdown_cell", "&lt;!-- discipline:report --&gt; &lt;b&gt;x&lt;/b&gt; &#64;org/team allow-assertion-drop: y `https://h.example.invalid/a\\|b` `#12`"),
            ("markdown_cell", ""),
            ("comment_markdown_cell", "!\"#$%&amp;'()\\*+,-./:;&lt;=&gt;?&#64;\\[\\\\]^\\_'{\\|}\\~ a b\tc \\| 'd' &#64;name é ü 日本 \u{1f600} \u{fffd}"),
            ("comment_markdown_cell", "a\u{fffd}b\u{fffd}c d\u{fffd}e\u{fffe}f\u{ffff}g"),
            ("comment_markdown_cell", "&lt;!-- discipline:report --&gt; &lt;b&gt;x&lt;/b&gt; &#64;org/team [redacted-directive]: y `https://h.example.invalid/a\\|b` `#12`"),
            ("comment_markdown_cell", ""),
        ];
        assert_eq!(got, want);
    }
}
