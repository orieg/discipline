//! Calls inside test bodies that a test gate wants counted: sleeps, and assertions on
//! properties that are nearly always true.
//!
//! Same walk as `mocks.rs`: call nodes, judged by the callee and the call text up to the
//! first argument list, attributed to the innermost test whose span contains them.
//!
//! What is judged is code ([`code_text`]): the text of a string literal or a comment is
//! never read, so a test that holds another program's source in a string is not counted
//! for what that source says.

use super::mocks::MockSpec;
use super::TestFn;
use tree_sitter::Node;

/// Call names a test gate counts, and how a bare entry (`sleep(`, no `.` or `::`) matches.
#[derive(Clone, Copy)]
pub struct Vocab {
    pub entries: &'static [&'static str],
    /// A bare entry names the whole callee or its last segment: `delay(` counts
    /// `delay(` and `this.delay(`, never `retry_delay(`. Qualified entries match anywhere.
    pub whole_name: bool,
    /// The entries describe what an assertion checks. A pack whose assertions are not
    /// call nodes (a Rust macro) then judges the asserted condition and nothing else.
    pub assertions: bool,
}

/// A hard-coded delay in a test: the shape of a race "fixed" by waiting.
pub const SLEEP_VOCAB: Vocab = Vocab {
    entries: SLEEP_CALLS,
    whole_name: true,
    assertions: false,
};

const SLEEP_CALLS: &[&str] = &[
    "thread::sleep(",
    "std::thread::sleep(",
    "tokio::time::sleep(",
    "async_std::task::sleep(",
    "time.sleep(",
    "asyncio.sleep(",
    "time.Sleep(",
    "Thread.sleep(",
    "TimeUnit.",
    "Task.Delay(",
    "Thread.Sleep(",
    "setTimeout(",
    "sleep(",
    "usleep(",
    "delay(",
];

/// An assertion that holds for nearly any value: it counts as an assertion and fails
/// almost nothing. Entries match as substrings: `NotNil(` is meant to count
/// `XCTAssertNotNil(`.
pub const TRIVIAL_ASSERT_VOCAB: Vocab = Vocab {
    entries: TRIVIAL_ASSERTS,
    whole_name: false,
    assertions: true,
};

const TRIVIAL_ASSERTS: &[&str] = &[
    // Python
    "assertIsNotNone(",
    "is not None",
    "assertIsInstance(",
    // JS/TS
    ".toBeDefined(",
    ".toBeTruthy(",
    ".not.toBeNull(",
    ".not.toBeUndefined(",
    ".toBeInstanceOf(",
    ".to.exist",
    ".to.be.ok",
    // Rust
    ".is_some()",
    ".is_ok()",
    ".is_empty() == false",
    // Go
    "NotNil(",
    "NoError(",
    // Java / C#
    "assertNotNull(",
    "IsNotNull(",
    "NotNull(",
    "Assert.NotNull(",
    "Should().NotBeNull(",
];

fn text<'a>(node: Node, src: &'a str) -> &'a str {
    node.utf8_text(src.as_bytes()).unwrap_or("")
}

/// Node kinds whose text is not code: string, character and regular-expression literals
/// and comments, under the names the grammars give them.
const TEXT_KINDS: &[&str] = &[
    // Strings.
    "string",
    "string_literal",
    "raw_string_literal",
    "interpreted_string_literal",
    "verbatim_string_literal",
    "interpolated_string_expression",
    "interpolated_string",
    "template_string",
    "encapsed_string",
    "heredoc",
    "heredoc_body",
    "nowdoc",
    "bare_string",
    "line_string_literal",
    "multi_line_string_literal",
    "multiline_string_literal",
    "xml_string",
    "jsx_text",
    // Characters, symbols and regular expressions.
    "char_literal",
    "character_literal",
    "character",
    "rune_literal",
    "bare_symbol",
    "delimited_symbol",
    "regex",
    "regex_literal",
    // Comments.
    "comment",
    "line_comment",
    "block_comment",
    "multiline_comment",
    "html_comment",
    "xml_comment",
    "doc_comment",
];

