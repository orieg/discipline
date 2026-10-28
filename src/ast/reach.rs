//! Code the test runner never reaches: the body of an `if` whose condition is a constant
//! false, and the statements after an unconditional terminator (`return`, `panic!`,
//! `pytest.fail()`, `throw`) at the same block level. A skip call (`t.Skip`,
//! `pytest.skip`) or a thrown skip exception (`throw XCTSkip(...)`, `raise
//! unittest.SkipTest`) is not a terminator: the test is reported as ignored, and its
//! body stays what it is.
//!
//! A pack may also name code that runs once per element of an empty literal: a callback
//! passed to an iteration method called on `[]` (`[].forEach(() => ..)`), or the body of
//! a loop over one (`for _ in []:`). It runs zero times (#899).
//!
//! An assertion there counts as an assertion to a line count and fails nothing. Each
//! pack's assertion walk skips nodes that start inside a dead range.

use tree_sitter::Node;

pub struct ReachSpec {
    /// `if` node kinds; the condition is the `condition` field or the first named child,
    /// the consequence the `consequence` / `body` field or the next named child.
    pub if_kinds: &'static [&'static str],
    /// Block node kinds whose named children are the statements of one level.
    pub block_kinds: &'static [&'static str],
    /// Node kinds that are not statements inside a block (comments).
    pub ignored_kinds: &'static [&'static str],
    /// Heads of a statement after which nothing at the same level runs.
    pub terminators: &'static [&'static str],
    /// The pack's reading of code that runs once per element of an empty literal: called
    /// with each node of the file, it adds the byte range of each such callback or loop
    /// body to the ranges. `None` where the pack reads none.
    pub empty_iteration: Option<EmptyIteration>,
}

/// See [`ReachSpec::empty_iteration`].
pub type EmptyIteration = fn(Node, &str, &mut DeadRanges);

/// Whether `node` is a literal of one of `kinds` that holds no element: `[]`, `()`. A
/// comment inside it is not an element.
pub fn holds_nothing(node: Node, kinds: &[&str]) -> bool {
    if !kinds.contains(&node.kind()) {
        return false;
    }
    let mut cursor = node.walk();
    let empty = node.named_children(&mut cursor).all(|c| {
        c.kind() == "comment" || c.kind() == "line_comment" || c.kind() == "block_comment"
    });
    empty
}

/// Adds the byte range of `node` to `out`.
pub fn push_range(node: Node, out: &mut DeadRanges) {
    out.push((node.start_byte(), node.end_byte()));
}

/// Byte ranges no execution reaches.
pub type DeadRanges = Vec<(usize, usize)>;

fn text<'a>(node: Node, src: &'a str) -> &'a str {
    node.utf8_text(src.as_bytes()).unwrap_or("")
}

/// The Rust constant-expression check, shared: a condition that is the literal `false`
/// (or `0` where the language reads it as false), with any parentheses stripped.
pub fn is_constant_false(condition: &str) -> bool {
    let t = condition
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim();
    matches!(t, "false" | "False" | "FALSE" | "0" | "0 == 1" | "1 == 0")
}

/// Whether a statement's text starts with a terminator head as a whole word: `return`
/// and `return x`, not `returned = 1`.
/// Exceptions a test framework raises to skip a test (`throw XCTSkip(...)`, `raise
/// unittest.SkipTest`, TestNG's `SkipException`, JUnit's `TestAbortedException`). A
/// statement throwing one is a skip, not a terminator.
const SKIP_EXCEPTIONS: &[&str] = &[
    "XCTSkip",
    "SkipTest",
    "SkipException",
    "skip.Exception",
    "TestAbortedException",
];

fn terminates(t: &str, spec: &ReachSpec) -> bool {
    let t = t.trim();
    // The head first: it is read from the start of the statement, and the whole of the
    // statement is searched for a skip exception only when it has one. A statement that
    // holds blocks is read once for each block around it otherwise.
    spec.terminators.iter().any(|h| {
        t.starts_with(h)
            && t[h.len()..]
                .chars()
                .next()
                .is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
    }) && !SKIP_EXCEPTIONS.iter().any(|s| t.contains(s))
}

