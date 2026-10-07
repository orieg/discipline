//! The text a grammar is given to parse, and how long it may take to parse it.
//!
//! A grammar's lexer reads the character 0 as the end of the file wherever it stands: a
//! comment, a string or a here-document holding a NUL byte ends at it, and what follows is
//! read as an error region that can take the next declaration with it. Scanners written by
//! hand make the same reading, and one of them indexes a table by it (the Swift pack's
//! `swift_tree` has the detail). So no parser is given a NUL byte: [`parse`] parses a copy
//! in which each NUL byte is [`NUL_PLACEHOLDER`]. The copy has the source's length and
//! every byte offset and line in it is the source's, so a node's text is read from the
//! source by the node's byte range.
//!
//! The parser library's error recovery does not always end: a few bytes can keep it at
//! one position for as long as it is left to run (the Rust grammar on
//! `fuzz/corpus/language_packs/timeout_rust_error_recovery`). So every parse has a
//! budget, counted in steps: the library's progress checks, which it makes once per 100
//! parse operations (`OP_COUNT_PER_PARSER_TIMEOUT_CHECK` in its `parser.c`). The count
//! depends on the text and the grammar and on nothing else, so the same text is cut, or
//! not, on every machine.
//! [`parse_within_budget`] is the only call of the parser in `src/`; a parse it cuts has
//! no tree, and its caller reports the file as one it could not parse.
//!
//! The shell parser of the pre-tool hook (`src/pretool.rs`) parses one command from its
//! own copy, through [`parse_within_budget`] as well.

use std::borrow::Cow;

use tree_sitter::{ParseOptions, ParseState, Parser, Tree};

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

/// Steps every parse may take whatever its length: 25,600 parse operations before the
/// length of the text adds to them.
pub(crate) const STEP_FLOOR: u64 = 256;

/// Steps a parse may take per byte of text. The sources of this repository take at most
/// 0.016 steps per byte with their own grammar (`tests/test_gates_e2e.rs`, the largest,
/// takes 0.010), and at most 0.05 when each is parsed with every other grammar, which is
/// error recovery from the first line to the last. Four is 250 times the first figure
/// and 80 times the second.
pub(crate) const STEPS_PER_BYTE: u64 = 4;

/// The steps a parse of `len` bytes may take.
pub(crate) fn step_budget(len: usize) -> u64 {
    STEP_FLOOR.saturating_add(STEPS_PER_BYTE.saturating_mul(len as u64))
}

/// Why a parse has no tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoTree {
    /// The parser was still working when its budget ended.
    OverBudget { steps: u64, bytes: usize },
    /// The parser library returned no tree and did not use its budget (no language set).
    NotParsed,
}

impl std::fmt::Display for NoTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoTree::OverBudget { steps, bytes } => write!(
                f,
                "the parser did not finish within its budget of {steps} steps for {bytes} bytes"
            ),
            NoTree::NotParsed => write!(f, "tree-sitter returned no tree"),
        }
    }
}

impl std::error::Error for NoTree {}

/// The syntax tree of `src`, parsed from [`parse_text`] within [`step_budget`]. Byte
/// ranges and positions in the tree are those of `src`.
pub(crate) fn parse(parser: &mut Parser, src: &str) -> Result<Tree, NoTree> {
    let text = parse_text(src);
    debug_assert!(text.len() == src.len() && !text.as_bytes().contains(&0));
    parse_within_budget(parser, text.as_bytes())
}

/// [`parse`] for a language pack reading the file at `path`: the error names the file,
/// so a gate that stops on it, or notes it, says which file it could not parse.
pub(crate) fn parse_file(parser: &mut Parser, path: &str, src: &str) -> anyhow::Result<Tree> {
    parse(parser, src).map_err(|why| anyhow::anyhow!("could not parse `{path}`: {why}"))
}

/// [`parse_file`] with a parser of `language`. `grammar` is the language as the error of
/// a grammar that cannot be loaded names it (`the Go`).
pub(crate) fn parse_file_as(
    language: &tree_sitter::Language,
    grammar: &str,
    path: &str,
    src: &str,
) -> anyhow::Result<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(language)
        .map_err(|e| anyhow::anyhow!("failed to load {grammar} grammar: {e}"))?;
    parse_file(&mut parser, path, src)
}

/// The syntax tree of `text` as it stands, within [`step_budget`]. The caller has taken
/// out what its grammar must not be given ([`parse`] does for source text).
pub(crate) fn parse_within_budget(parser: &mut Parser, text: &[u8]) -> Result<Tree, NoTree> {
    #[cfg(test)]
    let budget = BUDGET_OF_THIS_TEST.with(|b| b.get().unwrap_or(step_budget(text.len())));
    #[cfg(not(test))]
    let budget = step_budget(text.len());
    parse_counting_steps(parser, text, budget).0
}