/// Code inside a string: an interpolated expression runs, so it is read as code.
const INTERPOLATION_KINDS: &[&str] = &[
    "interpolation",
    "template_substitution",
    "string_interpolation",
    "interpolated_expression",
];

/// PHP interpolates without a wrapping node: every named part of these strings that is
/// not one of `PHP_TEXT_PARTS` is an expression.
const PHP_INTERPOLATING: &[&str] = &["encapsed_string", "heredoc_body"];
const PHP_TEXT_PARTS: &[&str] = &[
    "string_content",
    "escape_sequence",
    "heredoc_start",
    "heredoc_end",
];

/// The source of `node` as code: every byte of a string literal or a comment inside it is
/// a space, except an interpolated expression, which is kept (with its own strings
/// blanked). Offsets are those of the node's text.
pub fn code_text(node: Node, src: &str) -> String {
    let base = node.start_byte();
    let Some(bytes) = src.as_bytes().get(base..node.end_byte()) else {
        return String::new();
    };
    let mut out = bytes.to_vec();
    blank_text(node, src.as_bytes(), base, &mut out);
    // Whole nodes are blanked and whole nodes restored, so the bytes stay valid UTF-8.
    String::from_utf8(out).unwrap_or_default()
}

fn is_text(node: Node) -> bool {
    node.is_named() && TEXT_KINDS.contains(&node.kind())
}

/// Blanks the string literals and comments of a code node.
fn blank_text(node: Node, src: &[u8], base: usize, out: &mut [u8]) {
    if is_text(node) {
        if let Some(part) = out.get_mut(node.start_byte() - base..node.end_byte() - base) {
            part.fill(b' ');
        }
        restore_code(node, src, base, out);
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        blank_text(child, src, base, out);
    }
}

/// Puts back the interpolated expressions of a blanked string.
fn restore_code(text_node: Node, src: &[u8], base: usize, out: &mut [u8]) {
    let php = PHP_INTERPOLATING.contains(&text_node.kind());
    let mut cursor = text_node.walk();
    for child in text_node.children(&mut cursor) {
        let code = INTERPOLATION_KINDS.contains(&child.kind())
            || (php
                && child.is_named()
                && !PHP_TEXT_PARTS.contains(&child.kind())
                && !is_text(child));
        if code {
            let (a, b) = (child.start_byte(), child.end_byte());
            if let (Some(to), Some(from)) = (out.get_mut(a - base..b - base), src.get(a..b)) {
                to.copy_from_slice(from);
            }
            blank_text(child, src, base, out);
        } else {
            restore_code(child, src, base, out);
        }
    }
}

/// Whether `entry` occurs in `hay` as a name of its own: not preceded by an identifier
/// character, so `delay(` is found in `this.delay(` and `delay(` but not `retry_delay(`.
fn at_name_start(hay: &str, entry: &str) -> bool {
    hay.match_indices(entry).any(|(i, _)| {
        !hay[..i]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    })
}

/// Whether a call with this callee and call prefix is one `vocab` names.
fn names(vocab: Vocab, callee: &str, head: &str) -> bool {
    let tail = format!("{callee}(");
    vocab.entries.iter().any(|v| {
        let bare = vocab.whole_name && !v.contains(['.', ':']);
        if bare {
            at_name_start(callee, v) || at_name_start(head, v) || at_name_start(&tail, v)
        } else {
            callee.contains(v) || head.contains(v) || tail.ends_with(v)
        }
    })
}

/// Count calls whose callee or call prefix contains a `vocab` entry, per test, into the
/// field `pick` selects. A matching chain counts once.
pub fn count(
    root: Node,
    src: &str,
    tests: &mut [TestFn],
    spec: &MockSpec,
    vocab: Vocab,
    pick: fn(&mut TestFn) -> &mut usize,
) {
    count_with(root, src, tests, spec, vocab, pick, &|_| None);
}