pub fn dead_ranges(root: Node, src: &str, spec: &ReachSpec) -> DeadRanges {
    let mut out: DeadRanges = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if spec.if_kinds.contains(&node.kind()) {
            let condition = node
                .child_by_field_name("condition")
                .or_else(|| node.named_child(0));
            let consequence = node
                .child_by_field_name("consequence")
                .or_else(|| node.child_by_field_name("body"))
                .or_else(|| node.named_child(1));
            if let (Some(c), Some(body)) = (condition, consequence) {
                if is_constant_false(text(c, src)) {
                    out.push((body.start_byte(), body.end_byte()));
                }
            }
        }
        if let Some(empty_iteration) = spec.empty_iteration {
            empty_iteration(node, src, &mut out);
        }
        if spec.block_kinds.contains(&node.kind()) {
            let mut cursor = node.walk();
            let stmts: Vec<Node> = node
                .named_children(&mut cursor)
                .filter(|c| !spec.ignored_kinds.contains(&c.kind()))
                .collect();
            if let Some(i) = stmts.iter().position(|s| terminates(text(*s, src), spec)) {
                if let Some(next) = stmts.get(i + 1) {
                    out.push((next.start_byte(), node.end_byte()));
                }
            }
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
    merged(out)
}

/// The same bytes as `ranges` hold, as ranges in order that neither overlap nor touch,
/// for [`is_dead`]: a file of many dead ranges was read through for each node asked
/// about.
fn merged(mut ranges: DeadRanges) -> DeadRanges {
    ranges.retain(|(start, end)| start < end);
    ranges.sort_unstable();
    let mut out: DeadRanges = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        match out.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => out.push((start, end)),
        }
    }
    out
}

/// Whether a node that starts at `at` is inside a dead range. `ranges` is what
/// [`dead_ranges`] returned: in order, none overlapping another.
pub fn is_dead(ranges: &DeadRanges, at: usize) -> bool {
    // The ranges read to answer: none where there is no dead range.
    super::ancestry::count((usize::BITS - ranges.len().leading_zeros()) as usize);
    // The last range that starts at or before `at` is the only one that can hold it.
    ranges
        .partition_point(|(start, _)| *start <= at)
        .checked_sub(1)
        .is_some_and(|last| at < ranges[last].1)
}

#[cfg(test)]
mod tests {
    use super::is_constant_false;

    /// The ranges kept in order answer as the ranges found do, for every byte: ranges
    /// one inside another, overlapping, touching, equal, empty and reversed.
    #[test]
    fn ranges_in_order_hold_the_bytes_the_ranges_found_hold() {
        let found: super::DeadRanges = vec![
            (40, 50),
            (10, 30),
            (12, 14),
            (25, 35),
            (35, 38),
            (60, 60),
            (70, 65),
            (80, 90),
            (80, 90),
            (95, 96),
            (2, 3),
        ];
        let kept = super::merged(found.clone());
        assert_eq!(kept, vec![(2, 3), (10, 38), (40, 50), (80, 90), (95, 96)]);
        for at in 0..100 {
            let held = found.iter().any(|(s, e)| *s <= at && at < *e);
            assert_eq!(super::is_dead(&kept, at), held, "byte {at}");
        }
        assert!(!super::is_dead(&Vec::new(), 0));
    }

    #[test]
    fn constant_false_conditions() {
        assert!(is_constant_false("false"));
        assert!(is_constant_false("(false)"));
        assert!(is_constant_false("0"));
        assert!(is_constant_false("False"));
        assert!(!is_constant_false("flag"));
        assert!(!is_constant_false("x == 0"));
        assert!(!is_constant_false("true"));
    }
}

#[cfg(all(
    test,
    feature = "lang-rust",
    feature = "lang-python",
    feature = "lang-javascript",
    feature = "lang-go",
    feature = "lang-java",
    feature = "lang-kotlin"
))]
mod pack_tests {
    use crate::ast::{default_registry, AssertVocabulary};

    /// `(total, strong)` of each test in `src`, in order.
    fn counts(path: &str, src: &str) -> Vec<(usize, usize)> {
        let reg = default_registry();
        reg.find_pack(path)
            .unwrap()
            .extract(path, src, &AssertVocabulary::default())
            .unwrap()
            .tests
            .iter()
            .map(|t| (t.total_asserts, t.strong_asserts))
            .collect()
    }

    /// A statement that throws a skip exception is not a terminator, wherever in the
    /// statement the exception is named, and a statement that only holds a terminator
    /// further in is not one either.
    #[test]
    fn a_terminator_is_read_from_the_head_of_its_statement() {
        let py = counts(
            "tests/test_a.py",
            "def test_skip():\n    raise unittest.SkipTest('later')\n    assert 1 == 2\n\n\
             def test_skip_far():\n    raise make(1, 2, 3, kind=unittest.SkipTest)\n    assert 1 == 2\n\n\
             def test_raise():\n    raise ValueError('SkipTes')\n    assert 1 == 2\n\n\
             def test_inner():\n    if flag:\n        return\n    assert 1 == 2\n\n\
             def test_named():\n    returned = 1\n    assert returned == 2\n",
        );
        assert_eq!(py, vec![(1, 1), (1, 1), (0, 0), (1, 1), (1, 1)]);
    }

