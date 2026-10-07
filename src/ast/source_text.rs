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
//! A parse that finishes can still give a tree no walker here may descend: most of them
//! call themselves once for each level of the tree, so a source nested deeply enough
//! would end the process with a stack overflow and no report. [`parse_within_budget`]
//! measures the tree it parsed ([`tree_depth`], a walk that keeps no stack of its own)
//! and gives no tree past [`TREE_DEPTH_LIMIT`]. A walker therefore never meets a tree
//! deeper than the limit, whichever caller parsed it.
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

/// Bytes of text that add one step to a parse's budget. The sources of this repository
/// take at most 0.016 steps per byte with their own grammar (`tests/test_gates_e2e.rs`,
/// the largest, takes 0.010), and at most 0.05 when each is parsed with every other
/// grammar, which is error recovery from the first line to the last. One step for every
/// four bytes is 16 times the first figure and 5 times the second.
///
/// The rate also bounds what a parse that never finishes costs, since it runs its whole
/// budget: a 34,026-byte input on which the Rust grammar's error recovery did not
/// finish ran 136,360 steps at four steps per byte, and runs 8,762 at this rate.
pub(crate) const BYTES_PER_STEP: u64 = 4;

/// The steps a parse of `len` bytes may take.
pub(crate) fn step_budget(len: usize) -> u64 {
    STEP_FLOOR.saturating_add(len as u64 / BYTES_PER_STEP)
}

/// The deepest a syntax tree may nest and be read, counted in nodes from the root (the
/// first level) to the deepest node.
///
/// It is set from two measurements. Above it: of 29,795 sources of other projects, the
/// deepest nests 2,056 levels (a header of generated macros; a table written as one
/// concatenation of several hundred strings nests 947, the sources of this repository
/// at most 77), and the limit is twice that. Below it: the walker that takes the most
/// stack for a level takes 3,400 bytes in a build without optimisation
/// (`crate::deep_stack::MEASURED_STACK_PER_LEVEL`), so a tree at the limit takes
/// 4,096 x 3,400 = 13.9 MB of stack there, and the stack the work runs on
/// (`crate::deep_stack::WORK_STACK_BYTES`, 256 MiB) holds that 19 times; the test
/// `the_stack_holds_the_deepest_tree_four_times` requires four.
///
/// The limit protects a walker on that stack only. On a thread with a default stack a
/// tree well inside the limit ends the process (623 levels on 2 MiB), so whatever
/// hands a pack a deeply nested source does it inside `deep_stack::on_deep_stack`, as
/// the binary does for everything.
pub(crate) const TREE_DEPTH_LIMIT: usize = 4_096;