/// [`count`], for a pack some of whose call nodes do not hold their arguments as syntax:
/// `judged` returns the code such a node is judged by (a Rust macro's token tree, read by
/// the pack), and `None` for a node read the ordinary way.
pub fn count_with(
    root: Node,
    src: &str,
    tests: &mut [TestFn],
    spec: &MockSpec,
    vocab: Vocab,
    pick: fn(&mut TestFn) -> &mut usize,
    judged: &dyn Fn(Node) -> Option<String>,
) {
    if tests.is_empty() {
        return;
    }
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if is_text(node) {
            // Nothing inside a string or a comment is a call, except what a string
            // interpolates.
            push_interpolations(node, &mut stack);
            continue;
        }
        if spec.call_kinds.contains(&node.kind()) {
            let callee = spec
                .callee_fields
                .iter()
                .find_map(|f| node.child_by_field_name(f))
                .map(|n| code_text(n, src))
                .unwrap_or_else(|| code_text(node, src));
            let head = match judged(node) {
                Some(code) => code,
                None => {
                    let mut whole = code_text(node, src);
                    let cut = whole.find(['(', '{']).map_or(whole.len(), |i| i + 1);
                    whole.truncate(cut);
                    whole
                }
            };
            // The callee's tail is judged too: a chain's head (`a.b(`) stops before the
            // `sleep(` it ends in.
            if names(vocab, &callee, &head) {
                let line = node.start_position().row + 1;
                if let Some(t) = tests
                    .iter_mut()
                    .filter(|t| t.line <= line && line <= t.end_line.max(t.line))
                    .min_by_key(|t| t.end_line.saturating_sub(t.line))
                {
                    *pick(t) += 1;
                }
                continue;
            }
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
}

/// The interpolated expressions of a string, for a walk that does not enter strings.
fn push_interpolations<'t>(text_node: Node<'t>, stack: &mut Vec<Node<'t>>) {
    let php = PHP_INTERPOLATING.contains(&text_node.kind());
    let mut cursor = text_node.walk();
    for child in text_node.children(&mut cursor) {
        if INTERPOLATION_KINDS.contains(&child.kind())
            || (php
                && child.is_named()
                && !PHP_TEXT_PARTS.contains(&child.kind())
                && !is_text(child))
        {
            stack.push(child);
        } else {
            push_interpolations(child, stack);
        }
    }
}

/// Python's `assert x is not None` is a statement, not a call. Counted from the syntax of
/// assert statements the walker cannot see as calls: an `is not None` comparison, or a
/// call of `isinstance`. Neither is read out of a string or a comment.
pub fn count_python_assert_statements(root: Node, src: &str, tests: &mut [TestFn]) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "assert_statement" {
            if python_assert_is_trivial(node, src) {
                let line = node.start_position().row + 1;
                if let Some(test) = tests
                    .iter_mut()
                    .filter(|x| x.line <= line && line <= x.end_line.max(x.line))
                    .min_by_key(|x| x.end_line.saturating_sub(x.line))
                {
                    test.trivial_asserts += 1;
                }
            }
            continue;
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
}

fn python_assert_is_trivial(assert: Node, src: &str) -> bool {
    let mut stack = vec![assert];
    while let Some(node) = stack.pop() {
        if is_text(node) {
            push_interpolations(node, &mut stack);
            continue;
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        match node.kind() {
            "comparison_operator" => {
                let not_none = children
                    .windows(2)
                    .any(|w| w[0].kind() == "is not" && w[1].kind() == "none");
                if not_none {
                    return true;
                }
            }
            "call" => {
                let callee = node.child_by_field_name("function");
                if callee.is_some_and(|f| f.kind() == "identifier" && text(f, src) == "isinstance")
                {
                    return true;
                }
            }
            _ => {}
        }
        stack.extend(children);
    }
    false
}

/// Selects the count of a test that a walk adds to.
pub type Pick = fn(&mut TestFn) -> &mut usize;

pub fn sleeps(t: &mut TestFn) -> &mut usize {
    &mut t.sleeps
}

pub fn trivial_asserts(t: &mut TestFn) -> &mut usize {
    &mut t.trivial_asserts
}

#[cfg(all(
    test,
    feature = "lang-python",
    feature = "lang-javascript",
    feature = "lang-rust"
))]
mod tests {
    use crate::ast::{default_registry, AssertVocabulary, TestFn};