    /// Dead branches in one test cost steps in proportion to their number. Before #672
    /// every dead range of the file was read for each node of a test, so four times the
    /// branches cost about sixteen times the steps.
    #[test]
    fn dead_branches_cost_steps_in_proportion_to_their_number() {
        let steps_of = |n: usize| -> u64 {
            let body: String = (0..n)
                .map(|i| {
                    format!(
                        "    if False:\n        assert f({i}) == {i}\n    assert g({i}) == {i}\n"
                    )
                })
                .collect();
            let src = format!("def test_t():\n{body}");
            let reg = default_registry();
            let pack = reg.find_pack("tests/test_m.py").unwrap();
            let (facts, counted) = crate::ast::ancestry::steps(|| {
                pack.extract("tests/test_m.py", &src, &AssertVocabulary::default())
            });
            assert_eq!(facts.unwrap().tests[0].total_asserts, n);
            counted
        };
        let (few, many) = (steps_of(40), steps_of(160));
        assert!(many < 5 * few, "{few} steps for 40, {many} for 160");
    }

    #[test]
    fn assertions_under_a_constant_false_branch_or_after_a_terminator_do_not_count() {
        // Rust: dead under `if false`, dead after `return`, live under a real condition and
        // in the `else` branch of a constant-false `if`.
        let rs = counts(
            "src/lib.rs",
            "#[test]\nfn dead_branch() { if false { assert_eq!(1, 2); } }\n\
             #[test]\nfn after_return() { return; assert_eq!(1, 2); }\n\
             #[test]\nfn after_panic() { panic!(\"not yet\"); assert_eq!(1, 2); }\n\
             #[test]\nfn live() { let flag = f(); if flag { assert_eq!(1, 2); } if false { } else { assert_eq!(3, 4); } }\n",
        );
        assert_eq!(rs, vec![(0, 0), (0, 0), (0, 0), (2, 2)]);
        let py = counts(
            "tests/test_a.py",
            "def test_dead_branch():\n    if False:\n        assert 1 == 2\n\n\
             def test_after_fail():\n    pytest.fail('later')\n    assert 1 == 2\n\n\
             def test_after_return():\n    return\n    assert 1 == 2\n\n\
             def test_live():\n    if flag:\n        assert 1 == 2\n    if 0:\n        pass\n    else:\n        assert 3 == 4\n",
        );
        assert_eq!(py, vec![(0, 0), (0, 0), (0, 0), (2, 2)]);
        let js = counts(
            "src/a.test.ts",
            "test('dead', () => { if (false) { expect(1).toBe(2); } return; expect(3).toBe(4); });\n\
             test('live', () => { if (flag) { expect(1).toBe(2); } });\n",
        );
        assert_eq!(js, vec![(0, 0), (1, 1)]);
        let go = counts(
            "pkg/a_test.go",
            "package a\nfunc TestDead(t *testing.T) {\n\tif false {\n\t\tt.Fatal(\"x\")\n\t}\n\treturn\n\tt.Fatal(\"y\")\n}\nfunc TestLive(t *testing.T) {\n\tif flag {\n\t\tt.Fatal(\"x\")\n\t}\n}\n",
        );
        assert_eq!(go, vec![(0, 0), (1, 1)]);
        let java = counts(
            "src/test/java/ATest.java",
            "class ATest {\n  @Test void dead() { if (false) { assertEquals(1, 2); } return; }\n  @Test void live() { if (flag) { assertEquals(1, 2); } }\n}\n",
        );
        assert_eq!(java, vec![(0, 0), (1, 1)]);
        let kt = counts(
            "src/test/kotlin/ATest.kt",
            "class ATest {\n    @Test\n    fun dead() {\n        if (false) {\n            assertEquals(1, 2)\n        }\n        return\n        assertEquals(3, 4)\n    }\n    @Test\n    fun live() {\n        if (flag) {\n            assertEquals(1, 2)\n        }\n    }\n}\n",
        );
        assert_eq!(kt, vec![(0, 0), (1, 1)]);
    }

