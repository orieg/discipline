//! Calls inside test bodies that a test gate wants counted: sleeps, assertions on
//! properties that are nearly always true, and the use of test doubles (`mocks.rs`).
//!
//! One walk ([`count`]): call nodes, judged by the callee and the call text up to the
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
    super::ancestry::count(bytes.len());
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
    super::ancestry::count(1);
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

/// Whether `child`, a child of the blanked node `within` (a string, or a part of one that
/// is not code), is an expression the string interpolates.
fn is_interpolated(within: Node, child: Node) -> bool {
    INTERPOLATION_KINDS.contains(&child.kind())
        || (PHP_INTERPOLATING.contains(&within.kind())
            && child.is_named()
            && !PHP_TEXT_PARTS.contains(&child.kind())
            && !is_text(child))
}

/// Puts back the interpolated expressions of a blanked string.
fn restore_code(text_node: Node, src: &[u8], base: usize, out: &mut [u8]) {
    super::ancestry::count(1);
    let mut cursor = text_node.walk();
    for child in text_node.children(&mut cursor) {
        if is_interpolated(text_node, child) {
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

/// [`names`] for a call whose texts lie inside the callee of a call around it that
/// `vocab` did not name (`whole` is the call's text, of which its head is the start).
///
/// Nothing inside that callee is an entry, or an entry at the start of a name, so what
/// is left to read is where this call's texts differ from a stretch of it: an entry that
/// begins where the callee or the head begins (the character before it is not part of
/// either), and an entry that ends at the `(` the tail adds to the callee. Each is read
/// in the length of the entry, not of the call.
fn names_at_edges(vocab: Vocab, callee: &str, whole: &str) -> bool {
    let in_a_name = |before: &str| {
        before
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    };
    vocab.entries.iter().any(|v| {
        let bare = vocab.whole_name && !v.contains(['.', ':']);
        // The tail is the callee and a `(`: an entry ends there when it ends in `(`
        // and the callee ends in the rest of it.
        let ends_the_tail = v.strip_suffix('(').is_some_and(|stem| {
            callee
                .strip_suffix(stem)
                .is_some_and(|before| !bare || !in_a_name(before))
        });
        if !bare {
            return ends_the_tail;
        }
        // The head is the call's text up to its first `(` or `{`: it begins with the
        // entry when the text does and the entry has neither before its last character.
        let fits_the_head = v
            .char_indices()
            .next_back()
            .is_some_and(|(last, _)| !v[..last].contains(['(', '{']));
        callee.starts_with(v) || (fits_the_head && whole.starts_with(v)) || ends_the_tail
    })
}

#[cfg(test)]
thread_local! {
    /// Set by a test that wants every call read in full ([`count`] with no range known).
    static READ_EVERY_CALL_IN_FULL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether [`count`] may leave unread what the callee of a call around a call has shown.
fn ranges_are_kept() -> bool {
    #[cfg(test)]
    return !READ_EVERY_CALL_IN_FULL.with(std::cell::Cell::get);
    #[cfg(not(test))]
    true
}

/// A byte range of the file, and the counts ([`count`]'s bits) whose words it does not
/// hold: the callee of a call that was read in full and that those counts did not match.
/// A call whose text lies inside it is not read again for them (`names_at_edges`).
#[derive(Clone, Copy)]
struct Unnamed {
    counts: u32,
    from: usize,
    to: usize,
    /// The range is the head of a call: it ends with the first `(` or `{` after its
    /// start, or holds neither. The head of a call that starts inside it ends inside
    /// it too.
    is_a_head: bool,
}

impl Unnamed {
    const NOTHING: Unnamed = Unnamed {
        counts: 0,
        from: 0,
        to: 0,
        is_a_head: false,
    };

    /// Whether the texts the call `node` is judged by, its callee (`callee`, or the
    /// whole call when it has none) and its head, lie inside the range.
    fn holds(&self, node: Node, callee: Option<Node>) -> bool {
        let within = |n: Node| self.from <= n.start_byte() && n.end_byte() <= self.to;
        within(node)
            || (self.is_a_head
                && (self.from..self.to).contains(&node.start_byte())
                && callee.is_some_and(within))
    }
}

/// What one walk of a file counts into its tests.
pub struct Counted<'a> {
    /// The grammar's call nodes and where their callee is.
    pub spec: &'a MockSpec,
    /// Configured names of mock set-up, beside the built-in ones (`mocks::double`).
    pub mock_setup: &'a [String],
    /// Configured names of mock verification, beside the built-in ones.
    pub mock_verify: &'a [String],
    /// The call vocabularies counted, each into the field its `Pick` selects.
    pub vocabs: &'a [(Vocab, Pick)],
    /// For a pack some of whose call nodes do not hold their arguments as syntax: the
    /// code such a node is judged by for a vocabulary (a Rust macro's token tree, read
    /// by the pack), and `None` for a node read the ordinary way.
    pub judged: &'a dyn Fn(Node, Vocab) -> Option<String>,
}

/// The two vocabularies every pack that counts both counts.
pub const SLEEPS_AND_TRIVIAL_ASSERTS: &[(Vocab, Pick)] = &[
    (SLEEP_VOCAB, sleeps),
    (TRIVIAL_ASSERT_VOCAB, trivial_asserts),
];

/// The text of a file as code ([`code_text`] of its root), made once for a walk and
/// read by the byte range of each node the walk judges.
///
/// [`code_text`] of a node reads the whole of the node: every node under it is visited
/// and every byte copied. A walk that asks it of each call re-reads, for a call nested in
/// `n` others, the same text `n` times. The text of the whole file holds the same bytes
/// at a node's range as [`code_text`] of that node does, except inside a string or a
/// comment, where it holds spaces; the walk knows when it stands there (`blanked`) and
/// asks [`code_text`] of the node itself.
struct FileCode<'a> {
    root: Node<'a>,
    src: &'a str,
    code: std::cell::OnceCell<String>,
}

impl<'a> FileCode<'a> {
    fn new(root: Node<'a>, src: &'a str) -> Self {
        Self {
            root,
            src,
            code: std::cell::OnceCell::new(),
        }
    }

    /// [`code_text`] of `node`. `blanked` says the node stands inside a string or a
    /// comment and not in an expression it interpolates.
    fn of(&self, node: Node, blanked: bool) -> std::borrow::Cow<'_, str> {
        use std::borrow::Cow;
        if blanked {
            return Cow::Owned(code_text(node, self.src));
        }
        let code = self.code.get_or_init(|| code_text(self.root, self.src));
        let base = self.root.start_byte();
        match (
            node.start_byte().checked_sub(base),
            node.end_byte().checked_sub(base),
        ) {
            (Some(from), Some(to)) => code
                .get(from..to)
                .map_or_else(|| Cow::Owned(code_text(node, self.src)), Cow::Borrowed),
            _ => Cow::Owned(code_text(node, self.src)),
        }
    }

    /// A call node's text as code, up to and with the `(` or `{` that opens its first
    /// argument list. Never the arguments: a test's own body would otherwise make the
    /// enclosing `test(...)` call a match.
    fn call_head(&self, node: Node, blanked: bool) -> std::borrow::Cow<'_, str> {
        use std::borrow::Cow;
        let cut = |whole: &str| whole.find(['(', '{']).map_or(whole.len(), |i| i + 1);
        match self.of(node, blanked) {
            Cow::Borrowed(whole) => Cow::Borrowed(&whole[..cut(whole)]),
            Cow::Owned(mut whole) => {
                whole.truncate(cut(&whole));
                Cow::Owned(whole)
            }
        }
    }
}