    fn tests_of(path: &str, src: &str) -> Vec<TestFn> {
        let reg = default_registry();
        reg.find_pack(path)
            .unwrap()
            .extract(path, src, &AssertVocabulary::default())
            .unwrap()
            .tests
    }

    #[test]
    fn sleeps_and_trivial_assertions_are_counted_per_test() {
        let py = tests_of(
            "tests/test_a.py",
            "import time\n\ndef test_a():\n    time.sleep(0.5)\n    r = run()\n    assert r is not None\n\n\
             def test_b():\n    assert run() == 3\n",
        );
        assert_eq!((py[0].sleeps, py[0].trivial_asserts), (1, 1), "{:?}", py[0]);
        assert_eq!((py[1].sleeps, py[1].trivial_asserts), (0, 0));

        let ts = tests_of(
            "src/a.test.ts",
            "test('a', async () => {\n  await new Promise((r) => setTimeout(r, 200));\n  expect(run()).toBeDefined();\n});\n\
             test('b', () => {\n  expect(run()).toEqual(3);\n});\n",
        );
        assert_eq!((ts[0].sleeps, ts[0].trivial_asserts), (1, 1), "{:?}", ts[0]);
        assert_eq!((ts[1].sleeps, ts[1].trivial_asserts), (0, 0));

        let rs = tests_of(
            "src/lib.rs",
            "#[test]\nfn a() { std::thread::sleep(std::time::Duration::from_millis(50)); assert!(run().is_ok()); }\n\
             #[test]\nfn b() { assert_eq!(run().unwrap(), 3); }\n",
        );
        assert_eq!((rs[0].sleeps, rs[0].trivial_asserts), (1, 1), "{:?}", rs[0]);
        assert_eq!((rs[1].sleeps, rs[1].trivial_asserts), (0, 0));
    }

    #[test]
    fn a_bare_sleep_entry_names_the_whole_callee_or_its_last_segment() {
        use super::{names, SLEEP_VOCAB, TRIVIAL_ASSERT_VOCAB};
        // (callee, call prefix up to the first argument list)
        for (callee, head) in [
            ("retry_delay", "retry_delay("),
            ("compute_delay", "compute_delay("),
            ("nosleep", "nosleep("),
            ("self.no_usleep", "self.no_usleep("),
        ] {
            assert!(!names(SLEEP_VOCAB, callee, head), "{callee}");
        }
        for (callee, head) in [
            ("delay", "delay("),
            ("sleep", "sleep("),
            ("usleep", "usleep("),
            ("std::thread::sleep", "std::thread::sleep("),
            ("Task.Delay", "Task.Delay("),
            ("this.delay", "this.delay("),
            ("$this->sleep", "$this->sleep("),
            ("Kernel::sleep", "Kernel::sleep("),
            ("a.b(x).sleep", "a.b("),
        ] {
            assert!(names(SLEEP_VOCAB, callee, head), "{callee}");
        }
        // Trivial-assertion entries still match inside a longer name.
        assert!(names(
            TRIVIAL_ASSERT_VOCAB,
            "XCTAssertNotNil",
            "XCTAssertNotNil("
        ));
    }

