//! Lexical limits of the hand-written grammar scanners, checked before a source reaches
//! the parser.
//!
//! A few tree-sitter scanners keep part of their state in a fixed buffer or a one-byte
//! field and trust it back when the parser restores the state. A construct past that size
//! makes the scanner store a length that does not fit: tree-sitter-ruby 0.23.1 writes a
//! here-document word's length in one byte (`src/scanner.c`, `serialize`), so a word of 256
//! bytes round-trips as length 0 and the scanner's own `assert(size == length)` aborts the
//! process. A build without assertions goes on to read an empty word and mis-reads the rest
//! of the file (the here-document never ends and swallows the following declarations), and a
//! word of about 1019 bytes writes past the 1024-byte serialization buffer. The Python
//! grammar (tree-sitter-python 0.25.0) writes its indentation stack two bytes per level into
//! the same 1024-byte buffer and tests the space once per level but writes twice, so a deep
//! enough stack writes one byte past the buffer (`[`python_indent_nesting`]`). One line of a
//! change must not be able to stop every gate or hide code from them, and the parser is what
//! aborts (its own assertion, or the library's `length <= 1024`), so the check cannot use it:
//! the pre-scans here find the construct lexically.
//!
//! The caller refuses the file the same way an over-budget parse is refused
//! (`crate::ast::source_text`): the existing ``could not parse `<path>`: ...`` error, so a
//! gate that needs the file's facts stops (exit 2) and a gate that reads what it can notes
//! it. The pre-scans are conservative: they over-count the construct (the Ruby scan looks
//! only where a here-document word can begin and refuses a long run whether or not the
//! grammar would read it as one; the Python count can only exceed the scanner's own stack
//! depth, never undercut it), and refuse when unsure. They never accept a construct they
//! cannot bound.

/// The length a tree-sitter-ruby 0.23.1 here-document word may have and round-trip through
/// the scanner's serialized state: the length is written in one byte, so 255 is the largest
/// it holds. A word of 256 bytes is restored as length 0 (`assert(size == length)` aborts a
/// build with assertions); a word of about 1019 bytes writes past the serialization buffer.
#[cfg(feature = "lang-ruby")]
pub(crate) const RUBY_HEREDOC_WORD_MAX: usize = 255;

/// `Err` naming the limit when `src` holds a here-document introducer whose word is longer
/// than [`RUBY_HEREDOC_WORD_MAX`]; `Ok` otherwise.
#[cfg(feature = "lang-ruby")]
///
/// A here-document introducer is `<<`, `<<-` or `<<~` followed with no space by the word:
/// a quoted word (`<<"END"`, `<<'END'`, ``<<`END` ``, up to the matching quote on the line)
/// or a bare word (a run of `[A-Za-z0-9_]`). `<<-` and `<<~` are here-document introducers
/// only — Ruby has no such operators — so a long word after them is always refused. A plain
/// `<<` is also the left-shift operator, so a long run after it is almost always a
/// here-document in practice and is refused as one; refusing is fail-closed (the file is
/// treated as unparsable, never passed), which is the safe direction when the shape is
/// ambiguous.
pub(crate) fn ruby_heredoc_word(src: &str) -> Result<(), String> {
    let bytes = src.as_bytes();
    let mut i = 0;
    while let Some(off) = find_shift(bytes, i) {
        // Position just past the `<<`.
        let mut j = off + 2;
        // An optional single `-` or `~`.
        if matches!(bytes.get(j), Some(b'-' | b'~')) {
            j += 1;
        }
        let word_len = match bytes.get(j) {
            Some(&q @ (b'"' | b'\'' | b'`')) => quoted_word_len(bytes, j + 1, q),
            Some(c) if is_word_byte(*c) => bare_word_len(bytes, j),
            _ => None,
        };
        if let Some(len) = word_len {
            if len > RUBY_HEREDOC_WORD_MAX {
                return Err(format!(
                    "a here-document word of {len} bytes is longer than the {RUBY_HEREDOC_WORD_MAX} \
                     the Ruby grammar's scanner can store, so it would abort or mis-read the file"
                ));
            }
        }
        i = off + 2;
    }
    Ok(())
}