    /// An assertion in a callback over an empty array literal, or in the body of a loop
    /// over one, runs zero times and does not count (#899). The negative controls: a
    /// literal with an element, a name as the receiver, a method that is not an iteration
    /// method, a Python `for` loop's `else` clause, and a Go array literal (`[2]int{}`
    /// holds two zero values).
    #[test]
    fn assertions_in_an_iteration_over_an_empty_literal_do_not_count() {
        let js = counts(
            "test/a.test.js",
            "test('empty', () => { [].forEach(() => expect(f(2)).toBe(4)); });\n\
             test('empty map', () => { [/* none */].map(function (x) { expect(x).toBe(1); }); });\n\
             test('empty for of', () => { for (const x of []) { expect(x).toBe(1); } });\n\
             test('one', () => { [1].forEach(() => expect(f(2)).toBe(4)); });\n\
             test('name', () => { xs.forEach(() => expect(f(2)).toBe(4)); });\n\
             test('not iteration', () => { [].push(expect(f(2)).toBe(4)); });\n\
             test('for of one', () => { for (const x of [1]) { expect(x).toBe(1); } });\n",
        );
        assert_eq!(
            js,
            vec![(0, 0), (0, 0), (0, 0), (1, 1), (1, 1), (1, 1), (1, 1)]
        );
        let ts = counts(
            "test/a.test.ts",
            "test('empty', () => { [].some((x: number) => { expect(x).toBe(1); return true; }); });\n\
             test('one', () => { [1].some((x: number) => { expect(x).toBe(1); return true; }); });\n",
        );
        assert_eq!(ts, vec![(0, 0), (1, 1)]);
        // A same-file helper called in such a callback runs zero times too.
        let helper = counts(
            "test/b.test.js",
            "function check(x) { expect(x).toBe(4); }\n\
             test('empty', () => { [].forEach(() => check(f(2))); });\n\
             test('dead', () => { if (false) { check(f(2)); } });\n\
             test('one', () => { [1].forEach(() => check(f(2))); });\n",
        );
        assert_eq!(helper, vec![(0, 0), (0, 0), (1, 1)]);
        let py = counts(
            "tests/test_a.py",
            "def test_empty():\n    for _ in []:\n        assert f(2) == 4\n\n\
             def test_empty_tuple():\n    for _ in ():\n        assert f(2) == 4\n\n\
             def test_else():\n    for _ in []:\n        assert f(2) == 4\n    else:\n        assert f(3) == 6\n\n\
             def test_one():\n    for _ in [1]:\n        assert f(2) == 4\n\n\
             def test_name():\n    for _ in xs:\n        assert f(2) == 4\n",
        );
        assert_eq!(py, vec![(0, 0), (0, 0), (1, 1), (1, 1), (1, 1)]);
        let rs = counts(
            "src/lib.rs",
            "#[test]\nfn empty_for() { for _ in [] { assert_eq!(f(2), 4); } }\n\
             #[test]\nfn empty_iter() { [].iter().for_each(|x: &u8| assert_eq!(*x, 4)); }\n\
             #[test]\nfn empty_into_iter() { [].into_iter().map(|x: u8| assert_eq!(x, 4)).count(); }\n\
             #[test]\nfn one_for() { for _ in [1] { assert_eq!(f(2), 4); } }\n\
             #[test]\nfn one_iter() { [1].iter().for_each(|x| assert_eq!(*x, 1)); }\n\
             #[test]\nfn name_iter() { xs.iter().for_each(|x| assert_eq!(*x, 1)); }\n",
        );
        assert_eq!(rs, vec![(0, 0), (0, 0), (0, 0), (1, 1), (1, 1), (1, 1)]);
        let go = counts(
            "pkg/a_test.go",
            "package a\nfunc TestEmptySlice(t *testing.T) {\n\tfor range []int{} {\n\t\tt.Fatal(\"x\")\n\t}\n}\n\
             func TestEmptyMap(t *testing.T) {\n\tfor k := range map[string]int{} {\n\t\tt.Fatal(k)\n\t}\n}\n\
             func TestOne(t *testing.T) {\n\tfor range []int{1} {\n\t\tt.Fatal(\"x\")\n\t}\n}\n\
             func TestArray(t *testing.T) {\n\tfor range [2]int{} {\n\t\tt.Fatal(\"x\")\n\t}\n}\n\
             func TestName(t *testing.T) {\n\tfor range xs {\n\t\tt.Fatal(\"x\")\n\t}\n}\n",
        );
        assert_eq!(go, vec![(0, 0), (0, 0), (1, 1), (1, 1), (1, 1)]);
    }
}