    #[test]
    fn a_sleep_is_the_callee_itself_not_a_name_ending_in_it() {
        let rs = tests_of(
            "src/lib.rs",
            "#[test]\nfn a() { retry_delay(1); compute_delay(); nosleep(); no_sleep(2); }\n\
             #[test]\nfn b() { delay(10); sleep(1); std::thread::sleep(d); Self::sleep(2); }\n",
        );
        assert_eq!(rs[0].sleeps, 0, "{:?}", rs[0]);
        assert_eq!(rs[1].sleeps, 4, "{:?}", rs[1]);

        let ts = tests_of(
            "src/a.test.ts",
            "test('a', async () => {\n  retry_delay(1);\n  api.compute_delay();\n  expect(nosleep()).toEqual(3);\n});\n\
             test('b', async () => {\n  await this.delay(5);\n  await Task.Delay(5);\n  await sleep(1);\n  expect(run()).toEqual(3);\n});\n",
        );
        assert_eq!(ts[0].sleeps, 0, "{:?}", ts[0]);
        assert_eq!(ts[1].sleeps, 3, "{:?}", ts[1]);
    }

    /// The text of a string literal or a comment is not code, in the callee chain of a
    /// call, in a Rust macro, or in a Python assert statement.
    #[test]
    fn text_in_a_string_or_a_comment_is_not_counted() {
        let py = tests_of(
            "tests/test_a.py",
            "def test_a():\n    src = \"time.sleep(1)\".strip()\n    assert run() == \"x is not None\"\n    assert \"isinstance(\" in src  # is not None\n",
        );
        assert_eq!((py[0].sleeps, py[0].trivial_asserts), (0, 0), "{:?}", py[0]);
        let ts = tests_of(
            "src/a.test.ts",
            "test('a', () => {\n  const src = \"setTimeout(f, 1)\".trim();\n  expect(\"x.toBeDefined()\").toBe(run());\n});\n",
        );
        assert_eq!((ts[0].sleeps, ts[0].trivial_asserts), (0, 0), "{:?}", ts[0]);
        let rs = tests_of(
            "tests/a.rs",
            "#[test]\nfn a() {\n    let s = format!(\"thread::sleep(d) {}\", 1 /* r.is_ok() */);\n    let t = \"assert!(r.is_ok())\".to_string();\n    assert_eq!(run(), \"r.is_some()\");\n}\n",
        );
        assert_eq!((rs[0].sleeps, rs[0].trivial_asserts), (0, 0), "{:?}", rs[0]);
        // Control: the same words as code.
        let py = tests_of(
            "tests/test_a.py",
            "def test_a():\n    time.sleep(1)\n    assert run() is not None\n    assert isinstance(run(), Thing)\n",
        );
        assert_eq!((py[0].sleeps, py[0].trivial_asserts), (1, 2), "{:?}", py[0]);
    }

    #[test]
    fn code_text_blanks_strings_and_comments_and_keeps_interpolations() {
        use tree_sitter::Parser;
        let read = |language: tree_sitter::Language, src: &str| {
            let mut parser = Parser::new();
            parser.set_language(&language).unwrap();
            let tree = crate::ast::source_text::parse(&mut parser, src).unwrap();
            super::code_text(tree.root_node(), src)
        };
        let js = "f(`a ${g(\"x\")} b`, 'c') // d\n";
        assert_eq!(
            read(tree_sitter_javascript::LANGUAGE.into(), js),
            "f(   ${g(   )}   ,    )     \n"
        );
        let py = "f(f\"a {g('x')} b\", \"c\")  # d\n";
        assert_eq!(
            read(tree_sitter_python::LANGUAGE.into(), py),
            "f(    {g(   )}   ,    )     \n"
        );
        let rs = "fn a() { f(\"b\", 'c', r#\"d\"#); /* e */ }\n";
        assert_eq!(
            read(tree_sitter_rust::LANGUAGE.into(), rs),
            "fn a() { f(   ,    ,       );         }\n"
        );
        // Text of several bytes per character stays one space per byte.
        let rs = "fn a() { f(\"\u{e9}\"); }\n";
        assert_eq!(
            read(tree_sitter_rust::LANGUAGE.into(), rs),
            "fn a() { f(    ); }\n"
        );
    }
}