/// The next `<<` at or after `from`, or `None`.
#[cfg(feature = "lang-ruby")]
fn find_shift(bytes: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 1 < bytes.len() {
        if bytes[i] == b'<' && bytes[i + 1] == b'<' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Whether `b` is a byte of a bare here-document word.
#[cfg(feature = "lang-ruby")]
fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The length of the bare word that starts at `start`, counting `[A-Za-z0-9_]` bytes.
#[cfg(feature = "lang-ruby")]
fn bare_word_len(bytes: &[u8], start: usize) -> Option<usize> {
    let mut end = start;
    while bytes.get(end).is_some_and(|b| is_word_byte(*b)) {
        end += 1;
    }
    Some(end - start)
}

/// The length of a quoted word: the bytes from `start` up to the closing `quote`, stopping
/// at a line break (a quoted here-document word is on one line). `None` when no closing
/// quote is found before the line ends — nothing is refused on an unterminated quote, which
/// the grammar does not read as a here-document word either.
#[cfg(feature = "lang-ruby")]
fn quoted_word_len(bytes: &[u8], start: usize, quote: u8) -> Option<usize> {
    let mut end = start;
    while let Some(&b) = bytes.get(end) {
        if b == quote {
            return Some(end - start);
        }
        if b == b'\n' || b == b'\r' {
            return None;
        }
        end += 1;
    }
    None
}

/// The most distinct leading-whitespace prefixes a Python source may have before its
/// indentation stack, serialized two bytes per level into the scanner's 1024-byte buffer,
/// could reach the overflow.
///
/// Arithmetic (`tree-sitter-python` 0.25.0 `src/scanner.c`
/// `tree_sitter_python_external_scanner_serialize`): the buffer is
/// `TREE_SITTER_SERIALIZATION_BUFFER_SIZE` = 1024 bytes. The scanner writes one byte for
/// `inside_interpolated_string`, one byte for the delimiter count, up to 255 delimiter bytes
/// (the count is clamped to `UINT8_MAX`), then two bytes for each indentation level above the
/// first. The indent loop tests `size < 1024` once but writes two bytes, so it overruns when
/// `size` reaches 1023; with the full 255-byte delimiter payload that is a stack of 384
/// levels (`2 + 255 + 2 * 383 = 1023`). The limit here is set far below that: at a stack of
/// `L` levels the worst serialized size is `2 + 255 + 2 * (L - 1)` bytes, which for `L = 200`
/// is 655, a third of the buffer. Measured (RUN, sanitizer harness): a stack of 200 with 255
/// open f-string delimiters serializes 655 bytes and does not fault; 384 faults.
#[cfg(feature = "lang-python")]
pub(crate) const PYTHON_INDENT_NESTING_MAX: usize = 200;

/// `Err` naming the count and the limit when `src` has more than [`PYTHON_INDENT_NESTING_MAX`]
/// distinct leading-whitespace prefixes; `Ok` otherwise.
///
/// The scanner's indentation stack holds strictly increasing indentation widths, each the
/// leading whitespace of some line, so its depth can never exceed the number of distinct
/// leading-whitespace prefixes among the file's lines. This counts those prefixes — the
/// maximal leading run of space, tab, form feed, carriage return or vertical tab on each line,
/// as raw bytes. It can only over-count the scanner's depth, never undercut it: distinct raw
/// prefixes map many-to-one onto the column widths the scanner stacks (a tab and eight spaces
/// are one width), and a line inside a string or brackets, which the scanner does not stack,
/// still only adds a prefix. So a source within the limit is certainly within the scanner's
/// capacity. It uses no parser, which is what aborts.
#[cfg(feature = "lang-python")]
pub(crate) fn python_indent_nesting(src: &str) -> Result<(), String> {
    use std::collections::HashSet;
    let mut prefixes: HashSet<&[u8]> = HashSet::new();
    for line in src.as_bytes().split(|&b| b == b'\n') {
        let end = line
            .iter()
            .position(|&b| !matches!(b, b' ' | b'\t' | b'\x0c' | b'\r' | b'\x0b'))
            .unwrap_or(line.len());
        prefixes.insert(&line[..end]);
        if prefixes.len() > PYTHON_INDENT_NESTING_MAX {
            return Err(format!(
                "the file has more than {PYTHON_INDENT_NESTING_MAX} distinct indentation \
                 prefixes, past what the Python grammar's scanner can store, so it would \
                 overflow its serialization buffer"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "lang-ruby")]
    #[test]
    fn a_short_here_document_word_is_allowed() {
        assert!(ruby_heredoc_word("x = <<END\nbody\nEND\n").is_ok());
        assert!(ruby_heredoc_word("x = <<~SQL\n  select 1\nSQL\n").is_ok());
        assert!(ruby_heredoc_word("x = <<-'done'\nbody\ndone\n").is_ok());
        // The left-shift operator with short operands.
        assert!(ruby_heredoc_word("list << item\nx = a<<b\n").is_ok());
        // A word of exactly the maximum round-trips.
        let at_max = format!("x = <<{}\n", "A".repeat(RUBY_HEREDOC_WORD_MAX));
        assert!(ruby_heredoc_word(&at_max).is_ok());
    }

    #[cfg(feature = "lang-ruby")]
    #[test]
    fn a_bare_word_past_the_limit_is_refused() {
        let over = format!("x = <<{}\n", "A".repeat(RUBY_HEREDOC_WORD_MAX + 1));
        let err = ruby_heredoc_word(&over).unwrap_err();
        assert!(err.contains("256 bytes"), "{err}");
        assert!(err.contains("255"), "{err}");
    }

    #[cfg(feature = "lang-ruby")]
    #[test]
    fn a_squiggly_or_dash_word_past_the_limit_is_refused() {
        let w = "B".repeat(RUBY_HEREDOC_WORD_MAX + 10);
        assert!(ruby_heredoc_word(&format!("x = <<~{w}\n")).is_err());
        assert!(ruby_heredoc_word(&format!("x = <<-{w}\n")).is_err());
    }

    #[cfg(feature = "lang-ruby")]
    #[test]
    fn a_quoted_word_past_the_limit_is_refused() {
        let w = "c".repeat(RUBY_HEREDOC_WORD_MAX + 1);
        assert!(ruby_heredoc_word(&format!("x = <<\"{w}\"\n")).is_err());
        assert!(ruby_heredoc_word(&format!("x = <<'{w}'\n")).is_err());
        assert!(ruby_heredoc_word(&format!("x = <<`{w}`\n")).is_err());
    }

    #[cfg(feature = "lang-ruby")]
    #[test]
    fn a_long_run_that_is_not_a_here_document_word_is_not_refused() {
        // A space after `<<` means the word does not begin there.
        let w = "A".repeat(RUBY_HEREDOC_WORD_MAX + 1);
        assert!(ruby_heredoc_word(&format!("x = << {w}\n")).is_ok());
        // An unterminated quote on the line is read as no here-document word.
        assert!(ruby_heredoc_word(&format!("x = <<\"{w}\n")).is_ok());
        // A long identifier not after `<<`.
        assert!(ruby_heredoc_word(&format!("{w} = 1\n")).is_ok());
    }

    /// Enforced by construction: the Ruby pack checks the here-document word limit before
    /// it hands the source to the parser, so no Ruby file reaches the scanner unchecked.
    /// The same shape as `source_text::source_text_is_parsed_in_one_place`.
    #[cfg(feature = "lang-ruby")]
    #[test]
    fn the_ruby_pack_checks_the_limit_before_it_parses() {
        let ruby =
            std::fs::read_to_string(format!("{}/src/ast/ruby.rs", env!("CARGO_MANIFEST_DIR")))
                .unwrap();
        let guard = "scanner_limits::ruby_heredoc_word(src)";
        let parse = "source_text::parse_file(&mut parser, path, src)";
        let guard_at = ruby
            .find(guard)
            .unwrap_or_else(|| panic!("the Ruby pack does not call `{guard}`"));
        let parse_at = ruby
            .find(parse)
            .unwrap_or_else(|| panic!("the Ruby pack does not call `{parse}`"));
        assert!(
            guard_at < parse_at,
            "the limit check must run before the parse"
        );
    }

    #[cfg(feature = "lang-ruby")]
    #[test]
    fn the_word_just_past_the_limit_is_the_one_that_aborts_the_scanner() {
        // The scanner stores the length in one byte, so 256 round-trips as 0.
        assert_eq!(RUBY_HEREDOC_WORD_MAX, 255);
        assert!(ruby_heredoc_word(&format!("x = <<{}\n", "A".repeat(255))).is_ok());
        assert!(ruby_heredoc_word(&format!("x = <<{}\n", "A".repeat(256))).is_err());
    }

    /// A Python source with `levels` distinct indentation prefixes: `levels - 1` nested
    /// blocks plus the top level, the deepest line carrying `open_fstrings` open f-strings.
    #[cfg(feature = "lang-python")]
    fn python_nested(levels: usize, open_fstrings: usize) -> String {
        let mut s = String::new();
        for i in 0..levels.saturating_sub(1) {
            s.push_str(&" ".repeat(i));
            s.push_str("if x:\n");
        }
        s.push_str(&" ".repeat(levels.saturating_sub(1)));
        s.push_str("y = ");
        for i in 0..open_fstrings {
            s.push('f');
            s.push(if i % 2 == 0 { '"' } else { '\'' });
            s.push('{');
        }
        s.push('1');
        s.push('\n');
        s
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn python_indentation_within_the_limit_is_allowed() {
        assert!(python_indent_nesting("def f():\n    return 1\n").is_ok());
        // Flat files with many short prefixes stay well under the limit.
        assert!(python_indent_nesting(&"x = 1\n".repeat(1000)).is_ok());
        // Exactly the limit of distinct prefixes round-trips (measured: 655 bytes).
        assert!(python_indent_nesting(&python_nested(PYTHON_INDENT_NESTING_MAX, 255)).is_ok());
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn python_indentation_past_the_limit_is_refused() {
        let over = python_nested(PYTHON_INDENT_NESTING_MAX + 1, 255);
        let err = python_indent_nesting(&over).unwrap_err();
        assert!(
            err.contains("more than 200 distinct indentation prefixes"),
            "{err}"
        );
        // The control at the limit is accepted; one more prefix is refused.
        assert!(python_indent_nesting(&python_nested(PYTHON_INDENT_NESTING_MAX, 0)).is_ok());
        assert!(python_indent_nesting(&python_nested(PYTHON_INDENT_NESTING_MAX + 1, 0)).is_err());
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn distinct_prefixes_are_counted_by_raw_bytes() {
        // A tab and eight spaces are distinct prefixes here (the count over-estimates the
        // scanner's column-width stack, which is the safe direction).
        let mut s = String::new();
        for i in 0..=PYTHON_INDENT_NESTING_MAX {
            s.push_str(&"\t".repeat(i));
            s.push_str("pass\n");
        }
        assert!(python_indent_nesting(&s).is_err());
        // Blank lines and the empty prefix do not multiply the count.
        assert!(python_indent_nesting("x = 1\n\n\n    y = 2\n").is_ok());
    }

    /// Enforced by construction: the Python pack checks the indentation limit before it
    /// hands the source to the parser.
    #[cfg(feature = "lang-python")]
    #[test]
    fn the_python_pack_checks_the_limit_before_it_parses() {
        let python =
            std::fs::read_to_string(format!("{}/src/ast/python.rs", env!("CARGO_MANIFEST_DIR")))
                .unwrap();
        let guard = "scanner_limits::python_indent_nesting(src)";
        let parse = "source_text::parse_file(&mut parser, path, src)";
        let guard_at = python
            .find(guard)
            .unwrap_or_else(|| panic!("the Python pack does not call `{guard}`"));
        let parse_at = python
            .find(parse)
            .unwrap_or_else(|| panic!("the Python pack does not call `{parse}`"));
        assert!(
            guard_at < parse_at,
            "the limit check must run before the parse"
        );
    }
}