/// Counts, in one walk and per test: the mock set-ups and mock verifications
/// (`mocks::double`), and the calls whose callee or call prefix names an entry of each
/// vocabulary of `counted.vocabs`. Each count is kept apart from the others: a matching
/// chain counts once for the count it matched, and what is under it is still read for
/// the others. The mock counts read every node; a vocabulary reads nothing inside a
/// string or a comment except what a string interpolates.
///
/// A call is judged by the text of its callee and of its head. In a chain of calls, and
/// for a call with no callee of its own (an object built with a body, a Kotlin or Swift
/// call), that text holds every call under it, so reading each in full reads a nested
/// source once for each level of it. The callee of a call that matched nothing is
/// therefore kept as a range ([`Unnamed`]) and the calls inside it are read at their
/// edges only.
pub fn count(root: Node, src: &str, tests: &mut [TestFn], counted: &Counted) {
    if tests.is_empty() {
        return;
    }
    const MOCKS: u32 = 1;
    let vocab_bit = |at: usize| 2u32 << at;
    let vocabs: u32 = (0..counted.vocabs.len()).map(vocab_bit).sum();
    let code = FileCode::new(root, src);
    // The third part says the node stands inside a string or a comment and not in an
    // expression it interpolates: its bytes are spaces in the text of the file as code.
    let keep_ranges = ranges_are_kept();
    let mut stack = vec![(root, MOCKS | vocabs, false, Unnamed::NOTHING)];
    let mut interpolated = Vec::new();
    while let Some((node, mut live, blanked, mut unnamed)) = stack.pop() {
        super::ancestry::count(1);
        let text_here = is_text(node);
        if text_here && live & vocabs != 0 {
            // Nothing inside a string or a comment is a call a vocabulary names, except
            // what a string interpolates.
            push_interpolations(node, &mut interpolated);
            stack.extend(
                interpolated
                    .drain(..)
                    .map(|n| (n, live & vocabs, false, unnamed)),
            );
            live &= MOCKS;
            if live == 0 {
                continue;
            }
        }
        let blanked = blanked || text_here;
        if counted.spec.call_kinds.contains(&node.kind()) {
            // Code only (`code_text`): the text of a string literal or a comment in the
            // callee chain names nothing.
            let callee_node = counted
                .spec
                .callee_fields
                .iter()
                .find_map(|f| node.child_by_field_name(f));
            let callee = callee_node
                .map(|n| code.of(n, blanked && !is_interpolated(node, n)))
                .unwrap_or_else(|| code.of(node, blanked));
            let line = node.start_position().row + 1;
            let mut plain_head: Option<std::borrow::Cow<str>> = None;
            // The call's texts are stretches of the text of the file, and lie in a
            // callee that was read in full: what that callee did not hold, they do not.
            let inside = keep_ranges && !blanked && unnamed.holds(node, callee_node);
            // The counts this call's callee is read in full for, and does not match; and
            // those of them its head is read for too (a Rust macro is judged by what
            // its pack reads of it, not by its head).
            let (mut read_in_full, mut head_read) = (0, 0);
            if live & MOCKS != 0 {
                // Judge the callee (for a chained matcher, `expect(f).toHaveBeenCalled`,
                // it carries the matcher) and the call text up to its first argument
                // list (`verify(`, `new Mock<T>(`, `every {`).
                let double = if inside && unnamed.counts & MOCKS != 0 {
                    None
                } else {
                    read_in_full |= MOCKS;
                    head_read |= MOCKS;
                    let head = plain_head.get_or_insert_with(|| code.call_head(node, blanked));
                    super::ancestry::count(callee.len() + head.len());
                    super::mocks::double(&callee, head, counted.mock_setup, counted.mock_verify)
                };
                if let Some(double) = double {
                    if let Some(t) = super::innermost_test(tests, line) {
                        match double {
                            super::mocks::Double::Verify => t.mock_asserts += 1,
                            super::mocks::Double::Setup => t.mock_setups += 1,
                        }
                    }
                    // `when(x).thenReturn(y)` nests a matching call; count the chain once.
                    live &= !MOCKS;
                }
            }
            for (at, (vocab, pick)) in counted.vocabs.iter().enumerate() {
                if live & vocab_bit(at) == 0 {
                    continue;
                }
                let judged = (counted.judged)(node, *vocab);
                // The callee's tail is judged too: a chain's head (`a.b(`) stops before
                // the `sleep(` it ends in.
                let named = match &judged {
                    Some(head) => {
                        read_in_full |= vocab_bit(at);
                        super::ancestry::count(callee.len() + head.len());
                        names(*vocab, &callee, head)
                    }
                    None if inside && unnamed.counts & vocab_bit(at) != 0 => {
                        super::ancestry::count(1);
                        names_at_edges(*vocab, &callee, &code.of(node, blanked))
                    }
                    None => {
                        read_in_full |= vocab_bit(at);
                        head_read |= vocab_bit(at);
                        let head = plain_head.get_or_insert_with(|| code.call_head(node, blanked));
                        super::ancestry::count(callee.len() + head.len());
                        names(*vocab, &callee, head)
                    }
                };
                if named {
                    if let Some(t) = super::innermost_test(tests, line) {
                        *pick(t) += 1;
                    }
                    live &= !vocab_bit(at);
                }
            }
            if live == 0 {
                continue;
            }
            // What is still counted and was read in full did not match: the callee
            // holds none of those words, for the calls inside it, and neither does the
            // head. The head is kept when it holds the callee (a call with no
            // parenthesis before its block, whose head runs on into the block), the
            // callee otherwise (a chain of calls). A call inside a kept range keeps that
            // range, which is the wider one.
            if !inside && !blanked && live & read_in_full != 0 {
                let callee_range = callee_node.unwrap_or(node).byte_range();
                let head_range = plain_head
                    .as_ref()
                    .map(|head| node.start_byte()..node.start_byte() + head.len())
                    .filter(|head| {
                        live & head_read != 0
                            && head.start <= callee_range.start
                            && callee_range.end <= head.end
                    });
                unnamed = match head_range {
                    Some(head) => Unnamed {
                        counts: live & head_read,
                        from: head.start,
                        to: head.end,
                        is_a_head: true,
                    },
                    None => Unnamed {
                        counts: live & read_in_full,
                        from: callee_range.start,
                        to: callee_range.end,
                        is_a_head: false,
                    },
                };
            }
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push((
                child,
                live,
                blanked && !is_interpolated(node, child),
                unnamed,
            ));
        }
    }
}