/// Why a parse has no tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoTree {
    /// The parser was still working when its budget ended.
    OverBudget { steps: u64, bytes: usize },
    /// The tree nests deeper than [`TREE_DEPTH_LIMIT`].
    TooDeep { depth: usize },
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
            NoTree::TooDeep { depth } => write!(
                f,
                "the source nests {depth} levels deep, past the {TREE_DEPTH_LIMIT} this tool reads"
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

thread_local! {
    /// Why a part of the file being read had no tree when a pack parsed it again on its
    /// own ([`parse_part`]).
    static UNREAD_PART: std::cell::Cell<Option<NoTree>> = const { std::cell::Cell::new(None) };
}

/// [`parse`] for a part of a file that a pack parses again on its own: the arguments of
/// a Rust macro read as an expression, a Python skip condition written as a string.
///
/// Such a part can nest where the file's tree does not. A macro's arguments are a flat
/// run of tokens in the file's tree, and a string is one node, so `assert!(!!..!true)`
/// and `skipif("((..True..))")` pass the file's depth check at any nesting; parsed as an
/// expression each nests once for every `!` or parenthesis. A part with no tree was not
/// read, and what it would have said (a constant assertion, an unconditional skip) is
/// not known. `None` here records why, and [`unread_part`] refuses the whole file.
pub(crate) fn parse_part(parser: &mut Parser, text: &str) -> Option<Tree> {
    match parse(parser, text) {
        Ok(tree) => Some(tree),
        Err(why) => {
            UNREAD_PART.with(|unread| unread.set(Some(why)));
            None
        }
    }
}

/// Forgets what [`parse_part`] recorded: a pack calls it before it reads a file.
pub(crate) fn forget_unread_part() {
    UNREAD_PART.with(|unread| unread.set(None));
}

/// The error of the file at `path` when a part of it had no tree since
/// [`forget_unread_part`]: a pack calls it once it has read the file, so a file with a
/// part nobody read has no facts, as a file with no tree has none.
pub(crate) fn unread_part(path: &str) -> anyhow::Result<()> {
    match UNREAD_PART.with(|unread| unread.take()) {
        Some(why) => Err(anyhow::anyhow!(
            "could not parse `{path}`: in a part of it parsed on its own, {why}"
        )),
        None => Ok(()),
    }
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

/// The syntax tree of `text` as it stands, within [`step_budget`] and no deeper than
/// [`TREE_DEPTH_LIMIT`]. The caller has taken out what its grammar must not be given
/// ([`parse`] does for source text).
pub(crate) fn parse_within_budget(parser: &mut Parser, text: &[u8]) -> Result<Tree, NoTree> {
    #[cfg(test)]
    let budget = BUDGET_OF_THIS_TEST.with(|b| b.get().unwrap_or(step_budget(text.len())));
    #[cfg(not(test))]
    let budget = step_budget(text.len());
    let tree = parse_counting_steps(parser, text, budget).0?;
    // Before any caller has the tree: no walker is handed one it cannot descend.
    match tree_depth(&tree) {
        depth if depth > TREE_DEPTH_LIMIT => Err(NoTree::TooDeep { depth }),
        _ => Ok(tree),
    }
}

/// The levels of `tree`: the nodes from its root to its deepest node, both counted.
///
/// One walk of the tree with a cursor, which moves to a node's first child, its next
/// sibling or its parent and keeps the way back in a list of its own. Nothing here
/// calls itself, so the walk takes the same stack for a tree of any depth; it ends
/// when the cursor is back at the root, after one visit of each node.
pub(crate) fn tree_depth(tree: &Tree) -> usize {
    let mut cursor = tree.walk();
    // The level is counted here: the cursor's own `depth` walks its list on each call.
    let (mut level, mut deepest) = (1usize, 1usize);
    loop {
        if cursor.goto_first_child() {
            level += 1;
            deepest = deepest.max(level);
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return deepest;
            }
            level -= 1;
        }
    }
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
        assert_eq!(step_budget(15), STEP_FLOOR + 3);
        assert_eq!(step_budget(1_000_000), STEP_FLOOR + 250_000);
        // The largest input the fuzz job gives a target is 64 KiB; a parse of it that
        // never finishes runs this many steps and no more.
        assert_eq!(step_budget(64 * 1024), 16_640);
        assert_eq!(
            step_budget(usize::MAX),
            STEP_FLOOR + usize::MAX as u64 / BYTES_PER_STEP
        );
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
            "the parser did not finish within its budget of 259 steps for 15 bytes"
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

    /// `depth` nested parentheses as the value of a `let` in a Rust function: a tree
    /// five levels deep (file, function, block, `let`, the innermost literal) plus one
    /// for each pair.
    #[cfg(feature = "lang-rust")]
    fn nested_rust(pairs: usize) -> String {
        format!(
            "fn f() {{ let _x = {}1{}; }}\n",
            "(".repeat(pairs),
            ")".repeat(pairs)
        )
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn the_depth_of_a_tree_is_its_longest_path_counted_in_nodes() {
        let depth_of = |text: &str| {
            let (tree, _) =
                parse_counting_steps(&mut rust_parser(), text.as_bytes(), step_budget(text.len()));
            tree_depth(&tree.unwrap())
        };
        // The file alone, then a function with an empty body: file, function, and the
        // function's name, parameters and block beside each other.
        assert_eq!(depth_of(""), 1);
        assert_eq!(depth_of("fn f() {}\n"), 4);
        // A sibling adds no level; a level is added under the deepest node only.
        assert_eq!(depth_of("fn f() {}\nfn g() {}\nfn h() {}\n"), 4);
        assert_eq!(depth_of(&nested_rust(0)), 5);
        assert_eq!(depth_of(&nested_rust(1)), 6);
        assert_eq!(depth_of(&nested_rust(40)), 45);
        // The deepest path is found wherever it is among the siblings.
        let middle = format!("fn a() {{}}\n{}fn z() {{}}\n", nested_rust(40));
        assert_eq!(depth_of(&middle), 45);
    }

    /// The deepest tree read and the first one refused, one level apart.
    #[cfg(feature = "lang-rust")]
    #[test]
    fn a_tree_at_the_limit_is_read_and_one_level_deeper_is_refused() {
        // On the deep stack, as every source is read.
        crate::deep_stack::on_deep_stack(|| {
        let at_limit = nested_rust(TREE_DEPTH_LIMIT - 5);
        let tree = parse(&mut rust_parser(), &at_limit).unwrap();
        assert_eq!(tree_depth(&tree), TREE_DEPTH_LIMIT);
        assert!(!tree.root_node().has_error());

        let over = nested_rust(TREE_DEPTH_LIMIT - 4);
        let refused = parse(&mut rust_parser(), &over).map(|_| ()).unwrap_err();
        assert_eq!(
            refused,
            NoTree::TooDeep {
                depth: TREE_DEPTH_LIMIT + 1
            }
        );
        assert_eq!(
            refused.to_string(),
            "the source nests 4097 levels deep, past the 4096 this tool reads"
        );
        assert_eq!(
            parse_file(&mut rust_parser(), "src/deep.rs", &over)
                .map(|_| ())
                .unwrap_err()
                .to_string(),
            "could not parse `src/deep.rs`: the source nests 4097 levels deep, past the 4096 this tool reads"
        );
        // The parser is not left holding anything: its next text is read from the start.
        let mut parser = rust_parser();
        assert!(parse(&mut parser, &over).is_err());
        assert_eq!(tree_depth(&parse(&mut parser, "fn f() {}\n").unwrap()), 4);
        })
        .unwrap();
    }

    /// A part of a file parsed on its own: one past the limit has no tree, and the file
    /// it is part of is refused once, with the reason. A part that is read, or one that
    /// parses with errors, refuses nothing.
    #[cfg(feature = "lang-rust")]
    #[test]
    fn a_part_with_no_tree_refuses_its_file_once() {
        // On the deep stack, as every source is read.
        crate::deep_stack::on_deep_stack(|| {
            forget_unread_part();
            assert!(parse_part(&mut rust_parser(), &nested_rust(10)).is_some());
            assert!(parse_part(&mut rust_parser(), "fn f( {").is_some());
            assert!(unread_part("src/a.rs").is_ok());

            assert!(parse_part(&mut rust_parser(), &nested_rust(TREE_DEPTH_LIMIT)).is_none());
            // A part read after it does not clear what the first one recorded.
            assert!(parse_part(&mut rust_parser(), &nested_rust(10)).is_some());
            assert_eq!(
                unread_part("src/a.rs").unwrap_err().to_string(),
                "could not parse `src/a.rs`: in a part of it parsed on its own, the source nests \
             4101 levels deep, past the 4096 this tool reads"
            );
            // Taken with the error: the next file starts with nothing recorded.
            assert!(unread_part("src/b.rs").is_ok());

            // A part cut at its budget is one nobody read as well.
            let cut = with_step_budget(0, || {
                parse_part(&mut rust_parser(), &"fn a() -> u8 { 1 }\n".repeat(400))
            });
            assert!(cut.is_none());
            assert!(unread_part("src/c.rs").is_err());
            assert!(parse_part(&mut rust_parser(), &nested_rust(TREE_DEPTH_LIMIT)).is_none());
            forget_unread_part();
            assert!(unread_part("src/c.rs").is_ok());
        })
        .unwrap();
    }

    /// The measurement itself takes no stack for a level: a tree 100,000 levels deep is
    /// parsed and measured on a thread whose whole stack is 256 KiB, which a walker that
    /// calls itself for each level exhausts long before (the cheapest one measured takes
    /// 480 bytes a level in an unoptimised build). The parser is within its budget, so it
    /// is the depth that refuses such a source, not the budget.
    #[cfg(feature = "lang-rust")]
    #[test]
    fn measuring_a_tree_takes_no_stack_for_a_level() {
        const PAIRS: usize = 100_000;
        let measured = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let text = nested_rust(PAIRS);
                let budget = step_budget(text.len());
                let (tree, steps) =
                    parse_counting_steps(&mut rust_parser(), text.as_bytes(), budget);
                let depth = tree_depth(&tree.unwrap());
                let refused = parse(&mut rust_parser(), &text).map(|_| ()).unwrap_err();
                (depth, steps, budget, refused)
            })
            .unwrap()
            .join()
            .unwrap();
        let (depth, steps, budget, refused) = measured;
        assert_eq!(depth, PAIRS + 5);
        assert!(steps < budget, "{steps} of {budget} steps");
        assert_eq!(refused, NoTree::TooDeep { depth: PAIRS + 5 });
    }

    /// The limit against the sources of this repository: fifty times as deep as the
    /// deepest of them. (Against the stack: `deep_stack`'s
    /// `the_stack_holds_the_deepest_tree_four_times`.)
    #[cfg(feature = "lang-rust")]
    #[test]
    fn the_sources_of_this_repository_nest_a_fiftieth_of_the_limit() {
        let root = env!("CARGO_MANIFEST_DIR");
        let mut dirs = vec![
            PathBuf::from(format!("{root}/src")),
            PathBuf::from(format!("{root}/tests")),
            PathBuf::from(format!("{root}/fuzz/fuzz_targets")),
        ];
        let mut parser = rust_parser();
        let (mut files, mut deepest, mut deepest_file) = (0usize, 0usize, PathBuf::new());
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    let tree = parse(&mut parser, &text)
                        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                    let depth = tree_depth(&tree);
                    if depth > deepest {
                        (deepest, deepest_file) = (depth, path);
                    }
                    files += 1;
                }
            }
        }
        assert!(files > 100, "{files}");
        assert!(
            deepest * 50 <= TREE_DEPTH_LIMIT,
            "{}: {deepest} levels",
            deepest_file.display()
        );
        // A tree is at least a few levels deep, so the walk did measure something.
        assert!(deepest > 40, "{deepest}");
    }

    /// The control: every Rust source of this repository, the largest of them 700 kB,
    /// parses whole and takes less than a tenth of its budget.
    #[cfg(feature = "lang-rust")]
    #[test]
    fn the_sources_of_this_repository_take_a_tenth_of_their_budget() {
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
                        steps * 10 < budget,
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
        // The figure `BYTES_PER_STEP` is set against.
        assert!(dearest < 0.1 / BYTES_PER_STEP as f64, "{dearest}");
    }
}
