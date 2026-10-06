//! The text a grammar is given to parse.
//!
//! A grammar's lexer reads the character 0 as the end of the file wherever it stands: a
//! comment, a string or a here-document holding a NUL byte ends at it, and what follows is
//! read as an error region that can take the next declaration with it. Scanners written by
//! hand make the same reading, and one of them indexes a table by it (the Swift pack's
//! `swift_tree` has the detail). So no parser is given a NUL byte: [`parse`] is the only
//! call of `Parser::parse` on source text, and it parses a copy in which each NUL byte is
//! [`NUL_PLACEHOLDER`]. The copy has the source's length and every byte offset and line in
//! it is the source's, so a node's text is read from the source by the node's byte range.
//!
//! The shell parser of the pre-tool hook (`src/pretool.rs`) parses one command and builds
//! its own copy; it is the one other caller of `Parser::parse`.

use std::borrow::Cow;

use tree_sitter::{Parser, Tree};

/// What stands for a NUL byte in the copy a grammar parses: one byte, a control character
/// that no grammar here reads as whitespace, as part of a name or as the end of a token.
/// Inside a comment or a string it is one more character of it; anywhere else it is an
/// error the grammar reports, as a NUL byte is to each language's own compiler.
pub(crate) const NUL_PLACEHOLDER: char = '\u{1}';

/// `src` as a grammar is given it: without a NUL byte, and byte for byte as long.
pub(crate) fn parse_text(src: &str) -> Cow<'_, str> {
    if src.as_bytes().contains(&0) {
        let mut placeholder = [0u8; 4];
        Cow::Owned(src.replace('\0', NUL_PLACEHOLDER.encode_utf8(&mut placeholder)))
    } else {
        Cow::Borrowed(src)
    }
}

/// The syntax tree of `src`, parsed from [`parse_text`]. Byte ranges and positions in the
/// tree are those of `src`.
pub(crate) fn parse(parser: &mut Parser, src: &str) -> Option<Tree> {
    let text = parse_text(src);
    debug_assert!(text.len() == src.len() && !text.as_bytes().contains(&0));
    parser.parse(text.as_ref(), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn the_parse_text_has_no_nul_byte_and_moves_no_offset() {
        let src = "a\0b\n\0\0é\0";
        let text = parse_text(src);
        assert_eq!(text, "a\u{1}b\n\u{1}\u{1}é\u{1}");
        assert_eq!(text.len(), src.len());
        // A source without one is parsed as it is, not copied.
        assert!(matches!(parse_text("a\nb"), Cow::Borrowed("a\nb")));
        assert!(matches!(parse_text(""), Cow::Borrowed("")));
    }

    /// `parse` cannot be handed a NUL byte, but a second call of `Parser::parse`
    /// somewhere else could be: the call is written once in `src/`, here, and once in the
    /// pre-tool hook's shell parser.
    #[test]
    fn source_text_is_parsed_in_one_place() {
        let call = [".parse", "("].concat();
        let untouched = [", ", "None)"].concat();
        let mut sites = Vec::new();
        let mut dirs = vec![PathBuf::from(format!("{}/src", env!("CARGO_MANIFEST_DIR")))];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    for (n, line) in text.lines().enumerate() {
                        // `Parser::parse(text, old_tree)` is the only `parse` here whose
                        // last argument is `None`.
                        if line.contains(&call) && line.contains(&untouched) {
                            let rel = path.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap();
                            sites.push(format!("{}:{}", rel.display(), n + 1));
                        }
                    }
                }
            }
        }
        sites.sort();
        let files: Vec<&str> = sites.iter().map(|s| s.split(':').next().unwrap()).collect();
        assert_eq!(
            files,
            ["src/ast/source_text.rs", "src/pretool.rs"],
            "{sites:?}"
        );
    }
}