/// The interpolated expressions of a string, for a walk that does not enter strings.
fn push_interpolations<'t>(text_node: Node<'t>, stack: &mut Vec<Node<'t>>) {
    let mut cursor = text_node.walk();
    for child in text_node.children(&mut cursor) {
        if is_interpolated(text_node, child) {
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
                if let Some(test) = super::innermost_test(tests, line) {
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
    /// The text of a file as code, read by the range of a node, is the text of that node
    /// as code, for every node of the file, in a string or a comment (where the text of
    /// the file holds spaces, and the node is read on its own) and in an expression a
    /// string interpolates; and the head of every node is that text cut after its first
    /// `(` or `{`. The walk says where each node stands as `count` does.
    #[test]
    fn the_text_of_a_file_read_by_range_is_the_text_of_each_node() {
        use tree_sitter::Parser;
        let sources: Vec<(tree_sitter::Language, &str)> = vec![
            (
                tree_sitter_javascript::LANGUAGE.into(),
                "f(`a ${g(`b ${h('c')} d`)} e`, 'f(1)') // k(2)\nconst s = 'x'; t(`${u()}`);\n",
            ),
            (
                tree_sitter_python::LANGUAGE.into(),
                "f(f\"a {g('x')} b {h(f'{k(1)}')}\", \"c(2)\")  # d(3)\n",
            ),
            (
                tree_sitter_php::LANGUAGE_PHP.into(),
                "<?php\nf(\"a {$x->g('b')} c $y d\", 'h(1)'); // k(2)\n$s = <<<EOT\na {$x->m()} b\nEOT;\n",
            ),
            (
                tree_sitter_ruby::LANGUAGE.into(),
                "f(\"a #{g('b')} c\", 'h(1)') # k(2)\nx = /a#{m(1)}b/\n",
            ),
            (
                tree_sitter_c_sharp::LANGUAGE.into(),
                "class C { void M() { F($\"a {G(\"b\")} c\", \"H(1)\"); /* K(2) */ } }\n",
            ),
            (
                tree_sitter_kotlin_ng::LANGUAGE.into(),
                "fun m() { f(\"a ${g(\"b\")} c\", \"h(1)\") /* k(2) */ }\n",
            ),
            (
                tree_sitter_scala::LANGUAGE.into(),
                "object O { def m() = f(s\"a ${g(\"b\")} c\", \"h(1)\") /* k(2) */ }\n",
            ),
            (
                tree_sitter_rust::LANGUAGE.into(),
                "fn a() { f(\"b(1)\", 'c', r#\"d(2)\"#); /* e(3) */ g(h(1)).k(); }\n",
            ),
        ];
        let (mut read, mut in_text) = (0, 0);
        for (language, src) in sources {
            let mut parser = Parser::new();
            parser.set_language(&language).unwrap();
            let tree = crate::ast::source_text::parse(&mut parser, src).unwrap();
            let code = super::FileCode::new(tree.root_node(), src);
            let mut stack = vec![(tree.root_node(), false)];
            while let Some((node, blanked)) = stack.pop() {
                let blanked = blanked || super::is_text(node);
                let whole = super::code_text(node, src);
                assert_eq!(code.of(node, blanked), whole, "{} of {src:?}", node.kind());
                let cut = whole.find(['(', '{']).map_or(whole.len(), |i| i + 1);
                assert_eq!(
                    code.call_head(node, blanked),
                    &whole[..cut],
                    "{} of {src:?}",
                    node.kind()
                );
                read += 1;
                in_text += usize::from(blanked && !whole.trim().is_empty());
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    stack.push((child, blanked && !super::is_interpolated(node, child)));
                }
            }
        }
        // Nodes whose own text is code although a string holds them were among those
        // read: the ones the text of the file has only spaces for.
        assert!(read > 400, "{read}");
        assert!(in_text > 10, "{in_text}");
    }
    /// A walk that keeps the callee of a call that matched nothing, and reads the calls
    /// inside it at their edges, counts what a walk that reads every call in full
    /// counts: for chains and nestings of calls that hold a counted word at the start of
    /// a callee, in its middle, at its end, after an identifier character, inside a
    /// string, and inside an expression a string interpolates.
    #[test]
    fn a_call_inside_a_callee_that_matched_nothing_counts_as_one_read_in_full() {
        let sources: &[(&str, &str)] = &[
            (
                "src/m.test.js",
                "test('t', () => {\n  a.sleep().b().c();\n  x.retry_delay().delay(1).z();\n  expect(f).toBeDefined().and.x();\n  api.fn().mockReturnValue(1).y();\n  new Foo(sleep(1)).go().on();\n  g(h(setTimeout(f, 1)));\n  wrap(() => { jest.fn(); }).then(() => { expect(m).toHaveBeenCalled(); }).done();\n  s(`a ${delay(2)} b`).t('sleep(3)');\n  q.w().e().r().delay(4);\n  delay(5).a().b();\n  u.i(o.sleep(6)).p();\n  outer(sleep /* c */ (7)).z().y();\n  outer(o . delay /* c */ (8)).z().y();\n});\n",
            ),
            (
                "src/test/java/MTest.java",
                "class MTest {\n  @Test void t() {\n    new Runnable() { public void run() { verify(m).x(); Thread.sleep(1); } }.run();\n    when(a.b()).thenReturn(1).z();\n    assertNotNull(x.y().z());\n    a.b().c().sleep(2);\n    new Box(new Box(new Box(mock(A.class)))).open();\n    assertEquals(1, f());\n  }\n}\n",
            ),
            (
                "src/test/kotlin/MTest.kt",
                "class MTest {\n  @Test fun t() {\n    run { run { every { m.x() } returns 1; delay(1) } }\n    x.a().b().sleep(1)\n    verify { m.y() }\n    a.b().c().delay(2).d()\n    wrap(mockk<A>()).e().f()\n    assertEquals(1, f())\n  }\n}\n",
            ),
            (
                "Tests/MTests.swift",
                "class MTests: XCTestCase {\n  func testT() {\n    a.b().c().sleep(1)\n    wrap(usleep(2)).d().e()\n    XCTAssertNotNil(x.y().z())\n    run { run { sleep(3) } }.f()\n    XCTAssertEqual(f(), 1)\n  }\n}\n",
            ),
            (
                "tests/test_m.py",
                "def test_t():\n    a.b().c().sleep(1)\n    time.sleep(2)\n    m = Mock(spec=A).x().y()\n    m.assert_called_once().z()\n    w(f\"a {time.sleep(3)} b\").q('sleep(4)')\n    asyncio.sleep(5).a().b()\n    assert f() == 1\n",
            ),
            (
                "m_test.go",
                "package p\n\nfunc TestT(t *testing.T) {\n\ta.b().c().Sleep(1)\n\ttime.Sleep(2)\n\tg(h(time.Sleep(3))).x().y()\n\tassert.NotNil(t, x.y().z())\n\tctrl := gomock.NewController(t).a().b()\n\tif f() != 1 {\n\t\tt.Fatal()\n\t}\n}\n",
            ),
            (
                "tests/MTests.cs",
                "class MTests {\n  [Fact] public void T() {\n    new Mock<A>().Setup(x => x.F()).Returns(1).Go();\n    Task.Delay(1).Wait().Then();\n    a.B().C().Thread.Sleep(2);\n    Assert.NotNull(x.Y().Z());\n    m.Verify(x => x.F()).And().More();\n    Assert.Equal(1, F());\n  }\n}\n",
            ),
            (
                "spec/m_spec.rb",
                "describe 'd' do\n  it 't' do\n    allow(x).to receive(:y).and_return(1)\n    a.b().c().sleep(1)\n    sleep 2\n    wrap(sleep(3)).d().e()\n    expect(x).to have_received(:y).once\n    expect(f).to eq(1)\n  end\nend\n",
            ),
            (
                "spec/nested_spec.rb",
                "describe 'a' do\n  describe 'b' do\n    context 'c' do\n      it 't' do\n        sleep 1\n        x.delay 2\n        y.retry_delay 3\n        allow(z).to receive(:w)\n        expect(f).to eq(1)\n      end\n      it 'u' do\n        usleep 4\n        expect(g).to eq(2)\n      end\n    end\n  end\nend\n",
            ),
            (
                "tests/MTest.php",
                "<?php\nclass MTest extends TestCase {\n  public function testT() {\n    $this->createMock(A::class)->method('a')->willReturn(1)->b();\n    usleep(1);\n    $a->b()->c()->sleep(2);\n    wrap(sleep(3))->d()->e();\n    $o = new class { public function m() { sleep(4); } };\n    $this->assertSame(1, f());\n  }\n}\n",
            ),
            (
                "src/test/scala/MTest.scala",
                "class MTest extends AnyFunSuite {\n  test(\"t\") {\n    a.b().c().sleep(1)\n    Thread.sleep(2)\n    wrap(mock[A]).d().e()\n    verify(m).x().y()\n    assert(f() == 1)\n  }\n}\n",
            ),
            (
                "tests/t.rs",
                "#[test]\nfn t() {\n    a.b().c().sleep(1);\n    std::thread::sleep(d);\n    wrap(thread::sleep(d)).e().f();\n    assert!(x.y().z().is_some());\n    let m = MockA::new().expect_f().returning(|| 1).g();\n    retry_delay(1).delay(2);\n    assert_eq!(f(), 1);\n}\n",
            ),
            (
                "tests/test_m.cpp",
                "TEST(S, T) {\n  a.b().c().sleep(1);\n  usleep(2);\n  wrap(sleep(3)).d().e();\n  EXPECT_CALL(m, F()).Times(1).WillOnce(Return(1));\n  EXPECT_EQ(f(), 1);\n}\n",
            ),
            (
                "Tests/MTests.m",
                "@implementation MTests\n- (void)testT {\n  sleep(1);\n  wrap(usleep(2));\n  g(h(sleep(3)));\n  XCTAssertEqual(f(), 1);\n}\n@end\n",
            ),
        ];
        let registry = default_registry();
        let counts =
            |facts: &crate::ast::ParsedFileFacts| -> Vec<(String, usize, usize, usize, usize)> {
                facts
                    .tests
                    .iter()
                    .map(|t| {
                        (
                            t.name.clone(),
                            t.mock_setups,
                            t.mock_asserts,
                            t.sleeps,
                            t.trivial_asserts,
                        )
                    })
                    .collect()
            };
        let (mut sleeps, mut doubles, mut trivial) = (0, 0, 0);
        for (path, src) in sources {
            let pack = registry.find_pack(path).unwrap();
            let read = |in_full: bool| {
                super::READ_EVERY_CALL_IN_FULL.with(|r| r.set(in_full));
                let facts = pack.extract(path, src, &AssertVocabulary::default());
                super::READ_EVERY_CALL_IN_FULL.with(|r| r.set(false));
                facts.unwrap_or_else(|e| panic!("{path}: {e:#}"))
            };
            let (kept, in_full) = (read(false), read(true));
            assert_eq!(counts(&kept), counts(&in_full), "{path}");
            assert_eq!(format!("{kept:?}"), format!("{in_full:?}"), "{path}");
            for (_, setups, asserts, slept, weak) in counts(&in_full) {
                sleeps += slept;
                doubles += setups + asserts;
                trivial += weak;
            }
        }
        // The sources hold what is counted: a walk that counted nothing either way
        // would compare equal too.
        assert!(sleeps >= 13, "{sleeps} sleeps");
        assert!(doubles >= 8, "{doubles} doubles");
        assert!(trivial >= 4, "{trivial} trivial assertions");
    }
    /// For a text that a vocabulary does not name, every call text cut from it (a callee
    /// and a call text that are stretches of it) is named, or not, the same by the
    /// reading at its edges as by the reading in full: every stretch of every text built
    /// from the pieces below, which hold the entries, their parts, and what stands
    /// before and after them.
    #[test]
    fn the_reading_at_the_edges_is_the_reading_in_full_inside_a_text_that_names_nothing() {
        use super::{names, names_at_edges, SLEEP_VOCAB, TRIVIAL_ASSERT_VOCAB};
        const PIECES: &[&str] = &[
            "sleep",
            "delay",
            "(",
            ")",
            ".",
            "_",
            "x",
            " ",
            "{",
            "TimeUnit",
            "is_some()",
            ".to.exist",
            "NotNil",
            "Task.Delay",
        ];
        let head_of = |whole: &str| {
            let cut = whole.find(['(', '{']).map_or(whole.len(), |i| i + 1);
            whole[..cut].to_string()
        };
        let (mut texts, mut compared, mut named) = (0usize, 0usize, 0usize);
        let mut at = [0usize; 3];
        loop {
            let text: String = at.iter().map(|i| PIECES[*i]).collect();
            for vocab in [SLEEP_VOCAB, TRIVIAL_ASSERT_VOCAB] {
                // The text as the callee of a call around: read in full, naming nothing.
                if names(vocab, &text, &head_of(&text)) {
                    continue;
                }
                texts += 1;
                for from in 0..text.len() {
                    for to in from + 1..=text.len() {
                        // A call inside it: its callee is a stretch, and its own text
                        // is that callee or runs on past it.
                        let callee = &text[from..to];
                        for whole_to in [to, text.len()] {
                            let whole = &text[from..whole_to];
                            let in_full = names(vocab, callee, &head_of(whole));
                            assert_eq!(
                                names_at_edges(vocab, callee, whole),
                                in_full,
                                "callee {callee:?} of the call {whole:?} in {text:?}"
                            );
                            compared += 1;
                            named += usize::from(in_full);
                        }
                    }
                }
            }
            // The next text, as the next number in base `PIECES.len()`.
            let mut place = 0;
            loop {
                at[place] += 1;
                if at[place] < PIECES.len() {
                    break;
                }
                at[place] = 0;
                place += 1;
                if place == at.len() {
                    break;
                }
            }
            if place == at.len() {
                break;
            }
        }
        // Texts that name nothing were found, and calls cut from them that are named:
        // by an entry at the start of the callee, or at the `(` the tail adds.
        assert!(texts > 4_000, "{texts}");
        assert!(compared > 300_000, "{compared}");
        assert!(named > 1_000, "{named}");
    }
}