#[cfg(test)]
thread_local! {
    static BUDGET_OF_THIS_TEST: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// `run` with every parse on this thread given `budget` steps instead of
/// [`step_budget`]: how a test of a caller reaches what the caller does with a cut
/// parse, on a text any grammar finishes.
#[cfg(test)]
pub(crate) fn with_step_budget<R>(budget: u64, run: impl FnOnce() -> R) -> R {
    BUDGET_OF_THIS_TEST.with(|b| b.set(Some(budget)));
    let result = run();
    BUDGET_OF_THIS_TEST.with(|b| b.set(None));
    result
}

/// [`parse_within_budget`] with the budget given, and the steps the parse took.
fn parse_counting_steps(
    parser: &mut Parser,
    text: &[u8],
    budget: u64,
) -> (Result<Tree, NoTree>, u64) {
    let mut steps = 0u64;
    // Returning `true` ends the parse. The library calls this once per step, in error
    // recovery as anywhere else.
    let mut over_budget = |_: &ParseState| {
        steps += 1;
        steps > budget
    };
    let tree = parser.parse_with_options(
        &mut |offset, _| text.get(offset..).unwrap_or_default(),
        None,
        Some(ParseOptions::new().progress_callback(&mut over_budget)),
    );
    match tree {
        Some(tree) => (Ok(tree), steps),
        None => {
            // A parse that was cut is kept by the parser to be resumed: drop it, so the
            // parser's next text is parsed from its start.
            parser.reset();
            let why = if steps > budget {
                NoTree::OverBudget {
                    steps: budget,
                    bytes: text.len(),
                }
            } else {
                NoTree::NotParsed
            };
            (Err(why), steps)
        }
    }
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

    /// `parse_within_budget` cannot run past its budget, and `parse` cannot be handed a
    /// NUL byte, but a second call of the parser somewhere else could: the call is
    /// written once in `src/`, here. `Parser::parse(text, old_tree)`, which has no
    /// budget, is the only `parse` whose last argument is `None`, and is written nowhere.
    #[test]
    fn source_text_is_parsed_in_one_place() {
        let bounded = [".parse_with", "_options("].concat();
        let unbounded = [[".parse", "("].concat(), [", ", "None)"].concat()];
        let other_entries = [
            [".parse_utf16", "_"].concat(),
            [".parse_custom", "_encoding("].concat(),
            [".parse_with", "("].concat(),
        ];
        let mut bounded_sites = Vec::new();
        let mut other_sites = Vec::new();
        let mut dirs = vec![PathBuf::from(format!("{}/src", env!("CARGO_MANIFEST_DIR")))];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    let rel = path.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap();
                    for (n, line) in text.lines().enumerate() {
                        let site = format!("{}:{}", rel.display(), n + 1);
                        if line.contains(&bounded) {
                            bounded_sites.push(site);
                        } else if unbounded.iter().all(|part| line.contains(part))
                            || other_entries.iter().any(|entry| line.contains(entry))
                        {
                            other_sites.push(site);
                        }
                    }
                }
            }
        }
        let files: Vec<&str> = bounded_sites
            .iter()
            .map(|s| s.split(':').next().unwrap())
            .collect();
        assert_eq!(files, ["src/ast/source_text.rs"], "{bounded_sites:?}");
        assert!(other_sites.is_empty(), "{other_sites:?}");
    }

    #[test]
    fn the_budget_is_a_floor_plus_a_share_per_byte() {
        assert_eq!(step_budget(0), STEP_FLOOR);
        assert_eq!(step_budget(15), STEP_FLOOR + 15 * STEPS_PER_BYTE);
        assert_eq!(
            step_budget(1_000_000),
            STEP_FLOOR + 1_000_000 * STEPS_PER_BYTE
        );
        assert_eq!(step_budget(usize::MAX), u64::MAX);
    }

    /// `run` on a thread of its own, or a failure when it has not returned in `LIMIT`: a
    /// parse with no budget does not return, and must fail one test, not stop the suite.
    /// The limit is not the bound under test (that one is counted in steps); it is far
    /// above what a parse cut at its budget takes.
    #[cfg(feature = "lang-rust")]
    fn returns<T: Send + 'static>(run: impl FnOnce() -> T + Send + 'static) -> T {
        const LIMIT: std::time::Duration = std::time::Duration::from_secs(240);
        let (done, result) = std::sync::mpsc::channel();
        std::thread::spawn(move || done.send(run()));
        result
            .recv_timeout(LIMIT)
            .expect("the parse did not return: it has no budget, or the budget is not applied")
    }

    #[cfg(feature = "lang-rust")]
    fn rust_parser() -> Parser {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        parser
    }

    /// The shortest text found that the Rust grammar's error recovery does not finish:
    /// an unclosed `(` before a character that starts no token, then two more unclosed
    /// calls and a `;`. The parser stays at the `;`.
    #[cfg(feature = "lang-rust")]
    const UNFINISHED: &str = "(>\u{fffd}t(0(.t();}";

    #[cfg(feature = "lang-rust")]
    #[test]
    fn a_parse_that_does_not_finish_is_cut_at_its_budget() {
        let (outcome, steps) = returns(|| {
            let mut parser = rust_parser();
            let budget = step_budget(UNFINISHED.len());
            let (tree, steps) = parse_counting_steps(&mut parser, UNFINISHED.as_bytes(), budget);
            (tree.map(|_| ()), steps)
        });
        let budget = step_budget(UNFINISHED.len());
        assert_eq!(
            outcome,
            Err(NoTree::OverBudget {
                steps: budget,
                bytes: UNFINISHED.len()
            })
        );
        // Cut at the first step past the budget, on every machine.
        assert_eq!(steps, budget + 1);
        assert_eq!(
            outcome.unwrap_err().to_string(),
            "the parser did not finish within its budget of 316 steps for 15 bytes"
        );
        // The same through `parse`, and a character that starts a name instead is read.
        assert!(returns(|| parse(&mut rust_parser(), UNFINISHED).is_err()));
        assert!(parse(&mut rust_parser(), "(>\u{e9}t(0(.t();}").is_ok());
    }

    /// The input the `language_packs` fuzz target stopped on: its first byte selects the
    /// pack, the rest is the source. The entry point every gate shares returns, with the
    /// reason.
    #[cfg(feature = "lang-rust")]
    #[test]
    fn the_fuzz_seed_that_did_not_finish_is_reported_as_unparsed() {
        let seed = std::fs::read(format!(
            "{}/fuzz/corpus/language_packs/timeout_rust_error_recovery",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let source = String::from_utf8_lossy(&seed[1..]).into_owned();
        let bytes = source.len();
        let error = returns(move || {
            crate::ast::analyze(&source, &crate::ast::AssertVocabulary::default())
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
        .unwrap_err();
        assert_eq!(
            error,
            format!(
                "could not parse `test.rs`: the parser did not finish within its budget of {} steps for {bytes} bytes",
                step_budget(bytes)
            )
        );
    }

    /// A parser whose parse was cut parses its next text from the start.
    #[cfg(feature = "lang-rust")]
    #[test]
    fn a_parser_is_usable_after_a_cut_parse() {
        let source = "fn a() -> u8 { 1 }\n".repeat(400);
        let mut parser = rust_parser();
        let (cut, steps) = parse_counting_steps(&mut parser, source.as_bytes(), 3);
        assert_eq!(
            cut.map(|_| ()),
            Err(NoTree::OverBudget {
                steps: 3,
                bytes: source.len()
            })
        );
        assert_eq!(steps, 4);
        let tree = parse(&mut parser, "fn b() {}\nfn c() {}\n").unwrap();
        let root = tree.root_node();
        assert!(!root.has_error());
        assert_eq!((root.start_byte(), root.end_byte()), (0, 20));
        assert_eq!(root.named_child_count(), 2);
    }

    /// A parser with no language returns no tree without using a step.
    #[test]
    fn no_tree_without_a_cut_is_not_over_budget() {
        let (tree, steps) = parse_counting_steps(&mut Parser::new(), b"x", step_budget(1));
        assert_eq!(tree.map(|_| ()), Err(NoTree::NotParsed));
        assert_eq!(steps, 0);
    }

    /// The control: every Rust source of this repository, the largest of them 700 kB,
    /// parses whole and takes less than a hundredth of its budget.
    #[cfg(feature = "lang-rust")]
    #[test]
    fn the_sources_of_this_repository_take_a_hundredth_of_their_budget() {
        let root = env!("CARGO_MANIFEST_DIR");
        let mut dirs = vec![
            PathBuf::from(format!("{root}/src")),
            PathBuf::from(format!("{root}/tests")),
            PathBuf::from(format!("{root}/fuzz/fuzz_targets")),
        ];
        let mut parser = rust_parser();
        let (mut files, mut largest, mut dearest) = (0usize, 0usize, 0f64);
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    let budget = step_budget(text.len());
                    let (tree, steps) = parse_counting_steps(&mut parser, text.as_bytes(), budget);
                    let tree = tree.unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                    assert_eq!(tree.root_node().end_byte(), text.len());
                    assert!(
                        steps * 100 < budget,
                        "{}: {steps} of {budget} steps",
                        path.display()
                    );
                    files += 1;
                    largest = largest.max(text.len());
                    dearest = dearest.max(steps as f64 / text.len().max(1) as f64);
                }
            }
        }
        assert!(files > 100, "{files}");
        assert!(largest > 500_000, "{largest}");
        // The figure `STEPS_PER_BYTE` is set against.
        assert!(dearest < STEPS_PER_BYTE as f64 / 200.0, "{dearest}");
    }
}
