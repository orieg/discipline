//! Assertions whose failure is caught inside the test function:
//! - Python `try: assert ... except AssertionError: pass` (or `except Exception`,
//!   `except BaseException`, bare `except:`, `except*`), and
//!   `with contextlib.suppress(AssertionError):`
//! - Rust `let _ = std::panic::catch_unwind(|| assert_eq!(...))`
//! - JS / TS `try { expect(...) } catch {}` and `<promise>.catch(() => {})`
//! - Java `try { assertEquals(...) } catch (AssertionError | Throwable e) {}`
//! - Kotlin `try { assertEquals(...) } catch (e: AssertionError) {}` and
//!   `runCatching { assertEquals(...) }` with the result unused
//! - C# `try { Assert.Equal(...) } catch (Exception) {}`
//! - Go `defer func() { recover() }()` before a check that panics
//! - a `finally` block that returns (Java, Kotlin, JavaScript / TypeScript, Python), which
//!   discards the failure whatever the handlers do
//!
//! An assertion wrapped in a handler that catches its failure and swallows it (neither re-raising
//! nor failing the test nor asserting) cannot fail the test. It drops out of `effective_asserts()`.
//!
//! Whether the handler catches the failure is read from the type it names (`Reach`): the
//! language's assertion failure type and its ancestors always catch it, a list of standard
//! classes that an assertion failure is not an instance of never do, and a type on neither
//! list may. A handler that may catch it and swallows is reported: a class of the project
//! under review can extend the failure type, and passing it would leave a way to hide a
//! failure behind a new class name. In Python a class the file declares is read through
//! its bases, and a handler that may catch the failure is reported only when its body is
//! nothing but `pass`, `continue` or a logging call.
//!
//! An assertion is what the pack counts as one by name, and a call to a helper the
//! configuration lists (`assert_helper_fns`, and `extra_assert_macros` for a Rust macro).
//!
//! An assertion in a function passed as an argument inside the `try` is read only when the
//! function it is passed to is known to run it before returning (`SYNC_CALLBACKS`):
//! whether any other callback runs before the `try` ends is not known here.
//!
//! Go is read differently. `t.Fatal`, `t.Error` and the testify `require` and `assert`
//! functions do not panic: `t.FailNow` ends the test through `runtime.Goexit`, which a
//! `recover()` does not stop. So a deferred `recover()` catches only a check that panics:
//! a `panic(..)` in the test, or a call to a function of the file that panics.

use super::ancestry::Ancestry;
use super::bounds::{text, walk};
use super::{helper_call_matches, AssertVocabulary, TestFn};
use tree_sitter::Node;

/// One assertion whose failure is caught inside the test by an enclosing handler.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CaughtAssertion {
    /// 1-based line of the assertion.
    pub line: usize,
    /// 1-based line of the enclosing handler that catches it.
    pub handler_line: usize,
    /// Description of the catching construct.
    pub detail: String,
    /// Byte range of the assertion: what makes two assertions on one line two.
    pub span: (usize, usize),
    /// Whether the assertion is already out of the effective count, so that being
    /// swallowed does not take it out again: the pack also counts it as a tautology
    /// (`TestFn::tautology_spans`), or does not count it at all (a `panic(..)` written
    /// in a Go test).
    pub tautology: bool,
}

/// One assertion node: its line and its byte range.
type Site = (usize, (usize, usize));

fn site(node: Node) -> Site {
    (
        node.start_position().row + 1,
        (node.start_byte(), node.end_byte()),
    )
}

/// The tests of one source, and the assertions a reader has recorded in them as caught.
///
/// An assertion goes to the innermost test that holds its line, once: one that overlaps
/// an assertion the test holds is not added. Reading every test for the first and every
/// assertion of the test for the second cost the tests and the assertions for each
/// assertion. The test is answered from a table of lines, and the overlap from the
/// assertions kept in the order of their bytes.
struct Caught<'a> {
    tests: &'a mut [TestFn],
    /// The last line of the source.
    last_line: usize,
    /// The innermost test of each line (`innermost_tests_by_line`), made when the first
    /// assertion is placed.
    innermost: Option<Vec<Option<usize>>>,
    /// For a test, by its index: what it holds.
    held: std::collections::HashMap<usize, Held>,
}

/// The byte ranges one test holds.
struct Held {
    /// The assertions recorded as caught, in the order of their bytes. No two of them
    /// overlap, so the one that starts last before a given byte is also the one that
    /// ends last. `None` for a test that held assertions before this reader ran: those
    /// are compared one by one, as they were.
    caught: Option<std::collections::BTreeSet<(usize, usize)>>,
    /// The tautologies, by the byte each starts at, and for each the furthest byte that
    /// one or one before it ends at.
    tautologies: Vec<(usize, usize)>,
}

/// Whether two byte ranges overlap. A range of no width overlaps a range it is strictly
/// inside, and no other: not itself.
fn overlap(a: (usize, usize), b: (usize, usize)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

impl Held {
    fn of(test: &TestFn) -> Self {
        let mut tautologies = test.tautology_spans.clone();
        tautologies.sort_unstable();
        let mut furthest = 0;
        for span in &mut tautologies {
            furthest = furthest.max(span.1);
            span.1 = furthest;
        }
        super::ancestry::count(tautologies.len());
        Self {
            caught: test
                .caught_assertions
                .is_empty()
                .then(std::collections::BTreeSet::new),
            tautologies,
        }
    }

    /// Whether an assertion the test holds overlaps `span`.
    fn holds(&self, test: &TestFn, span: (usize, usize)) -> bool {
        match &self.caught {
            // The last one that starts before `span` ends, which ends last of them.
            Some(caught) => caught
                .range(..(span.1, 0))
                .next_back()
                .is_some_and(|last| span.0 < last.1),
            None => {
                super::ancestry::count(test.caught_assertions.len());
                test.caught_assertions
                    .iter()
                    .any(|existing| overlap(existing.span, span))
            }
        }
    }

    /// Whether a tautology of the test overlaps `span`.
    fn is_tautology(&self, span: (usize, usize)) -> bool {
        // Of the tautologies that start before `span` ends, the one that ends last.
        let before = self.tautologies.partition_point(|t| t.0 < span.1);
        before
            .checked_sub(1)
            .is_some_and(|last| span.0 < self.tautologies[last].1)
    }
}

impl<'a> Caught<'a> {
    /// The caught assertions of `tests`, which are the tests of the tree of `root`.
    fn new(tests: &'a mut [TestFn], root: Node) -> Self {
        Self {
            tests,
            last_line: root.end_position().row + 1,
            innermost: None,
            held: Default::default(),
        }
    }

    fn attribute(&mut self, mut c: CaughtAssertion) {
        super::ancestry::count(1);
        let innermost = self
            .innermost
            .get_or_insert_with(|| super::innermost_tests_by_line(self.tests, self.last_line));
        let Some(&Some(at)) = innermost.get(c.line) else {
            return;
        };
        let t = &mut self.tests[at];
        let held = self.held.entry(at).or_insert_with(|| Held::of(t));
        // The assertions read to answer: as many as halving their number takes.
        super::ancestry::count((usize::BITS - t.caught_assertions.len().leading_zeros()) as usize);
        // One assertion is neutralized once, whatever number of handlers enclose it.
        // The assertion is its node, so two on one line are two; a node inside
        // another (`expect(x)` in `expect(x).toBe(1)`) is the same assertion.
        if held.holds(t, c.span) {
            return;
        }
        c.tautology = c.tautology || held.is_tautology(c.span);
        if let Some(caught) = &mut held.caught {
            caught.insert(c.span);
        }
        t.caught_assertions.push(c);
    }
}

/// The head test's swallowed assertions that its base counterpart did not already have.
///
/// A swallowed assertion is identified by where it sits inside its own test, never by its
/// line in the file: a file line moves whenever anything above the test is edited, and a
/// test paired across a move or a rename starts on another line altogether (#526).
///
/// Each head assertion is paired with a base one at the same offsets from the start of the
/// test (assertion and handler). The ones left, which an edit inside the test may have
/// moved, are paired with a remaining base one of the same construct at the same distance
/// from its handler. What stays unpaired is new.
///
/// The count is deliberately not a shortcut: a test that stops swallowing one assertion
/// and starts swallowing another swallows as many as before, and the second is new.
///
/// The limits: an edit that changes the distance between a swallowed assertion and its
/// handler reports that assertion as new, and when a new one has the shape of an earlier
/// one that moved, the line named may be the earlier one's.
pub fn newly_caught<'a>(base: &TestFn, head: &'a TestFn) -> Vec<&'a CaughtAssertion> {
    let offsets = |t: &TestFn, c: &CaughtAssertion| {
        (
            c.line as i64 - t.line as i64,
            c.handler_line as i64 - t.line as i64,
        )
    };
    let shape = |c: &CaughtAssertion| (c.line as i64 - c.handler_line as i64, c.detail.clone());

    // Each base assertion pairs with at most one head assertion.
    let mut free: Vec<&CaughtAssertion> = base.caught_assertions.iter().collect();
    let mut unpaired: Vec<&CaughtAssertion> = Vec::new();
    for hc in &head.caught_assertions {
        match free
            .iter()
            .position(|bc| offsets(base, bc) == offsets(head, hc))
        {
            Some(i) => {
                free.remove(i);
            }
            None => unpaired.push(hc),
        }
    }
    let mut new = Vec::new();
    for hc in unpaired {
        match free.iter().position(|bc| shape(bc) == shape(hc)) {
            Some(i) => {
                free.remove(i);
            }
            None => new.push(hc),
        }
    }
    new
}

/// Whether `callee` names a helper the configuration lists as an assertion
/// (`assert_helper_fns`), by its whole text or by its last `::` / `.` segment, as the
/// counters match it.
fn configured(callee: &str, vocab: &AssertVocabulary) -> bool {
    !callee.is_empty()
        && vocab
            .helper_fns
            .iter()
            .any(|h| helper_call_matches(callee.trim(), h))
}

/// Per language, the functions known to run a function they are passed before they
/// return: an assertion in such a callback fails inside the `try` that holds the call.
/// `docs/GATES.md` lists them.
pub const SYNC_CALLBACKS: &[(&str, &[&str])] = &[
    (
        "JavaScript / TypeScript",
        &[
            "forEach", "map", "filter", "some", "every", "reduce", "find",
        ],
    ),
    ("Java", &["forEach"]),
    (
        "Kotlin",
        &["let", "run", "apply", "also", "with", "forEach", "use"],
    ),
    ("C#", &["ForEach"]),
];

fn sync_callbacks(language: &str) -> &'static [&'static str] {
    SYNC_CALLBACKS
        .iter()
        .find(|(l, _)| *l == language)
        .map_or(&[], |(_, names)| names)
}

/// `walk`, for a visitor that keeps nodes it was handed.
fn walk_tree<'a>(root: Node<'a>, f: &mut dyn FnMut(Node<'a>) -> bool) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        super::ancestry::count(1);
        if !f(node) {
            continue;
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
}

/// The description of a `finally` block that returns.
const FINALLY_RETURNS: &str = "finally block returns, discarding the failure";

fn find_child_by_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let res = node
        .children(&mut cursor)
        .find(|child| child.kind() == kind);
    res
}

/// Whether `inner` lies within `outer`.
fn within(inner: Node, outer: Node) -> bool {
    inner.start_byte() >= outer.start_byte() && inner.end_byte() <= outer.end_byte()
}

/// The text of the last identifier-like leaf of a type or name node: `IOException` of
/// `java.io.IOException`, `XunitException` of `Xunit.Sdk.XunitException`.
fn simple_name<'a>(node: Node, src: &'a str) -> &'a str {
    // The last named leaf: the first one a walk from the end of the node meets. A walk
    // from its start reads the whole of a chain of calls for each call of the chain.
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.child_count() == 0 {
            if n.is_named() {
                return text(n, src);
            }
            continue;
        }
        let mut cursor = n.walk();
        stack.extend(n.children(&mut cursor));
    }
    ""
}

/// Whether an assertion failure reaches a handler, judged by the type the handler names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reach {
    /// The type is a known class that an assertion failure is not an instance of.
    Never,
    /// The type is not known: a class of the project, or a subclass of the failure type.
    /// It may be an assertion failure type, so a handler for it that swallows is reported.
    Maybe,
    /// The type is the language's assertion failure type or one of its ancestors.
    Always,
}

/// The reach of a handler that names `names` (a multi-catch names several).
fn reach(names: &[&str], ancestors: &[&str], unrelated: &[&str]) -> Reach {
    if names.iter().any(|n| ancestors.contains(n)) {
        Reach::Always
    } else if names.iter().all(|n| unrelated.contains(n)) {
        Reach::Never
    } else {
        Reach::Maybe
    }
}

/// The handler of a `try` that swallows an assertion failure, reading the handlers in
/// order as the language does: one that always catches the failure decides (it swallows,
/// or it does not and no later handler sees the failure); one that may catch it is the
/// answer when it swallows, and is passed over when it does not.
fn swallowing_handler<'a>(handlers: &[(Node<'a>, Reach, bool)]) -> Option<Node<'a>> {
    for &(handler, reach, swallows) in handlers {
        match reach {
            Reach::Never => {}
            Reach::Always => return swallows.then_some(handler),
            Reach::Maybe => {
                if swallows {
                    return Some(handler);
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Python
// ---------------------------------------------------------------------------

/// `AssertionError` and its ancestors.
const PY_CATCHING: &[&str] = &["AssertionError", "Exception", "BaseException"];

/// Exception classes of the standard library outside the built-in hierarchy (`json`,
/// `subprocess`, `urllib.error`, `asyncio`, `zipfile`, `io`, `pickle`, `statistics`;
/// library reference, each module's "Exceptions"). None derives from `AssertionError`.
const PY_STDLIB_UNRELATED: &[&str] = &[
    "JSONDecodeError",
    "SubprocessError",
    "CalledProcessError",
    "TimeoutExpired",
    "URLError",
    "HTTPError",
    "CancelledError",
    "InvalidStateError",
    "IncompleteReadError",
    "BadZipFile",
    "UnsupportedOperation",
    "PickleError",
    "PicklingError",
    "UnpicklingError",
    "StatisticsError",
];

/// The classes a file declares, each with the names of its bases (last segments).
type PyClasses<'a> = std::collections::HashMap<&'a str, Vec<&'a str>>;

fn py_classes<'a>(root: Node, src: &'a str) -> PyClasses<'a> {
    let mut classes = PyClasses::new();
    walk(root, &mut |n| {
        if n.kind() == "class_definition" {
            if let Some(name) = n.child_by_field_name("name") {
                let mut bases = Vec::new();
                if let Some(supers) = n.child_by_field_name("superclasses") {
                    let mut c = supers.walk();
                    for base in supers.named_children(&mut c) {
                        match base.kind() {
                            // `metaclass=...` is not a base.
                            "keyword_argument" | "comment" => {}
                            "identifier" | "attribute" => py_type_names(base, src, &mut bases),
                            // A base that is computed is not known.
                            _ => bases.push(""),
                        }
                    }
                }
                classes.insert(text(name, src), bases);
            }
        }
        true
    });
    classes
}

/// Whether `name` is a class of Python's standard exception hierarchy (the table the
/// expected-exception comparison uses), or one of the `OSError` aliases.
fn python_standard_exception(name: &str) -> bool {
    matches!(name, "IOError" | "EnvironmentError" | "WindowsError")
        || super::exception_tables::PYTHON
            .iter()
            .any(|(class, parent)| *class == name || *parent == name)
}

/// Whether an `AssertionError` (or a failure type a test class substitutes for it)
/// reaches a handler for the class `name`.
///
/// `AssertionError` and its ancestors always catch it. A class the file declares is read
/// through its bases: it never catches it when every base is a standard class beside
/// `AssertionError` or a declared class that never does, and may otherwise. A standard
/// class beside `AssertionError` never catches it. Any other name is a class of the
/// project or of a library, which may derive from `AssertionError`.
fn py_name_reach(name: &str, classes: &PyClasses, seen: &mut Vec<String>) -> Reach {
    if let Some(bases) = classes.get(name) {
        if seen.iter().any(|s| s == name) {
            return Reach::Maybe;
        }
        seen.push(name.to_string());
        let unrelated = bases.iter().all(|base| {
            // Deriving from an ancestor of `AssertionError` does not make a subclass of it.
            (!classes.contains_key(base) && matches!(*base, "Exception" | "BaseException"))
                || py_name_reach(base, classes, seen) == Reach::Never
        });
        seen.pop();
        return if unrelated {
            Reach::Never
        } else {
            Reach::Maybe
        };
    }
    if PY_CATCHING.contains(&name) {
        Reach::Always
    } else if python_standard_exception(name) || PY_STDLIB_UNRELATED.contains(&name) {
        Reach::Never
    } else {
        Reach::Maybe
    }
}

/// The reach of a handler that names `names`: as far as its widest class.
fn py_reach(names: &[&str], classes: &PyClasses) -> Reach {
    let each: Vec<Reach> = names
        .iter()
        .map(|n| py_name_reach(n, classes, &mut Vec::new()))
        .collect();
    if each.contains(&Reach::Always) {
        Reach::Always
    } else if each.iter().all(|r| *r == Reach::Never) {
        Reach::Never
    } else {
        Reach::Maybe
    }
}

/// The class names an `except` clause or a `suppress(...)` call lists, each by its last
/// segment (`builtins.AssertionError` is `AssertionError`).
fn py_type_names<'a>(expr: Node, src: &'a str, out: &mut Vec<&'a str>) {
    match expr.kind() {
        "identifier" => out.push(text(expr, src)),
        "attribute" => {
            if let Some(a) = expr.child_by_field_name("attribute") {
                out.push(text(a, src));
            }
        }
        "tuple" | "parenthesized_expression" | "list" => {
            let mut c = expr.walk();
            for child in expr.named_children(&mut c) {
                py_type_names(child, src, out);
            }
        }
        "as_pattern" => {
            if let Some(first) = expr.named_child(0) {
                py_type_names(first, src, out);
            }
        }
        _ => {}
    }
}

/// The expressions between `except` and `:` (none for a bare `except:`).
fn py_except_exprs(clause: Node) -> Vec<Node> {
    let mut c = clause.walk();
    clause
        .named_children(&mut c)
        .filter(|n| !matches!(n.kind(), "block" | "comment"))
        .collect()
}

fn py_except_reach(clause: Node, src: &str, classes: &PyClasses) -> Reach {
    let exprs = py_except_exprs(clause);
    if exprs.is_empty() {
        // Bare `except:` catches BaseException, which includes AssertionError.
        return Reach::Always;
    }
    let mut names = Vec::new();
    for e in exprs {
        py_type_names(e, src, &mut names);
    }
    if names.is_empty() {
        // The classes are computed (`except errors():`): not known.
        return Reach::Maybe;
    }
    py_reach(&names, classes)
}

/// The name the clause binds the caught error to (`except AssertionError as e`).
fn py_except_alias<'a>(clause: Node, src: &'a str) -> Option<&'a str> {
    py_except_exprs(clause)
        .into_iter()
        .find(|e| e.kind() == "as_pattern")
        .and_then(|e| e.child_by_field_name("alias"))
        .map(|a| text(a, src).trim())
}

fn py_is_fail_call(call: Node, src: &str) -> bool {
    let callee = call
        .child_by_field_name("function")
        .map_or("", |f| text(f, src));
    callee == "pytest.fail"
        || callee == "self.fail"
        || callee == "fail"
        || callee.ends_with(".fail")
}

/// Whether the subtree raises, asserts or calls a function that fails the test.
fn py_fails(node: Node, src: &str, vocab: &AssertVocabulary) -> bool {
    let mut fails = false;
    walk(node, &mut |n| {
        if fails
            || matches!(
                n.kind(),
                "function_definition" | "class_definition" | "lambda"
            )
        {
            return false;
        }
        match n.kind() {
            "raise_statement" | "assert_statement" => fails = true,
            "call" => {
                let callee = n
                    .child_by_field_name("function")
                    .map_or("", |f| text(f, src));
                if callee.starts_with("self.assert")
                    || callee.starts_with("assert_")
                    || py_is_fail_call(n, src)
                    || configured(callee, vocab)
                {
                    fails = true;
                }
            }
            _ => {}
        }
        !fails
    });
    fails
}

fn py_except_body_swallows(clause: Node, src: &str, vocab: &AssertVocabulary) -> bool {
    let Some(body) = clause
        .child_by_field_name("body")
        .or_else(|| find_child_by_kind(clause, "block"))
    else {
        return true;
    };
    !py_fails(body, src, vocab)
}

/// Methods a logger is called by.
const PY_LOG_METHODS: &[&str] = &[
    "debug",
    "info",
    "warning",
    "warn",
    "error",
    "exception",
    "critical",
    "fatal",
    "log",
];

/// A call that only reports: `print(..)`, `warnings.warn(..)`, or a logging method called
/// on something named for a log: `log`, `logging`, or a name ending in `logger` or `_log`
/// (`logger`, `self.log`, `LOG`, `app_logger`), whatever its case.
fn py_is_log_call(call: Node, src: &str) -> bool {
    let Some(function) = call.child_by_field_name("function") else {
        return false;
    };
    match function.kind() {
        "identifier" => text(function, src) == "print",
        "attribute" => {
            let method = function
                .child_by_field_name("attribute")
                .map_or("", |a| text(a, src));
            let object = function
                .child_by_field_name("object")
                .map_or("", |o| simple_name(o, src));
            let named = object.trim_start_matches('_').to_lowercase();
            (object == "warnings" && method == "warn")
                || (PY_LOG_METHODS.contains(&method)
                    && (named == "log"
                        || named == "logging"
                        || named.ends_with("logger")
                        || named.ends_with("_log")))
        }
        _ => false,
    }
}

/// Whether the handler does nothing with the failure: every statement of its body is
/// `pass`, `...`, `continue` or a logging call.
fn py_except_body_only_swallows(clause: Node, src: &str) -> bool {
    let Some(body) = clause
        .child_by_field_name("body")
        .or_else(|| find_child_by_kind(clause, "block"))
    else {
        return true;
    };
    let mut c = body.walk();
    let only = body.named_children(&mut c).all(|stmt| match stmt.kind() {
        "pass_statement" | "continue_statement" | "comment" => true,
        "expression_statement" => stmt.named_child(0).is_some_and(|e| {
            stmt.named_child_count() == 1
                && (e.kind() == "ellipsis" || (e.kind() == "call" && py_is_log_call(e, src)))
        }),
        _ => false,
    });
    only
}

/// The `finally` clause of a `try` whose block returns: the `return` replaces whatever
/// the `try` raised, so the failure is discarded.
fn py_finally_returns(try_stmt: Node) -> Option<Node> {
    let clause = find_child_by_kind(try_stmt, "finally_clause")?;
    let block = find_child_by_kind(clause, "block")?;
    find_child_by_kind(block, "return_statement").map(|_| clause)
}

/// Whether the subtree names `name` (an identifier, or an attribute such as `self.errors`).
fn py_mentions(node: Node, name: &str, src: &str) -> bool {
    let mut found = false;
    walk(node, &mut |n| {
        if !found && matches!(n.kind(), "identifier" | "attribute") && text(n, src) == name {
            found = true;
        }
        !found
    });
    found
}

/// Soft assertions: the handler keeps the caught error (`errors.append(e)`, `last = e`)
/// and a statement after the `try` that names what it was kept in raises, asserts or
/// fails the test.
fn py_error_kept_and_checked<'t>(
    try_stmt: Node<'t>,
    anc: &Ancestry<'t>,
    clause: Node,
    src: &str,
    vocab: &AssertVocabulary,
) -> bool {
    let Some(alias) = py_except_alias(clause, src) else {
        return false;
    };
    let Some(body) = find_child_by_kind(clause, "block") else {
        return false;
    };
    let mut kept_in: Vec<&str> = Vec::new();
    walk(body, &mut |n| {
        match n.kind() {
            "function_definition" | "class_definition" | "lambda" => return false,
            "call" => {
                let function = n.child_by_field_name("function");
                let arguments = n.child_by_field_name("arguments");
                if let (Some(f), Some(args)) = (function, arguments) {
                    let method = f
                        .child_by_field_name("attribute")
                        .map_or("", |a| text(a, src));
                    if f.kind() == "attribute"
                        && matches!(method, "append" | "add" | "extend" | "insert")
                        && py_mentions(args, alias, src)
                    {
                        if let Some(object) = f.child_by_field_name("object") {
                            kept_in.push(text(object, src));
                        }
                    }
                }
            }
            "assignment" | "augmented_assignment" => {
                let left = n.child_by_field_name("left");
                let right = n.child_by_field_name("right");
                if let (Some(l), Some(r)) = (left, right) {
                    if py_mentions(r, alias, src) {
                        let target = if l.kind() == "subscript" {
                            l.child_by_field_name("value").unwrap_or(l)
                        } else {
                            l
                        };
                        kept_in.push(text(target, src));
                    }
                }
            }
            _ => {}
        }
        true
    });
    if kept_in.is_empty() {
        return false;
    }
    // A name the error is kept in twice is asked about once.
    kept_in.sort_unstable();
    kept_in.dedup();
    let mut scope = try_stmt;
    while let Some(p) = anc.parent(scope) {
        if matches!(scope.kind(), "function_definition" | "lambda") {
            break;
        }
        scope = p;
    }
    let mut checked = false;
    walk(scope, &mut |n| {
        if checked || n.end_byte() <= try_stmt.end_byte() {
            return false;
        }
        if n.start_byte() >= try_stmt.end_byte()
            && n.kind().ends_with("_statement")
            && kept_in.iter().any(|k| py_mentions(n, k, src))
            && py_fails(n, src, vocab)
        {
            checked = true;
        }
        !checked
    });
    checked
}

/// A statement that fails the test whenever it runs: `raise`, a call that fails the test,
/// or `assert False`.
fn py_always_fails(stmt: Node, src: &str) -> bool {
    match stmt.kind() {
        "raise_statement" => true,
        "assert_statement" => stmt.named_child(0).is_some_and(|c| c.kind() == "false"),
        "expression_statement" => stmt
            .named_child(0)
            .is_some_and(|c| c.kind() == "call" && py_is_fail_call(c, src)),
        _ => false,
    }
}

/// A retry loop: the `try` sits in a loop, leaves it when the assertion holds (`break` or
/// `return` in the `try` body or its `else`), and the test fails once the loop runs out:
/// the loop's `else` raises or fails, or, when success returns, a statement after the
/// loop always fails.
fn py_retry_fails_after_last_attempt<'t>(
    try_stmt: Node<'t>,
    anc: &Ancestry<'t>,
    src: &str,
    vocab: &AssertVocabulary,
) -> bool {
    let mut enclosing_loop = None;
    let mut cur = anc.parent(try_stmt);
    while let Some(p) = cur {
        match p.kind() {
            "for_statement" | "while_statement" => {
                enclosing_loop = Some(p);
                break;
            }
            "function_definition" | "class_definition" | "lambda" => break,
            _ => {}
        }
        cur = anc.parent(p);
    }
    let Some(lp) = enclosing_loop else {
        return false;
    };
    let (mut breaks, mut returns) = (false, false);
    let mut c = try_stmt.walk();
    for part in try_stmt
        .children(&mut c)
        .filter(|n| matches!(n.kind(), "block" | "else_clause"))
    {
        walk(part, &mut |n| {
            match n.kind() {
                "function_definition"
                | "class_definition"
                | "lambda"
                | "for_statement"
                | "while_statement" => return false,
                "break_statement" => breaks = true,
                "return_statement" => returns = true,
                _ => {}
            }
            true
        });
    }
    let else_fails = lp
        .child_by_field_name("alternative")
        .is_some_and(|e| py_fails(e, src, vocab));
    if (breaks || returns) && else_fails {
        return true;
    }
    if returns && !breaks {
        let mut sibling = anc.next_named_sibling(lp);
        while let Some(s) = sibling {
            if py_always_fails(s, src) {
                return true;
            }
            sibling = anc.next_named_sibling(s);
        }
    }
    false
}

/// How far the `suppress(...)` items of a `with` statement (`contextlib.suppress`) reach:
/// `Never` when it has none, or none that lists a class an assertion failure may be.
fn py_with_suppress_reach(with_stmt: Node, src: &str, classes: &PyClasses) -> Reach {
    let mut suppresses = Reach::Never;
    let Some(clause) = find_child_by_kind(with_stmt, "with_clause") else {
        return Reach::Never;
    };
    walk(clause, &mut |n| {
        if suppresses == Reach::Always {
            return false;
        }
        if n.kind() != "call" {
            // Items, and the `as` pattern or parentheses around a call.
            return matches!(
                n.kind(),
                "with_clause" | "with_item" | "as_pattern" | "parenthesized_expression" | "tuple"
            );
        }
        let Some(function) = n.child_by_field_name("function") else {
            return false;
        };
        let callee = match function.kind() {
            "identifier" => text(function, src),
            "attribute" => function
                .child_by_field_name("attribute")
                .map_or("", |a| text(a, src)),
            _ => "",
        };
        if callee == "suppress" {
            let mut names = Vec::new();
            if let Some(args) = n.child_by_field_name("arguments") {
                let mut c = args.walk();
                for arg in args.named_children(&mut c) {
                    py_type_names(arg, src, &mut names);
                }
            }
            let reach = py_reach(&names, classes);
            if reach != Reach::Never && suppresses != Reach::Always {
                suppresses = reach;
            }
        }
        false
    });
    suppresses
}

pub fn python<'t>(
    root: Node<'t>,
    anc: &Ancestry<'t>,
    src: &str,
    tests: &mut [TestFn],
    vocab: &AssertVocabulary,
) {
    if tests.is_empty() {
        return;
    }
    let mut caught = Caught::new(tests, root);
    let classes = py_classes(root, src);
    // For a `try`, by its id: each handler, whether it is reached, whether it swallows.
    let mut handlers_of: std::collections::HashMap<usize, Vec<(Node, Reach, bool)>> =
        std::collections::HashMap::new();
    walk(root, &mut |node| {
        let is_assertion = match node.kind() {
            "assert_statement" => true,
            "call" => {
                let callee = node
                    .child_by_field_name("function")
                    .map_or("", |f| text(f, src));
                (callee.starts_with("self.assert")
                    && !callee.contains("assertRaises")
                    && !callee.contains("assertWarns")
                    && !callee.contains("assertLogs"))
                    || configured(callee, vocab)
            }
            _ => false,
        };
        if !is_assertion {
            return true;
        }

        // Walk up to the nearest enclosing handler that swallows the failure.
        let mut cur = anc.parent(node);
        while let Some(p) = cur {
            if matches!(p.kind(), "function_definition" | "class_definition") {
                break;
            }
            if p.kind() == "with_statement" {
                let in_body = p
                    .child_by_field_name("body")
                    .is_some_and(|b| within(node, b));
                let reach = if in_body {
                    py_with_suppress_reach(p, src, &classes)
                } else {
                    Reach::Never
                };
                if reach != Reach::Never {
                    caught.attribute(CaughtAssertion {
                        line: node.start_position().row + 1,
                        span: site(node).1,
                        tautology: false,
                        handler_line: p.start_position().row + 1,
                        detail: if reach == Reach::Always {
                            "contextlib.suppress discards the AssertionError"
                        } else {
                            "contextlib.suppress of a class that may be an assertion failure"
                        }
                        .to_string(),
                    });
                    break;
                }
            }
            if p.kind() == "try_statement" {
                let inside_try_body = p
                    .child_by_field_name("body")
                    .is_some_and(|b| within(node, b));
                if inside_try_body {
                    if let Some(finally) = py_finally_returns(p) {
                        caught.attribute(CaughtAssertion {
                            line: node.start_position().row + 1,
                            span: site(node).1,
                            tautology: false,
                            handler_line: finally.start_position().row + 1,
                            detail: FINALLY_RETURNS.to_string(),
                        });
                        break;
                    }
                    // What the handlers of a `try` do is the same for every assertion
                    // in its body: it is read once for the `try`.
                    let handlers = handlers_of.entry(p.id()).or_insert_with(|| {
                        let mut cursor = p.walk();
                        p.children(&mut cursor)
                            .filter(|c| c.kind() == "except_clause")
                            .map(|c| {
                                let reach = py_except_reach(c, src, &classes);
                                let swallows =
                                    match reach {
                                        Reach::Never => false,
                                        Reach::Always => py_except_body_swallows(c, src, vocab),
                                        // A class that may be an assertion failure: reported
                                        // only when the handler plainly does nothing with it.
                                        Reach::Maybe => py_except_body_only_swallows(c, src),
                                    } && !py_error_kept_and_checked(p, anc, c, src, vocab)
                                        && !py_retry_fails_after_last_attempt(p, anc, src, vocab);
                                (c, reach, swallows)
                            })
                            .collect()
                    });
                    if let Some(clause) = swallowing_handler(handlers) {
                        let always = handlers
                            .iter()
                            .any(|(c, reach, _)| *c == clause && *reach == Reach::Always);
                        caught.attribute(CaughtAssertion {
                            line: node.start_position().row + 1,
                            span: site(node).1,
                            tautology: false,
                            handler_line: clause.start_position().row + 1,
                            detail: if always {
                                "AssertionError caught without re-raise or test failure"
                            } else {
                                "a class that may be an assertion failure caught and discarded"
                            }
                            .to_string(),
                        });
                        break;
                    }
                }
            }
            cur = anc.parent(p);
        }
        true
    });
}

// ---------------------------------------------------------------------------
// Rust
// ---------------------------------------------------------------------------

const RS_ASSERT_MACROS: &[&str] = &[
    "assert",
    "assert_eq",
    "assert_ne",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
    "assert_matches",
];

/// Macros that end the test when they run.
const RS_PANIC_MACROS: &[&str] = &["panic", "unreachable", "todo", "unimplemented"];

/// Methods that panic on one side of a `Result`: calling one looks at the outcome.
const RS_CHECKING_METHODS: &[&str] = &["unwrap", "expect", "unwrap_err", "expect_err"];

fn rs_macro_name<'a>(invocation: Node, src: &'a str) -> &'a str {
    let name = invocation
        .child_by_field_name("macro")
        .map_or("", |m| text(m, src));
    name.rsplit("::").next().unwrap_or(name)
}

/// Whether `n` is what the Rust reader counts as an assertion in a closure
/// `catch_unwind` runs: an asserting macro, `unwrap` / `expect`, or a helper the
/// configuration lists.
fn rs_is_assertion(n: Node, src: &str, vocab: &AssertVocabulary) -> bool {
    if n.kind() == "macro_invocation" {
        let name = rs_macro_name(n, src);
        return RS_ASSERT_MACROS.contains(&name) || vocab.extra_macros.iter().any(|m| m == name);
    }
    if n.kind() != "call_expression" {
        return false;
    }
    let Some(f) = n.child_by_field_name("function") else {
        return false;
    };
    if f.kind() == "field_expression" {
        f.child_by_field_name("field").is_some_and(|field| {
            let method = text(field, src);
            method == "unwrap" || method == "expect" || configured(method, vocab)
        })
    } else {
        configured(text(f, src), vocab)
    }
}

/// The assertions under `closure`, outside any function declared in it.
fn rs_closure_contains_assert(closure: Node, src: &str, vocab: &AssertVocabulary) -> Vec<Site> {
    let mut asserts = Vec::new();
    walk(closure, &mut |n| {
        if n.kind() == "function_item" {
            return false;
        }
        if rs_is_assertion(n, src, vocab) {
            asserts.push(site(n));
        }
        true
    });
    asserts
}

/// The assertions of one Rust tree, by the function each is written in, for the
/// closures `catch_unwind` runs.
///
/// The assertions of a closure are the ones under it, outside any function declared in
/// it. Walking the closure for each `catch_unwind` costs the closure for each, and a
/// closure that holds another `catch_unwind` is walked again for that one: 640 of them
/// one inside another, in a file of 44 kB, cost 6.5e10 instructions. The assertions are
/// listed once, each under the function that holds it, and a closure asks for the run of
/// them between where it begins and where it ends.
struct RsAsserts<'t> {
    root: Node<'t>,
    listed: std::cell::OnceCell<RsAssertsListed>,
}

struct RsAssertsListed {
    /// For the root and each function, by its id: the assertions written in it and in no
    /// function inside it, each with its place among all the assertions of the tree.
    held: std::collections::HashMap<usize, Vec<(usize, Site)>>,
    /// For each closure, by its id: the function that holds it, and the places of the
    /// assertions of the tree that are under it.
    closures: std::collections::HashMap<usize, RsRun>,
}

/// The function that holds a closure, by its id, and the places of the assertions of
/// the tree that are under the closure: the first, and one past the last.
type RsRun = (usize, usize, usize);

impl<'t> RsAsserts<'t> {
    fn new(root: Node<'t>) -> Self {
        Self {
            root,
            listed: std::cell::OnceCell::new(),
        }
    }

    fn listed(&self, src: &str, vocab: &AssertVocabulary) -> &RsAssertsListed {
        self.listed.get_or_init(|| {
            let mut held: std::collections::HashMap<usize, Vec<(usize, Site)>> = Default::default();
            let mut closures: std::collections::HashMap<usize, RsRun> = Default::default();
            // The functions around the node being read, the root first.
            let mut holders = vec![self.root.id()];
            let mut seen = 0usize;
            // A node, and whether it was entered already and is now left.
            let mut stack: Vec<(Node, bool)> = vec![(self.root, false)];
            while let Some((n, left)) = stack.pop() {
                super::ancestry::count(1);
                if left {
                    if n.kind() == "function_item" {
                        holders.pop();
                    } else if let Some(run) = closures.get_mut(&n.id()) {
                        run.2 = seen;
                    }
                    continue;
                }
                let holder = holders.last().copied().unwrap_or(self.root.id());
                if rs_is_assertion(n, src, vocab) {
                    held.entry(holder).or_default().push((seen, site(n)));
                    seen += 1;
                }
                match n.kind() {
                    "function_item" => {
                        stack.push((n, true));
                        holders.push(n.id());
                    }
                    "closure_expression" => {
                        closures.insert(n.id(), (holder, seen, seen));
                        stack.push((n, true));
                    }
                    _ => {}
                }
                let mut cursor = n.walk();
                let children: Vec<Node> = n.children(&mut cursor).collect();
                stack.extend(children.into_iter().rev().map(|c| (c, false)));
            }
            RsAssertsListed { held, closures }
        })
    }

    /// The run of `closure` and its assertions, in the order of the source. `None` for a
    /// closure the list does not hold.
    fn of(
        &self,
        closure: Node<'t>,
        src: &str,
        vocab: &AssertVocabulary,
    ) -> Option<(RsRun, &[(usize, Site)])> {
        let listed = self.listed(src, vocab);
        let run = listed.closures.get(&closure.id()).copied()?;
        let (holder, from, to) = run;
        let held = listed.held.get(&holder).map_or(&[][..], Vec::as_slice);
        super::ancestry::count(1);
        Some((
            run,
            &held[held.partition_point(|(at, _)| *at < from)
                ..held.partition_point(|(at, _)| *at < to)],
        ))
    }
}

/// The closure `catch_unwind` runs: its argument, or the argument of the
/// `AssertUnwindSafe(..)` that wraps it.
fn rs_unwind_closure<'a>(args: Node<'a>, src: &str) -> Option<Node<'a>> {
    if let Some(closure) = find_child_by_kind(args, "closure_expression") {
        return Some(closure);
    }
    let wrapper = find_child_by_kind(args, "call_expression")?;
    let callee = wrapper
        .child_by_field_name("function")
        .map_or("", |f| text(f, src));
    if callee.rsplit("::").next() != Some("AssertUnwindSafe") {
        return None;
    }
    find_child_by_kind(
        wrapper.child_by_field_name("arguments")?,
        "closure_expression",
    )
}

/// Whether a branch of the construct ends the test or hands the failure on: a panicking
/// or asserting macro, `resume_unwind`, `unwrap` / `expect`, `?`, `return`, or an exit.
fn rs_fails(node: Node, src: &str) -> bool {
    let mut fails = false;
    walk(node, &mut |n| {
        if fails || n.kind() == "function_item" {
            return false;
        }
        match n.kind() {
            "macro_invocation" => {
                let name = rs_macro_name(n, src);
                fails = RS_PANIC_MACROS.contains(&name) || name.starts_with("assert");
            }
            "try_expression" | "return_expression" => fails = true,
            "call_expression" => {
                if let Some(f) = n.child_by_field_name("function") {
                    if f.kind() == "field_expression" {
                        let method = f.child_by_field_name("field").map_or("", |m| text(m, src));
                        fails = RS_CHECKING_METHODS.contains(&method);
                    } else {
                        let callee = text(f, src);
                        let last = callee.rsplit("::").next().unwrap_or(callee);
                        fails = matches!(last, "resume_unwind" | "exit" | "abort");
                    }
                }
            }
            _ => {}
        }
        !fails
    });
    fails
}

/// The side of a `Result` a pattern names: `Some(true)` for `Err(..)`, `Some(false)` for
/// `Ok(..)`, `None` for anything else (a wildcard, a binding).
fn rs_pattern_is_err(pattern: Node, src: &str) -> Option<bool> {
    let mut side = None;
    walk(pattern, &mut |n| {
        if side.is_some() {
            return false;
        }
        if n.kind() == "tuple_struct_pattern" {
            let name = n.child_by_field_name("type").map_or("", |t| text(t, src));
            side = match name.rsplit("::").next() {
                Some("Err") => Some(true),
                Some("Ok") => Some(false),
                _ => None,
            };
            return false;
        }
        true
    });
    side
}

/// Whether the branch of an `if` taken when the unwind result is an `Err` fails.
/// `err_when_true` says which branch that is: the consequence, or the alternative.
/// With no `else`, the code after the `if` is not read as that branch.
fn rs_if_fails_on_err(if_expr: Node, err_when_true: bool, src: &str) -> bool {
    let branch = if err_when_true {
        if_expr.child_by_field_name("consequence")
    } else {
        if_expr.child_by_field_name("alternative")
    };
    branch.is_some_and(|b| rs_fails(b, src))
}

/// Whether an `if` whose condition is `condition` (the unwind result with `is_err()` or
/// `is_ok()` called on it, possibly negated) fails on the branch taken for an `Err`.
/// A condition of another shape is judged as before: by whether any branch fails.
fn rs_if_checks(if_expr: Node, condition: Node, src: &str) -> bool {
    let mut cond = condition;
    let mut negated = false;
    loop {
        match cond.kind() {
            "parenthesized_expression" => match cond.named_child(0) {
                Some(inner) => cond = inner,
                None => break,
            },
            "unary_expression" if text(cond, src).trim_start().starts_with('!') => {
                match cond.named_child(0) {
                    Some(inner) => {
                        negated = !negated;
                        cond = inner;
                    }
                    None => break,
                }
            }
            _ => break,
        }
    }
    let method = (cond.kind() == "call_expression")
        .then(|| cond.child_by_field_name("function"))
        .flatten()
        .filter(|f| f.kind() == "field_expression")
        .and_then(|f| f.child_by_field_name("field"))
        .map_or("", |m| text(m, src));
    match method {
        "is_err" => rs_if_fails_on_err(if_expr, !negated, src),
        "is_ok" => rs_if_fails_on_err(if_expr, negated, src),
        _ => rs_fails(if_expr, src),
    }
}

/// Whether a `match` on the unwind result fails in the arm taken for an `Err`: an arm
/// whose pattern names `Err`, or, when no arm does, one whose pattern does not name `Ok`.
/// A `match` with no arm for either side is judged by whether any arm fails.
fn rs_match_checks(match_expr: Node, src: &str) -> bool {
    let mut arms: Vec<(Option<bool>, bool)> = Vec::new();
    if let Some(body) = match_expr.child_by_field_name("body") {
        let mut c = body.walk();
        for arm in body.named_children(&mut c) {
            if arm.kind() != "match_arm" {
                continue;
            }
            let side = arm
                .child_by_field_name("pattern")
                .and_then(|p| rs_pattern_is_err(p, src));
            let fails = arm
                .child_by_field_name("value")
                .is_some_and(|v| rs_fails(v, src));
            arms.push((side, fails));
        }
    }
    if arms.iter().any(|(side, _)| *side == Some(true)) {
        arms.iter()
            .any(|(side, fails)| *side == Some(true) && *fails)
    } else if arms.iter().any(|(side, _)| *side == Some(false)) {
        arms.iter().any(|(side, fails)| side.is_none() && *fails)
    } else {
        rs_fails(match_expr, src)
    }
}

/// The identifiers of one Rust tree, by their text and in the order of the source, for
/// [`rs_binding_is_checked`].
///
/// A binding that holds an unwind result is judged by the uses of its name after it in
/// its function. Walking the function for each binding costs the function for each, and
/// a later `let` of the same name is itself a use, judged by the uses after it: with no
/// use that looks at the outcome, each binding asked every later one again, so 18
/// bindings of one name cost 1.0e12 instructions and each one more doubled it. The
/// identifiers are listed once, and what the uses from a given one on answer is kept, so
/// each use is judged once.
struct RsUses<'t, 's> {
    root: Node<'t>,
    listed: std::cell::OnceCell<RsIdentifiers<'t, 's>>,
    /// For a scope and a use in it, by their ids: whether that use or a later one of the
    /// same name in the scope looks at the outcome.
    from_here_on: std::cell::RefCell<std::collections::HashMap<(usize, usize), bool>>,
}

struct RsIdentifiers<'t, 's> {
    /// The identifiers with a given text, each with its place among all of them.
    by_name: std::collections::HashMap<&'s str, Vec<(usize, Node<'t>)>>,
    /// For the root and each function, by its id, the places of the identifiers in it.
    runs: std::collections::HashMap<usize, (usize, usize)>,
}

impl<'t, 's> RsUses<'t, 's> {
    fn new(root: Node<'t>) -> Self {
        Self {
            root,
            listed: std::cell::OnceCell::new(),
            from_here_on: Default::default(),
        }
    }

    fn listed(&self, src: &'s str) -> &RsIdentifiers<'t, 's> {
        self.listed.get_or_init(|| {
            let mut by_name: std::collections::HashMap<&str, Vec<(usize, Node)>> =
                Default::default();
            let mut runs = std::collections::HashMap::new();
            let mut seen = 0usize;
            // A node, and for one already entered the place its run began at.
            let mut stack: Vec<(Node, Option<usize>)> = vec![(self.root, None)];
            while let Some((n, begun)) = stack.pop() {
                super::ancestry::count(1);
                if let Some(from) = begun {
                    runs.insert(n.id(), (from, seen));
                    continue;
                }
                if n.kind() == "function_item" || n.id() == self.root.id() {
                    stack.push((n, Some(seen)));
                }
                if n.kind() == "identifier" {
                    by_name.entry(text(n, src)).or_default().push((seen, n));
                    seen += 1;
                }
                let mut cursor = n.walk();
                let children: Vec<Node> = n.children(&mut cursor).collect();
                stack.extend(children.into_iter().rev().map(|c| (c, None)));
            }
            RsIdentifiers { by_name, runs }
        })
    }
}

/// Whether a later use of the binding `name` looks at the outcome it holds: a use in the
/// function that holds `binding` (the file, outside any), after `binding`.
fn rs_binding_is_checked<'t, 's>(
    binding: Node<'t>,
    anc: &Ancestry<'t>,
    name: &str,
    src: &'s str,
    uses: &RsUses<'t, 's>,
) -> bool {
    let mut scope = binding;
    while let Some(p) = anc.parent(scope) {
        if scope.kind() == "function_item" {
            break;
        }
        scope = p;
    }
    let listed = uses.listed(src);
    let Some((from, to)) = listed.runs.get(&scope.id()).copied() else {
        // A scope the list has no run for: read as before the list was kept.
        let mut checked = false;
        walk(scope, &mut |n| {
            if checked {
                return false;
            }
            if n.kind() == "identifier"
                && n.start_byte() >= binding.end_byte()
                && text(n, src) == name
            {
                checked = !rs_value_is_discarded(n, anc, src, uses);
            }
            !checked
        });
        return checked;
    };
    let Some(named) = listed.by_name.get(name) else {
        return false;
    };
    let in_scope = &named
        [named.partition_point(|(at, _)| *at < from)..named.partition_point(|(at, _)| *at < to)];
    let later = &in_scope[in_scope.partition_point(|(_, n)| n.start_byte() < binding.end_byte())..];
    let mut judged = Vec::new();
    let mut checked = false;
    for (_, n) in later {
        super::ancestry::count(1);
        let key = (scope.id(), n.id());
        if let Some(known) = uses.from_here_on.borrow().get(&key).copied() {
            checked = known;
            break;
        }
        judged.push(key);
        if !rs_value_is_discarded(*n, anc, src, uses) {
            checked = true;
            break;
        }
    }
    // Each use judged here was thrown away, up to the one that was not: from any of them
    // on, the answer is this one.
    let mut from_here_on = uses.from_here_on.borrow_mut();
    for key in judged {
        from_here_on.insert(key, checked);
    }
    checked
}

/// Whether the value of `value` (the `catch_unwind` call, or a later use of the binding
/// that holds its result) is thrown away without anything looking at the outcome.
///
/// The value is followed outwards to where it ends up. It is looked at when a method that
/// panics on one side is called on it, when `?` or `return` hands it on, when it is the
/// value of the test function, when an asserting or panicking macro names it, and when an
/// `if` / `match` on it fails in the branch taken for an `Err` (`is_err()`, `Err(..)`).
/// It is thrown away as a statement, in `let _ =` or `_ =`, in `drop(..)`, in a binding
/// nothing looks at, and in an `if` / `match` that fails only in the branch taken for an
/// `Ok`: there the test fails when the assertion held and passes when it did not. A shape
/// not listed is followed further out, so a value this does not understand is reported,
/// not passed.
fn rs_value_is_discarded<'t, 's>(
    value: Node<'t>,
    anc: &Ancestry<'t>,
    src: &'s str,
    uses: &RsUses<'t, 's>,
) -> bool {
    let mut cur = value;
    while let Some(p) = anc.parent(cur) {
        match p.kind() {
            "expression_statement" => return true,
            "try_expression" | "return_expression" => return false,
            // The value of the function body is the value of the test.
            "function_item" | "closure_expression" | "source_file" => return false,
            "token_tree" => {
                // A use inside a macro: its arguments are tokens, so the macro decides.
                let mut m = p;
                while m.kind() == "token_tree" {
                    match anc.parent(m) {
                        Some(up) => m = up,
                        None => return true,
                    }
                }
                let name = rs_macro_name(m, src);
                return !(m.kind() == "macro_invocation"
                    && (RS_PANIC_MACROS.contains(&name) || name.starts_with("assert")));
            }
            "field_expression" => {
                let method = p.child_by_field_name("field").map_or("", |m| text(m, src));
                if RS_CHECKING_METHODS.contains(&method) {
                    return false;
                }
            }
            "arguments" => {
                let callee = anc
                    .parent(p)
                    .and_then(|call| call.child_by_field_name("function"))
                    .map_or("", |f| text(f, src));
                if matches!(callee, "drop" | "mem::drop" | "std::mem::drop") {
                    return true;
                }
            }
            "let_declaration" => {
                let Some(pattern) = p.child_by_field_name("pattern") else {
                    return true;
                };
                return pattern.kind() != "identifier"
                    || !rs_binding_is_checked(p, anc, text(pattern, src), src, uses);
            }
            "assignment_expression" => {
                let Some(left) = p.child_by_field_name("left") else {
                    return true;
                };
                return left.kind() != "identifier"
                    || !rs_binding_is_checked(p, anc, text(left, src), src, uses);
            }
            "match_expression" => {
                if p.child_by_field_name("value") == Some(cur) {
                    return !rs_match_checks(p, src);
                }
            }
            "if_expression" => {
                if p.child_by_field_name("condition") == Some(cur) {
                    return !rs_if_checks(p, cur, src);
                }
            }
            "let_condition" | "let_chain" => {
                let mut branch = p;
                while branch.kind() != "if_expression" && branch.kind() != "while_expression" {
                    match anc.parent(branch) {
                        Some(up)
                            if matches!(
                                up.kind(),
                                "let_chain" | "if_expression" | "while_expression"
                            ) =>
                        {
                            branch = up
                        }
                        _ => return true,
                    }
                }
                // `if let Err(..) = r`: the consequence is the branch for a failure;
                // `if let Ok(..) = r`: the alternative is. A chain of conditions, a
                // `while`, or another pattern is judged by whether any branch fails.
                let side = (p.kind() == "let_condition" && branch.kind() == "if_expression")
                    .then(|| p.child_by_field_name("pattern"))
                    .flatten()
                    .and_then(|pattern| rs_pattern_is_err(pattern, src));
                return match side {
                    Some(err) if anc.parent(p) == Some(branch) => {
                        !rs_if_fails_on_err(branch, err, src)
                    }
                    _ => !rs_fails(branch, src),
                };
            }
            _ => {}
        }
        cur = p;
    }
    true
}

pub fn rust<'t>(
    root: Node<'t>,
    anc: &Ancestry<'t>,
    src: &str,
    tests: &mut [TestFn],
    vocab: &AssertVocabulary,
) {
    if tests.is_empty() {
        return;
    }
    let mut caught = Caught::new(tests, root);
    let uses = RsUses::new(root);
    let asserts = RsAsserts::new(root);
    // The run of the last closure whose assertions were all handed to `attribute`.
    let mut handed: Option<RsRun> = None;
    walk(root, &mut |node| {
        if node.kind() != "call_expression" {
            return true;
        }
        let callee = node
            .child_by_field_name("function")
            .map_or("", |f| text(f, src));
        let is_catch_unwind = callee == "catch_unwind"
            || callee.ends_with("::catch_unwind")
            || callee.ends_with(".catch_unwind");
        if !is_catch_unwind {
            return true;
        }

        let Some(args) = node.child_by_field_name("arguments") else {
            return true;
        };
        let Some(closure) = rs_unwind_closure(args, src) else {
            return true;
        };

        let walked;
        let (run, inner_asserts): (Option<RsRun>, &[(usize, Site)]) =
            match asserts.of(closure, src, vocab) {
                Some((run, held)) => (Some(run), held),
                // A closure the list does not hold: read as before the list was kept.
                None => {
                    walked = rs_closure_contains_assert(closure, src, vocab)
                        .into_iter()
                        .map(|site| (0, site))
                        .collect::<Vec<_>>();
                    (None, &walked)
                }
            };
        if inner_asserts.is_empty() {
            return true;
        }
        // A closure inside one whose assertions were handed over holds some of the same
        // assertions, when the same function holds both: each was recorded then, or was
        // turned away for a reason that still stands, so handing them over again changes
        // nothing.
        if let (Some((holder, from, to)), Some((handed_holder, handed_from, handed_to))) =
            (run, handed)
        {
            if holder == handed_holder && handed_from <= from && to <= handed_to {
                return true;
            }
        }

        if rs_value_is_discarded(node, anc, src, &uses) {
            let handler_line = node.start_position().row + 1;
            // An assertion of no width does not overlap itself: it would be recorded
            // again, so a run that holds one is not kept as handed over.
            let mut every_one_has_width = true;
            for &(_, (line, span)) in inner_asserts {
                every_one_has_width &= span.0 < span.1;
                caught.attribute(CaughtAssertion {
                    line,
                    span,
                    tautology: false,
                    handler_line,
                    detail: "std::panic::catch_unwind with discarded result".to_string(),
                });
            }
            if every_one_has_width {
                handed = run.or(handed);
            }
        }
        true
    });
}

// ---------------------------------------------------------------------------
// JavaScript / TypeScript
// ---------------------------------------------------------------------------

const JS_FUNCTIONS: &[&str] = &[
    "arrow_function",
    "function_expression",
    "function_declaration",
];

fn js_callee<'a>(call: Node, src: &'a str) -> &'a str {
    call.child_by_field_name("function")
        .map_or("", |f| text(f, src))
}

/// Whether `func` is passed to a method known to run it before returning
/// (`SYNC_CALLBACKS`): `items.forEach((v) => ..)`.
fn js_is_sync_callback<'t>(func: Node<'t>, anc: &Ancestry<'t>, src: &str) -> bool {
    anc.parent(func)
        .filter(|args| args.kind() == "arguments")
        .and_then(|args| anc.parent(args))
        .filter(|call| call.kind() == "call_expression")
        .and_then(|call| call.child_by_field_name("function"))
        .filter(|callee| callee.kind() == "member_expression")
        .and_then(|callee| callee.child_by_field_name("property"))
        .is_some_and(|name| sync_callbacks("JavaScript / TypeScript").contains(&text(name, src)))
}

/// The body of the one function the file declares under `name`: a function declaration,
/// or a `const` / `let` / `var` bound to a function. None when the file declares no such
/// function, or more than one.
fn js_declared_function_body<'a>(root: Node<'a>, name: &str, src: &str) -> Option<Node<'a>> {
    let mut bodies = Vec::new();
    walk_tree(root, &mut |n| {
        let function = match n.kind() {
            "function_declaration" | "generator_function_declaration" => Some(n),
            "variable_declarator" => n
                .child_by_field_name("value")
                .filter(|v| matches!(v.kind(), "arrow_function" | "function_expression")),
            _ => None,
        };
        if let Some(function) = function {
            if n.child_by_field_name("name")
                .is_some_and(|id| text(id, src) == name)
            {
                bodies.push(function.child_by_field_name("body"));
            }
        }
        true
    });
    match bodies[..] {
        [body] => body,
        _ => None,
    }
}

/// The `finally` block of a `try` when it returns: the `return` replaces whatever the
/// `try` threw, so the failure is discarded.
fn js_finally_returns(try_stmt: Node) -> Option<Node> {
    let finally = try_stmt
        .child_by_field_name("finalizer")
        .or_else(|| find_child_by_kind(try_stmt, "finally_clause"))?;
    let block = finally
        .child_by_field_name("body")
        .or_else(|| find_child_by_kind(finally, "statement_block"))?;
    find_child_by_kind(block, "return_statement").map(|_| finally)
}

fn js_is_assertion(call: Node, src: &str, vocab: &AssertVocabulary) -> bool {
    let callee = js_callee(call, src);
    if configured(callee, vocab) {
        return true;
    }
    let is_assert = callee == "expect"
        || callee.starts_with("expect(")
        || callee == "assert"
        || callee.starts_with("assert.");
    let is_expect_chain = {
        let mut cur = call;
        while cur.kind() == "member_expression" {
            if let Some(obj) = cur.child_by_field_name("object") {
                cur = obj;
            } else {
                break;
            }
        }
        cur.kind() == "call_expression"
            && cur
                .child_by_field_name("function")
                .is_some_and(|f| text(f, src) == "expect")
    };
    is_assert || is_expect_chain
}

/// Whether a handler body (a block, or the expression an arrow function returns) neither
/// rethrows, nor asserts, nor fails the test. `done(err)` and `reject(..)` fail it: the
/// runner's callback with an argument, and the promise the test returns.
fn js_handler_swallows(body: Node, src: &str, vocab: &AssertVocabulary) -> bool {
    let mut fails = false;
    walk(body, &mut |n| {
        if fails || (n != body && JS_FUNCTIONS.contains(&n.kind())) {
            return false;
        }
        if n.kind() == "throw_statement" {
            fails = true;
        } else if n.kind() == "call_expression" {
            let callee = js_callee(n, src);
            let has_argument = n
                .child_by_field_name("arguments")
                .is_some_and(|a| a.named_child_count() > 0);
            fails = callee == "expect"
                || callee.starts_with("expect(")
                || callee == "assert"
                || callee.starts_with("assert.")
                || callee == "fail"
                || callee == "done.fail"
                || callee.ends_with(".fail")
                || callee == "reject"
                || (callee == "done" && has_argument)
                || configured(callee, vocab);
        }
        !fails
    });
    !fails
}

fn js_catch_body_swallows(catch_clause: Node, src: &str, vocab: &AssertVocabulary) -> bool {
    let Some(body) = catch_clause
        .child_by_field_name("body")
        .or_else(|| find_child_by_kind(catch_clause, "statement_block"))
    else {
        return true;
    };
    js_handler_swallows(body, src, vocab)
}

/// `<promise>.catch(<function that swallows>)`: the assertions in the chain before it.
/// The function is one written in place, or one the file declares under the name given.
/// The body of the one function a file declares under each name asked about
/// ([`js_declared_function_body`]), kept so the file is read once for a name and not
/// once for each `.catch(name)`.
type JsDeclared<'t, 's> = std::collections::HashMap<&'s str, Option<Node<'t>>>;

fn js_promise_catch<'t, 's>(
    root: Node<'t>,
    call: Node<'t>,
    src: &'s str,
    caught: &mut Caught,
    vocab: &AssertVocabulary,
    declared: &mut JsDeclared<'t, 's>,
    handed: &mut Option<(usize, usize)>,
) {
    let Some(function) = call.child_by_field_name("function") else {
        return;
    };
    if function.kind() != "member_expression"
        || function
            .child_by_field_name("property")
            .is_none_or(|p| text(p, src) != "catch")
    {
        return;
    }
    // A chain inside one whose assertions were all handed to `attribute` holds some of
    // the same assertions: each was recorded then, or was turned away for a reason that
    // still stands, so handing them over again changes nothing. A chain of `.catch()`
    // calls was walked once for each of them: 400 links, in a file of 21 kB, cost
    // 2.7e10 instructions.
    if let (Some(chain), Some((from, to))) = (function.child_by_field_name("object"), *handed) {
        super::ancestry::count(1);
        // A node of no width at either end of those bytes is not inside the chain.
        if chain.start_byte() < chain.end_byte()
            && from <= chain.start_byte()
            && chain.end_byte() <= to
        {
            return;
        }
    }
    let handler = call
        .child_by_field_name("arguments")
        .and_then(|a| a.named_child(0));
    let body = match handler {
        Some(h) if matches!(h.kind(), "arrow_function" | "function_expression") => {
            h.child_by_field_name("body")
        }
        Some(h) if h.kind() == "identifier" => {
            let name = text(h, src);
            *declared
                .entry(name)
                .or_insert_with(|| js_declared_function_body(root, name, src))
        }
        _ => None,
    };
    let Some(body) = body else {
        // A handler the file does not declare (`.catch(done)`, `.catch(h.ignore)`) is
        // not read.
        return;
    };
    if !js_handler_swallows(body, src, vocab) {
        return;
    }
    let (Some(chain), Some(property)) = (
        function.child_by_field_name("object"),
        function.child_by_field_name("property"),
    ) else {
        return;
    };
    let handler_line = property.start_position().row + 1;
    // An assertion of no width does not overlap itself: it would be recorded again, so
    // a chain that holds one is not kept as handed over.
    let mut every_one_has_width = true;
    walk(chain, &mut |n| {
        if n.kind() == "call_expression" && js_is_assertion(n, src, vocab) {
            every_one_has_width &= n.start_byte() < n.end_byte();
            caught.attribute(CaughtAssertion {
                line: n.start_position().row + 1,
                span: site(n).1,
                tautology: false,
                handler_line,
                detail: "promise .catch() swallows assertion error".to_string(),
            });
        }
        true
    });
    if every_one_has_width {
        *handed = Some((chain.start_byte(), chain.end_byte()));
    }
}

pub fn javascript<'t>(
    root: Node<'t>,
    anc: &Ancestry<'t>,
    src: &str,
    tests: &mut [TestFn],
    vocab: &AssertVocabulary,
) {
    if tests.is_empty() {
        return;
    }
    let mut caught = Caught::new(tests, root);
    let mut declared = JsDeclared::new();
    // The bytes of the last promise chain whose assertions were all handed over.
    let mut handed = None;
    walk(root, &mut |node| {
        if node.kind() == "call_expression" {
            js_promise_catch(
                root,
                node,
                src,
                &mut caught,
                vocab,
                &mut declared,
                &mut handed,
            );
            return true;
        }
        if node.kind() != "try_statement" {
            return true;
        }
        let Some(body) = node.child_by_field_name("body") else {
            return true;
        };
        let finally = js_finally_returns(node);
        let catch_clause = node
            .child_by_field_name("handler")
            .or_else(|| find_child_by_kind(node, "catch_clause"))
            .filter(|clause| js_catch_body_swallows(*clause, src, vocab));
        let (handler, detail) = match (finally, catch_clause) {
            (Some(finally), _) => (finally, FINALLY_RETURNS),
            (None, Some(clause)) => (clause, "try/catch swallows assertion error"),
            (None, None) => return true,
        };

        let handler_line = handler.start_position().row + 1;
        walk(body, &mut |n| {
            if n.kind() == "try_statement"
                || (JS_FUNCTIONS.contains(&n.kind()) && !js_is_sync_callback(n, anc, src))
            {
                return false;
            }
            if n.kind() == "call_expression" && js_is_assertion(n, src, vocab) {
                caught.attribute(CaughtAssertion {
                    line: n.start_position().row + 1,
                    span: site(n).1,
                    tautology: false,
                    handler_line,
                    detail: detail.to_string(),
                });
            }
            true
        });
        true
    });
}

// ---------------------------------------------------------------------------
// Java (and the JVM type names Kotlin shares)
// ---------------------------------------------------------------------------

/// `AssertionError` and its ancestors. JUnit 5 `AssertionFailedError`, JUnit 4
/// `ComparisonFailure` and every other subclass are read as a type that is not known.
const JVM_ASSERTION_ANCESTORS: &[&str] = &["AssertionError", "Error", "Throwable"];

/// JDK and Kotlin classes that an `AssertionError` is not an instance of: `Exception` and
/// its subclasses, and the `Error` subclasses beside `AssertionError`.
const JVM_UNRELATED: &[&str] = &[
    "Exception",
    "RuntimeException",
    "IOException",
    "UncheckedIOException",
    "FileNotFoundException",
    "EOFException",
    "InterruptedIOException",
    "InterruptedException",
    "ExecutionException",
    "TimeoutException",
    "CancellationException",
    "CompletionException",
    "IllegalArgumentException",
    "IllegalStateException",
    "NullPointerException",
    "IndexOutOfBoundsException",
    "ArrayIndexOutOfBoundsException",
    "StringIndexOutOfBoundsException",
    "ArithmeticException",
    "ClassCastException",
    "NumberFormatException",
    "UnsupportedOperationException",
    "ConcurrentModificationException",
    "NoSuchElementException",
    "ClassNotFoundException",
    "CloneNotSupportedException",
    "ReflectiveOperationException",
    "NoSuchMethodException",
    "NoSuchFieldException",
    "IllegalAccessException",
    "InstantiationException",
    "InvocationTargetException",
    "SQLException",
    "URISyntaxException",
    "MalformedURLException",
    "SocketException",
    "SocketTimeoutException",
    "UnknownHostException",
    "ConnectException",
    "ParseException",
    "DateTimeException",
    "DateTimeParseException",
    "GeneralSecurityException",
    "NoSuchAlgorithmException",
    "SecurityException",
    "OutOfMemoryError",
    "StackOverflowError",
    "VirtualMachineError",
    "LinkageError",
    "NoClassDefFoundError",
    "ExceptionInInitializerError",
    "UnsatisfiedLinkError",
    "NotImplementedError",
];

/// The reach of a `catch` clause, from each type its parameter names (`A | B` names two).
fn java_catch_reach(clause: Node, src: &str) -> Reach {
    let types = clause
        .child_by_field_name("param")
        .or_else(|| find_child_by_kind(clause, "catch_formal_parameter"))
        .and_then(|param| find_child_by_kind(param, "catch_type"));
    let Some(types) = types else {
        return Reach::Maybe;
    };
    let mut c = types.walk();
    let names: Vec<&str> = types
        .named_children(&mut c)
        .map(|t| simple_name(t, src))
        .collect();
    reach(&names, JVM_ASSERTION_ANCESTORS, JVM_UNRELATED)
}

fn java_catch_body_swallows(clause: Node, src: &str, vocab: &AssertVocabulary) -> bool {
    let Some(body) = clause
        .child_by_field_name("body")
        .or_else(|| find_child_by_kind(clause, "block"))
    else {
        return true;
    };

    let mut has_throw = false;
    let mut has_fail = false;
    let mut has_assert = false;

    walk(body, &mut |n| {
        // One is enough: the rest of the body, and the handlers inside it, say no more.
        if has_throw
            || has_fail
            || has_assert
            || matches!(
                n.kind(),
                "class_declaration" | "method_declaration" | "lambda_expression"
            )
        {
            return false;
        }
        if n.kind() == "throw_statement" {
            has_throw = true;
            return false;
        }
        if n.kind() == "assert_statement" {
            has_assert = true;
            return false;
        }
        if n.kind() == "method_invocation" {
            let name = n.child_by_field_name("name").map_or("", |m| text(m, src));
            if name.starts_with("assert") || configured(name, vocab) {
                has_assert = true;
                return false;
            }
            if name == "fail" || name.ends_with(".fail") {
                has_fail = true;
                return false;
            }
        }
        true
    });

    !has_throw && !has_fail && !has_assert
}

/// Whether `lambda` is passed to a method known to run it before returning
/// (`SYNC_CALLBACKS`): `items.forEach(v -> ..)`.
fn java_is_sync_callback<'t>(lambda: Node<'t>, anc: &Ancestry<'t>, src: &str) -> bool {
    anc.parent(lambda)
        .filter(|args| args.kind() == "argument_list")
        .and_then(|args| anc.parent(args))
        .filter(|call| call.kind() == "method_invocation")
        .and_then(|call| call.child_by_field_name("name"))
        .is_some_and(|name| sync_callbacks("Java").contains(&text(name, src)))
}

/// The `finally` clause of a `try` when its block returns: the `return` replaces
/// whatever the `try` threw, so the failure is discarded.
fn java_finally_returns(try_stmt: Node) -> Option<Node> {
    let finally = find_child_by_kind(try_stmt, "finally_clause")?;
    let block = find_child_by_kind(finally, "block")?;
    find_child_by_kind(block, "return_statement").map(|_| finally)
}

pub fn java<'t>(
    root: Node<'t>,
    anc: &Ancestry<'t>,
    src: &str,
    tests: &mut [TestFn],
    vocab: &AssertVocabulary,
) {
    if tests.is_empty() {
        return;
    }
    let mut caught = Caught::new(tests, root);
    walk(root, &mut |node| {
        if !matches!(
            node.kind(),
            "try_statement" | "try_with_resources_statement"
        ) {
            return true;
        }
        let Some(body) = node.child_by_field_name("body") else {
            return true;
        };

        let mut cursor = node.walk();
        let handlers: Vec<(Node, Reach, bool)> = node
            .children(&mut cursor)
            .filter(|c| c.kind() == "catch_clause")
            .map(|c| {
                (
                    c,
                    java_catch_reach(c, src),
                    java_catch_body_swallows(c, src, vocab),
                )
            })
            .collect();
        let (handler, detail) = match (java_finally_returns(node), swallowing_handler(&handlers)) {
            (Some(finally), _) => (finally, FINALLY_RETURNS),
            (None, Some(clause)) => (clause, "AssertionError caught by catch clause"),
            (None, None) => return true,
        };
        let handler_line = handler.start_position().row + 1;

        walk(body, &mut |n| {
            if matches!(
                n.kind(),
                "class_declaration"
                    | "method_declaration"
                    | "try_statement"
                    | "try_with_resources_statement"
            ) || (n.kind() == "lambda_expression" && !java_is_sync_callback(n, anc, src))
            {
                return false;
            }
            let is_assertion = match n.kind() {
                "assert_statement" => true,
                "method_invocation" => {
                    let name = n.child_by_field_name("name").map_or("", |m| text(m, src));
                    (name.starts_with("assert") && name != "assertThrows")
                        || configured(name, vocab)
                }
                _ => false,
            };
            if is_assertion {
                caught.attribute(CaughtAssertion {
                    line: n.start_position().row + 1,
                    span: site(n).1,
                    tautology: false,
                    handler_line,
                    detail: detail.to_string(),
                });
            }
            true
        });
        true
    });
}

// ---------------------------------------------------------------------------
// Kotlin
// ---------------------------------------------------------------------------

const KT_SCOPES: &[&str] = &[
    "lambda_literal",
    "function_declaration",
    "class_declaration",
];

/// The name a call is made by: `assertEquals` of `assertEquals(..)`, `getOrThrow` of
/// `r.getOrThrow()`.
fn kt_callee<'a>(call: Node, src: &'a str) -> &'a str {
    match call.named_child(0) {
        Some(f) if f.kind() == "identifier" => text(f, src),
        Some(f) if f.kind() == "navigation_expression" => simple_name(f, src),
        _ => "",
    }
}

/// `assertEquals(..)`, `assert(..)`, `x.shouldBe(y)` and the infix `x shouldBe y`; not the
/// calls that expect a throw (`assertThrows`, `assertFailsWith`, `shouldThrow`).
fn kt_is_assertion(node: Node, src: &str, vocab: &AssertVocabulary) -> bool {
    let name = match node.kind() {
        "call_expression" => kt_callee(node, src),
        "infix_expression" => node.named_child(1).map_or("", |op| text(op, src)),
        _ => return false,
    };
    if node.kind() == "call_expression" && configured(name, vocab) {
        return true;
    }
    (name.starts_with("assert") || name.starts_with("should"))
        && !name.starts_with("assertThrows")
        && !name.starts_with("assertFails")
        && !name.starts_with("shouldThrow")
        && !name.starts_with("shouldNotThrow")
}

/// Whether the subtree throws, asserts or calls a function that fails the test.
fn kt_fails(node: Node, src: &str, vocab: &AssertVocabulary) -> bool {
    let mut fails = false;
    walk(node, &mut |n| {
        if fails || (n != node && KT_SCOPES.contains(&n.kind())) {
            return false;
        }
        fails = n.kind() == "throw_expression"
            || kt_is_assertion(n, src, vocab)
            || (n.kind() == "call_expression" && matches!(kt_callee(n, src), "fail" | "error"));
        !fails
    });
    fails
}

/// Whether `lambda` is passed to a function known to run it before returning
/// (`SYNC_CALLBACKS`): `x.let { .. }`, `items.forEach { .. }`, `with(x) { .. }`.
fn kt_is_sync_callback<'t>(lambda: Node<'t>, anc: &Ancestry<'t>, src: &str) -> bool {
    if lambda.kind() != "lambda_literal" {
        return false;
    }
    let mut cur = lambda;
    let call = loop {
        match anc.parent(cur) {
            Some(p) if p.kind() == "call_expression" => break p,
            Some(p)
                if matches!(
                    p.kind(),
                    "annotated_lambda" | "call_suffix" | "value_argument" | "value_arguments"
                ) =>
            {
                cur = p
            }
            _ => return false,
        }
    };
    // `with(x) { .. }` is a call of the call `with(x)`.
    let callee = match call.named_child(0) {
        Some(inner) if inner.kind() == "call_expression" => kt_callee(inner, src),
        _ => kt_callee(call, src),
    };
    sync_callbacks("Kotlin").contains(&callee)
}

/// Lines of the assertions directly in `body`: not in a nested `try`, and not in a lambda
/// unless it is passed to a function that runs it before returning.
fn kt_assertions<'t>(
    body: Node<'t>,
    anc: &Ancestry<'t>,
    src: &str,
    vocab: &AssertVocabulary,
) -> Vec<Site> {
    let mut lines = Vec::new();
    walk(body, &mut |n| {
        if n != body
            && ((KT_SCOPES.contains(&n.kind()) && !kt_is_sync_callback(n, anc, src))
                || n.kind() == "try_expression")
        {
            return false;
        }
        if kt_is_assertion(n, src, vocab) {
            lines.push(site(n));
        }
        true
    });
    lines
}

fn kt_catch_reach(catch: Node, src: &str) -> Reach {
    match find_child_by_kind(catch, "user_type") {
        Some(t) => reach(
            &[simple_name(t, src)],
            JVM_ASSERTION_ANCESTORS,
            JVM_UNRELATED,
        ),
        None => Reach::Maybe,
    }
}

/// The `finally` block of a `try` when it returns: the `return` replaces whatever the
/// `try` threw, so the failure is discarded.
fn kt_finally_returns<'a>(try_expr: Node<'a>, src: &str) -> Option<Node<'a>> {
    let finally = find_child_by_kind(try_expr, "finally_block")?;
    let block = find_child_by_kind(finally, "block")?;
    let statements = find_child_by_kind(block, "statements").unwrap_or(block);
    let mut c = statements.walk();
    let returns = statements.named_children(&mut c).any(|stmt| {
        matches!(stmt.kind(), "return_expression" | "jump_expression")
            && text(stmt, src).trim_start().starts_with("return")
    });
    returns.then_some(finally)
}

/// The identifiers of one Kotlin tree, by their text and in the order of the source, for
/// [`kt_result_is_unused`].
///
/// A `runCatching` result bound to a name is looked at when the name is used after the
/// binding, in the function or lambda that holds it. Walking that function for each
/// binding costs the function for each: 400 bindings in one test, in a file of 21 kB,
/// cost 2.6e10 instructions. The identifiers are listed once, and a binding asks whether
/// the last one of its name in its function stands after it.
struct KtNames<'t, 's> {
    root: Node<'t>,
    listed: std::cell::OnceCell<KtIdentifiers<'s>>,
}

struct KtIdentifiers<'s> {
    /// The identifiers with a given text: for each, its place among all of them and the
    /// byte it starts at.
    by_name: std::collections::HashMap<&'s str, Vec<(usize, usize)>>,
    /// For the root, each function and each lambda, by its id, the places of the
    /// identifiers in it.
    runs: std::collections::HashMap<usize, (usize, usize)>,
}

impl<'t, 's> KtNames<'t, 's> {
    fn new(root: Node<'t>) -> Self {
        Self {
            root,
            listed: std::cell::OnceCell::new(),
        }
    }

    fn listed(&self, src: &'s str) -> &KtIdentifiers<'s> {
        self.listed.get_or_init(|| {
            let mut by_name: std::collections::HashMap<&str, Vec<(usize, usize)>> =
                Default::default();
            let mut runs = std::collections::HashMap::new();
            let mut seen = 0usize;
            // A node, and for one already entered the place its run began at.
            let mut stack: Vec<(Node, Option<usize>)> = vec![(self.root, None)];
            while let Some((n, begun)) = stack.pop() {
                super::ancestry::count(1);
                if let Some(from) = begun {
                    runs.insert(n.id(), (from, seen));
                    continue;
                }
                if matches!(n.kind(), "function_declaration" | "lambda_literal")
                    || n.id() == self.root.id()
                {
                    stack.push((n, Some(seen)));
                }
                if n.kind() == "identifier" {
                    by_name
                        .entry(text(n, src))
                        .or_default()
                        .push((seen, n.start_byte()));
                    seen += 1;
                }
                let mut cursor = n.walk();
                let children: Vec<Node> = n.children(&mut cursor).collect();
                stack.extend(children.into_iter().rev().map(|c| (c, None)));
            }
            KtIdentifiers { by_name, runs }
        })
    }

    /// Whether an identifier `name` stands in `scope` at or after the byte `after`.
    /// `None` for a scope the list has no run for.
    fn used_after(&self, scope: Node<'t>, name: &str, after: usize, src: &'s str) -> Option<bool> {
        let listed = self.listed(src);
        let (from, to) = listed.runs.get(&scope.id()).copied()?;
        super::ancestry::count(1);
        let Some(named) = listed.by_name.get(name) else {
            return Some(false);
        };
        // The identifiers are in the order of the source: the last of the run is the
        // one that starts furthest on.
        let in_scope = &named[named.partition_point(|(at, _)| *at < from)
            ..named.partition_point(|(at, _)| *at < to)];
        Some(in_scope.last().is_some_and(|(_, start)| *start >= after))
    }
}

/// Whether nothing looks at the result of a `runCatching { }` call. It is looked at when
/// `getOrThrow()` is called on it, when a callback chained on it (`onFailure { }`,
/// `fold`, `recover`, `getOrElse`) throws or fails, when it is bound to a name used
/// later, and when it is passed on (an argument, a `return`, an expression body).
fn kt_result_is_unused<'t, 's>(
    call: Node<'t>,
    anc: &Ancestry<'t>,
    src: &'s str,
    vocab: &AssertVocabulary,
    names: &KtNames<'t, 's>,
) -> bool {
    let mut cur = call;
    while let Some(p) = anc.parent(cur) {
        match p.kind() {
            "navigation_expression" => {
                if simple_name(p, src) == "getOrThrow" {
                    return false;
                }
            }
            "call_expression" => {
                // A call chained on the result: its callback may be what fails the test.
                let callback = find_child_by_kind(p, "annotated_lambda")
                    .and_then(|a| find_child_by_kind(a, "lambda_literal"));
                if let Some(lambda) = callback {
                    if kt_fails(lambda, src, vocab) {
                        return false;
                    }
                }
            }
            "parenthesized_expression" => {}
            "block" | "statements" | "lambda_literal" | "source_file" => return true,
            "property_declaration" => {
                let name = find_child_by_kind(p, "variable_declaration")
                    .map_or("", |v| simple_name(v, src));
                let mut scope = p;
                while let Some(up) = anc.parent(scope) {
                    if scope.kind() == "function_declaration" || scope.kind() == "lambda_literal" {
                        break;
                    }
                    scope = up;
                }
                if let Some(used) = names.used_after(scope, name, p.end_byte(), src) {
                    return !used;
                }
                // A scope the list has no run for: read as before the list was kept.
                let mut used = false;
                walk(scope, &mut |n| {
                    if n.kind() == "identifier"
                        && n.start_byte() >= p.end_byte()
                        && text(n, src) == name
                    {
                        used = true;
                    }
                    !used
                });
                return !used;
            }
            _ => return false,
        }
        cur = p;
    }
    true
}

pub fn kotlin<'t>(
    root: Node<'t>,
    anc: &Ancestry<'t>,
    src: &str,
    tests: &mut [TestFn],
    vocab: &AssertVocabulary,
) {
    if tests.is_empty() {
        return;
    }
    let mut caught = Caught::new(tests, root);
    let names = KtNames::new(root);
    walk(root, &mut |node| {
        if node.kind() == "try_expression" {
            let Some(body) = find_child_by_kind(node, "block") else {
                return true;
            };
            let mut cursor = node.walk();
            let handlers: Vec<(Node, Reach, bool)> = node
                .children(&mut cursor)
                .filter(|c| c.kind() == "catch_block")
                .map(|c| {
                    let swallows =
                        find_child_by_kind(c, "block").is_none_or(|b| !kt_fails(b, src, vocab));
                    (c, kt_catch_reach(c, src), swallows)
                })
                .collect();
            let catching = match (kt_finally_returns(node, src), swallowing_handler(&handlers)) {
                (Some(finally), _) => Some((finally, FINALLY_RETURNS)),
                (None, Some(clause)) => Some((clause, "AssertionError caught by catch clause")),
                (None, None) => None,
            };
            if let Some((handler, detail)) = catching {
                let handler_line = handler.start_position().row + 1;
                for (line, span) in kt_assertions(body, anc, src, vocab) {
                    caught.attribute(CaughtAssertion {
                        line,
                        span,
                        tautology: false,
                        handler_line,
                        detail: detail.to_string(),
                    });
                }
            }
        } else if node.kind() == "call_expression" && kt_callee(node, src) == "runCatching" {
            let lambda = find_child_by_kind(node, "annotated_lambda")
                .and_then(|a| find_child_by_kind(a, "lambda_literal"));
            if let Some(lambda) = lambda {
                if kt_result_is_unused(node, anc, src, vocab, &names) {
                    let handler_line = node.start_position().row + 1;
                    for (line, span) in kt_assertions(lambda, anc, src, vocab) {
                        caught.attribute(CaughtAssertion {
                            line,
                            span,
                            tautology: false,
                            handler_line,
                            detail: "runCatching with the result unused".to_string(),
                        });
                    }
                }
            }
        }
        true
    });
}

// ---------------------------------------------------------------------------
// C#
// ---------------------------------------------------------------------------

/// The ancestor of every assertion failure type: NUnit `AssertionException`, xUnit
/// `XunitException` and MSTest `AssertFailedException` all derive from `Exception`, and
/// are read, like their subclasses, as a type that is not known.
const CS_ASSERTION_ANCESTORS: &[&str] = &["Exception"];

/// .NET classes that an assertion failure is not an instance of.
const CS_UNRELATED: &[&str] = &[
    "SystemException",
    "ApplicationException",
    "IOException",
    "FileNotFoundException",
    "DirectoryNotFoundException",
    "PathTooLongException",
    "EndOfStreamException",
    "InvalidDataException",
    "ArgumentException",
    "ArgumentNullException",
    "ArgumentOutOfRangeException",
    "InvalidOperationException",
    "NotSupportedException",
    "NotImplementedException",
    "NullReferenceException",
    "FormatException",
    "InvalidCastException",
    "IndexOutOfRangeException",
    "KeyNotFoundException",
    "TimeoutException",
    "OperationCanceledException",
    "TaskCanceledException",
    "ObjectDisposedException",
    "UnauthorizedAccessException",
    "HttpRequestException",
    "SocketException",
    "WebException",
    "JsonException",
    "AggregateException",
    "ArithmeticException",
    "DivideByZeroException",
    "OverflowException",
    "StackOverflowException",
    "OutOfMemoryException",
    "DbException",
    "SqlException",
];

/// The method of an `Assert.<Method>(..)` invocation, read from the callee node: a string
/// or a comment that mentions `Assert.` is not one. The class is `Assert` or one named
/// `...Assert` (`StringAssert`, `CollectionAssert`, `ClassicAssert`, a project's own).
fn csharp_assert_method<'a>(invocation: Node, src: &'a str) -> Option<&'a str> {
    let function = invocation.child_by_field_name("function")?;
    if function.kind() != "member_access_expression" {
        return None;
    }
    let class = function.child_by_field_name("expression")?;
    if !matches!(
        class.kind(),
        "identifier" | "member_access_expression" | "qualified_name"
    ) || !simple_name(class, src).ends_with("Assert")
    {
        return None;
    }
    function.child_by_field_name("name").map(|n| text(n, src))
}

fn csharp_catch_reach(clause: Node, src: &str) -> Reach {
    let by_type = match find_child_by_kind(clause, "catch_declaration") {
        // `catch { }` catches everything.
        None => Reach::Always,
        Some(decl) => match decl.child_by_field_name("type") {
            Some(t) => reach(&[simple_name(t, src)], CS_ASSERTION_ANCESTORS, CS_UNRELATED),
            None => Reach::Maybe,
        },
    };
    // A `when` filter may turn the failure away, so the clause no longer always catches.
    if by_type == Reach::Always && find_child_by_kind(clause, "catch_filter_clause").is_some() {
        return Reach::Maybe;
    }
    by_type
}

/// NUnit calls that end the test without failing it: the result is a pass, an ignored
/// test or an inconclusive one. In a handler, they leave the failure swallowed.
const CS_NOT_A_FAILURE: &[&str] = &["Pass", "Ignore", "Inconclusive"];

/// The name a call is made by when it is a helper the configuration lists: `CheckTotal`
/// of `CheckTotal(..)` or `Checks.CheckTotal(..)`.
fn csharp_is_configured(invocation: Node, src: &str, vocab: &AssertVocabulary) -> bool {
    invocation
        .child_by_field_name("function")
        .is_some_and(|f| match f.kind() {
            "identifier" => configured(text(f, src), vocab),
            "member_access_expression" => f
                .child_by_field_name("name")
                .is_some_and(|n| configured(text(n, src), vocab)),
            _ => false,
        })
}

/// Whether `lambda` is passed to a method known to run it before returning
/// (`SYNC_CALLBACKS`): `items.ForEach(v => ..)`.
fn csharp_is_sync_callback<'t>(lambda: Node<'t>, anc: &Ancestry<'t>, src: &str) -> bool {
    let mut cur = lambda;
    let call = loop {
        match anc.parent(cur) {
            Some(p) if p.kind() == "invocation_expression" => break p,
            Some(p) if matches!(p.kind(), "argument" | "argument_list") => cur = p,
            _ => return false,
        }
    };
    call.child_by_field_name("function")
        .filter(|f| f.kind() == "member_access_expression")
        .and_then(|f| f.child_by_field_name("name"))
        .is_some_and(|name| sync_callbacks("C#").contains(&text(name, src)))
}

fn csharp_catch_body_swallows(clause: Node, src: &str, vocab: &AssertVocabulary) -> bool {
    let Some(body) = find_child_by_kind(clause, "block") else {
        return true;
    };

    let mut fails = false;
    walk(body, &mut |n| {
        if fails
            || matches!(
                n.kind(),
                "class_declaration" | "method_declaration" | "lambda_expression"
            )
        {
            return false;
        }
        // A rethrow, `Assert.Fail`, or an assertion on the caught error.
        fails = n.kind() == "throw_statement"
            || (n.kind() == "invocation_expression"
                && (csharp_assert_method(n, src).is_some_and(|m| !CS_NOT_A_FAILURE.contains(&m))
                    || csharp_is_configured(n, src, vocab)));
        !fails
    });
    !fails
}

pub fn csharp<'t>(
    root: Node<'t>,
    anc: &Ancestry<'t>,
    src: &str,
    tests: &mut [TestFn],
    vocab: &AssertVocabulary,
) {
    if tests.is_empty() {
        return;
    }
    let mut caught = Caught::new(tests, root);
    walk(root, &mut |node| {
        if node.kind() != "try_statement" {
            return true;
        }
        let Some(body_node) = find_child_by_kind(node, "block") else {
            return true;
        };

        let mut c2 = node.walk();
        let handlers: Vec<(Node, Reach, bool)> = node
            .children(&mut c2)
            .filter(|c| c.kind() == "catch_clause")
            .map(|c| {
                (
                    c,
                    csharp_catch_reach(c, src),
                    csharp_catch_body_swallows(c, src, vocab),
                )
            })
            .collect();
        let Some(clause) = swallowing_handler(&handlers) else {
            return true;
        };
        let handler_line = clause.start_position().row + 1;

        walk(body_node, &mut |n| {
            if matches!(
                n.kind(),
                "class_declaration" | "method_declaration" | "try_statement"
            ) || (n.kind() == "lambda_expression" && !csharp_is_sync_callback(n, anc, src))
            {
                return false;
            }
            if n.kind() == "invocation_expression"
                && (csharp_assert_method(n, src).is_some_and(|m| !m.starts_with("Throws"))
                    || csharp_is_configured(n, src, vocab))
            {
                caught.attribute(CaughtAssertion {
                    line: n.start_position().row + 1,
                    span: site(n).1,
                    tautology: false,
                    handler_line,
                    detail: "Assertion caught by catch clause".to_string(),
                });
            }
            true
        });
        true
    });
}

// ---------------------------------------------------------------------------
// Go
// ---------------------------------------------------------------------------

/// Methods of `testing.T` (and of a logger or `os`) that fail or end the test.
const GO_FAILING_METHODS: &[&str] = &[
    "Fatal", "Fatalf", "FailNow", "Fail", "Failf", "Error", "Errorf", "Exit",
];

/// Walks the nodes of a function body that belong to the function itself: a function
/// literal inside it is another function, with its own deferred calls and panics.
fn go_own_nodes<'a>(body: Node<'a>, visit: &mut dyn FnMut(Node<'a>)) {
    walk_tree(body, &mut |n| {
        if n != body && n.kind() == "func_literal" {
            return false;
        }
        visit(n);
        true
    });
}

/// Whether `call` is a call of the built-in `name` (`recover`, `panic`).
fn go_calls_builtin(call: Node, name: &str, src: &str) -> bool {
    call.kind() == "call_expression"
        && call
            .child_by_field_name("function")
            .is_some_and(|f| f.kind() == "identifier" && text(f, src) == name)
}

/// Whether a call in a deferred function fails the test, so that a panic it recovered
/// from is not swallowed: `panic(..)`, a failing method of `testing.T` (`t.Fatal(r)`,
/// `t.Errorf(..)`, `t.Fail()`), any `require` / `assert` call (`require.Nil(t, r)`), a
/// suite's (`s.Require().Nil(r)`), or a helper the configuration lists.
///
/// `err.Error()` formats an error and `fmt.Errorf(..)` builds one: neither fails the test.
fn go_call_fails_the_test(call: Node, src: &str, vocab: &AssertVocabulary) -> bool {
    let Some(function) = call.child_by_field_name("function") else {
        return false;
    };
    if function.kind() == "identifier" {
        let name = text(function, src);
        return name == "panic" || configured(name, vocab);
    }
    if function.kind() != "selector_expression" {
        return false;
    }
    let (Some(on), Some(method)) = (
        function.child_by_field_name("operand"),
        function.child_by_field_name("field"),
    ) else {
        return false;
    };
    let method = text(method, src);
    if configured(method, vocab) {
        return true;
    }
    match on.kind() {
        "identifier" => {
            let on = text(on, src);
            if matches!(on, "require" | "assert") {
                return true;
            }
            if !GO_FAILING_METHODS.contains(&method) || on == "fmt" {
                return false;
            }
            // `t.Error(r)` reports; `err.Error()` takes no argument.
            method != "Error"
                || call
                    .child_by_field_name("arguments")
                    .is_some_and(|a| a.named_child_count() > 0)
        }
        // `s.Require().NoError(..)`, `s.Assert().Nil(..)`, `s.T().Fatal(..)`.
        "call_expression" => {
            let through = on
                .child_by_field_name("function")
                .filter(|f| f.kind() == "selector_expression")
                .and_then(|f| f.child_by_field_name("field"))
                .map_or("", |f| text(f, src));
            matches!(through, "Require" | "Assert")
                || (through == "T" && GO_FAILING_METHODS.contains(&method))
        }
        _ => GO_FAILING_METHODS.contains(&method) && method != "Error",
    }
}

/// Whether a `defer` statement runs a function that calls `recover()` and then lets the
/// test go on as passed. The function is a literal, or one the file declares; any other
/// (a method, a function of another file) is not read.
fn go_defer_swallows(
    defer: Node,
    declared: &std::collections::HashMap<&str, Node>,
    src: &str,
    vocab: &AssertVocabulary,
) -> bool {
    let function = find_child_by_kind(defer, "call_expression")
        .and_then(|call| call.child_by_field_name("function"));
    let body = match function {
        Some(f) if f.kind() == "func_literal" => f.child_by_field_name("body"),
        Some(f) if f.kind() == "identifier" => declared.get(text(f, src)).copied(),
        _ => None,
    };
    let Some(body) = body else {
        return false;
    };
    let mut recovers = false;
    go_own_nodes(body, &mut |n| {
        recovers = recovers || go_calls_builtin(n, "recover", src);
    });
    if !recovers {
        return false;
    }
    let mut fails = false;
    walk(body, &mut |n| {
        fails = fails || (n.kind() == "call_expression" && go_call_fails_the_test(n, src, vocab));
        !fails
    });
    !fails
}

pub fn go(root: Node, src: &str, tests: &mut [TestFn], vocab: &AssertVocabulary) {
    if tests.is_empty() {
        return;
    }
    let mut caught = Caught::new(tests, root);
    // The functions the file declares, by name: a deferred function, or a check that
    // panics, may be one of them.
    let mut declared = std::collections::HashMap::new();
    walk_tree(root, &mut |n| {
        if n.kind() == "function_declaration" {
            if let (Some(name), Some(body)) =
                (n.child_by_field_name("name"), n.child_by_field_name("body"))
            {
                declared.insert(text(name, src), body);
            }
        }
        true
    });
    let panics = |body: Node| {
        let mut found = false;
        go_own_nodes(body, &mut |n| {
            found = found || go_calls_builtin(n, "panic", src);
        });
        found
    };

    walk(root, &mut |node| {
        if !matches!(
            node.kind(),
            "function_declaration" | "method_declaration" | "func_literal"
        ) {
            return true;
        }
        let Some(body) = node.child_by_field_name("body") else {
            return true;
        };

        // The first deferred function of this function that swallows a panic: it is in
        // force for what runs after the `defer` statement.
        let mut handler: Option<Node> = None;
        go_own_nodes(body, &mut |n| {
            if n.kind() == "defer_statement"
                && handler.is_none_or(|h| n.start_byte() < h.start_byte())
                && go_defer_swallows(n, &declared, src, vocab)
            {
                handler = Some(n);
            }
        });
        let Some(handler) = handler else {
            return true;
        };
        let handler_line = handler.start_position().row + 1;

        go_own_nodes(body, &mut |n| {
            if n.kind() != "call_expression" || n.start_byte() < handler.end_byte() {
                return;
            }
            // A `panic(..)` written in the test is a check the pack does not count; a
            // call to a function of the file that panics is one it does.
            let counted = if go_calls_builtin(n, "panic", src) {
                false
            } else if n
                .child_by_field_name("function")
                .filter(|f| f.kind() == "identifier")
                .and_then(|f| declared.get(text(f, src)))
                .is_some_and(|helper| panics(*helper))
            {
                true
            } else {
                return;
            };
            caught.attribute(CaughtAssertion {
                line: n.start_position().row + 1,
                span: site(n).1,
                tautology: !counted,
                handler_line,
                detail: "recover() swallows the panic of a check".to_string(),
            });
        });
        true
    });
}

// ---------------------------------------------------------------------------
// Unit Tests (Positive and Negative Controls)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crate::ast::{AssertVocabulary, LanguagePack};

    fn test_at(line: usize, caught: &[(usize, usize)]) -> crate::ast::TestFn {
        crate::ast::TestFn {
            line,
            caught_assertions: caught
                .iter()
                .map(|&(line, handler_line)| super::CaughtAssertion {
                    line,
                    handler_line,
                    detail: "except AssertionError".to_string(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    fn new_lines(base: &crate::ast::TestFn, head: &crate::ast::TestFn) -> Vec<usize> {
        super::newly_caught(base, head)
            .iter()
            .map(|c| c.line)
            .collect()
    }

    #[test]
    fn a_swallowed_assertion_is_new_by_its_place_in_the_test_not_its_file_line() {
        // The whole test moved down three lines, then up: nothing is new.
        let base = test_at(1, &[(3, 4)]);
        assert_eq!(
            new_lines(&base, &test_at(4, &[(6, 7)])),
            Vec::<usize>::new()
        );
        assert_eq!(
            new_lines(&test_at(4, &[(6, 7)]), &base),
            Vec::<usize>::new()
        );
        // A handler added where there was none is new, on its head file line.
        assert_eq!(new_lines(&test_at(1, &[]), &test_at(4, &[(6, 7)])), vec![6]);
        // A new one that lands on the FILE line the old one had (the test moved down three
        // lines, the new one sits three lines below the old one's place) is still new.
        assert_eq!(
            new_lines(&test_at(1, &[(6, 7)]), &test_at(4, &[(6, 7), (9, 10)])),
            vec![6]
        );
    }

    #[test]
    fn a_second_swallowed_assertion_is_new_once_and_the_first_is_not() {
        let base = test_at(1, &[(3, 4)]);
        assert_eq!(new_lines(&base, &test_at(4, &[(6, 7), (10, 11)])), vec![10]);
        // The first one moved inside the test as well: it pairs by its distance to its
        // handler, and the one of another shape is the new one.
        assert_eq!(new_lines(&base, &test_at(1, &[(4, 5), (9, 11)])), vec![9]);
        // Both moved and of one shape: one is new, never two.
        assert_eq!(new_lines(&base, &test_at(1, &[(4, 5), (9, 10)])).len(), 1);
    }

    #[test]
    fn an_edit_inside_the_test_moves_a_swallowed_assertion_without_making_it_new() {
        // A line added inside the test above the swallowed assertion moves its offset.
        assert_eq!(
            new_lines(&test_at(1, &[(3, 4)]), &test_at(1, &[(4, 5)])),
            Vec::<usize>::new()
        );
        assert_eq!(
            new_lines(&test_at(1, &[(3, 4), (7, 8)]), &test_at(1, &[(7, 8)])),
            Vec::<usize>::new()
        );
    }

    /// One assertion stops being swallowed and another starts: as many as before, and
    /// the second is new.
    #[test]
    fn a_swallowed_assertion_traded_for_another_is_new() {
        assert_eq!(
            new_lines(&test_at(1, &[(3, 4)]), &test_at(1, &[(6, 9)])),
            vec![6]
        );
    }

    fn caught_lines(pack: &dyn LanguagePack, path: &str, src: &str) -> Vec<(usize, usize)> {
        let facts = pack
            .extract(path, src, &AssertVocabulary::default())
            .unwrap();
        facts
            .tests
            .iter()
            .flat_map(|t| t.caught_assertions.iter())
            .map(|c| (c.line, c.handler_line))
            .collect()
    }

    fn py(body: &str) -> Vec<(usize, usize)> {
        let src = format!("def test_a():\n{body}");
        caught_lines(&crate::ast::python::PythonPack, "tests/test_x.py", &src)
    }

    fn rs(body: &str) -> Vec<(usize, usize)> {
        let src = format!("#[test]\nfn t() {{\n{body}}}\n");
        caught_lines(&crate::ast::rust::RustPack, "src/lib.rs", &src)
    }

    fn js(body: &str) -> Vec<(usize, usize)> {
        let src = format!("test('a', (done) => {{\n{body}}});\n");
        caught_lines(&crate::ast::javascript::JavaScriptPack, "a.test.js", &src)
    }

    fn java(catch: &str) -> Vec<(usize, usize)> {
        let src = format!(
            "class ATest {{\n    @Test\n    void t() {{\n        try {{\n            assertEquals(4, add(2, 2));\n        }} {catch}\n    }}\n}}\n"
        );
        caught_lines(&crate::ast::java::JavaPack, "ATest.java", &src)
    }

    fn kotlin(body: &str) -> Vec<(usize, usize)> {
        let src = format!("class ATest {{\n    @Test\n    fun t() {{\n{body}    }}\n}}\n");
        caught_lines(&crate::ast::kotlin::KotlinPack, "ATest.kt", &src)
    }

    fn csharp(body: &str) -> Vec<(usize, usize)> {
        let src = format!(
            "public class ATests {{\n    [Fact]\n    public void T() {{\n{body}    }}\n}}\n"
        );
        caught_lines(&crate::ast::csharp::CSharpPack, "ATests.cs", &src)
    }

    fn go(handler: &str) -> Vec<(usize, usize)> {
        let src = format!(
            "package p\nfunc TestA(t *testing.T) {{\n\tdefer func() {{\n\t\tif r := recover(); r != nil {{\n\t\t\t{handler}\n\t\t}}\n\t}}()\n\tmustEqual(4, add(2, 2))\n}}\nfunc mustEqual(a, b int) {{\n\tif a != b {{\n\t\tpanic(\"not equal\")\n\t}}\n}}\n"
        );
        caught_lines(&crate::ast::r#go::GoPack, "p_test.go", &src)
    }

    const NONE: Vec<(usize, usize)> = Vec::new();

    #[test]
    fn handlers_are_read_in_order_and_the_first_that_always_catches_decides() {
        use super::{reach, swallowing_handler, Reach};
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .unwrap();
        let tree = crate::ast::source_text::parse(&mut parser, "a\nb\n").unwrap();
        let (first, second) = (
            tree.root_node().named_child(0).unwrap(),
            tree.root_node().named_child(1).unwrap(),
        );
        let pick = |a: (Reach, bool), b: (Reach, bool)| {
            swallowing_handler(&[(first, a.0, a.1), (second, b.0, b.1)])
                .map(|n| n.start_position().row)
        };
        // A handler that always catches and rethrows hides the swallowing one after it.
        assert_eq!(pick((Reach::Always, false), (Reach::Always, true)), None);
        assert_eq!(pick((Reach::Always, true), (Reach::Always, false)), Some(0));
        // One that may catch and rethrows does not.
        assert_eq!(pick((Reach::Maybe, false), (Reach::Always, true)), Some(1));
        assert_eq!(pick((Reach::Maybe, true), (Reach::Always, false)), Some(0));
        // One that never catches is passed over whatever its body does.
        assert_eq!(pick((Reach::Never, true), (Reach::Always, true)), Some(1));
        assert_eq!(pick((Reach::Never, true), (Reach::Never, true)), None);

        let (ancestors, unrelated) = (&["AssertionError"][..], &["IOException"][..]);
        assert_eq!(
            reach(&["AssertionError"], ancestors, unrelated),
            Reach::Always
        );
        assert_eq!(reach(&["IOException"], ancestors, unrelated), Reach::Never);
        assert_eq!(reach(&["CheckFailure"], ancestors, unrelated), Reach::Maybe);
        // A multi-catch reaches as far as its widest type.
        assert_eq!(
            reach(&["IOException", "AssertionError"], ancestors, unrelated),
            Reach::Always
        );
        assert_eq!(
            reach(&["IOException", "CheckFailure"], ancestors, unrelated),
            Reach::Maybe
        );
    }

    #[test]
    fn python_except_types_are_read_from_the_clause_node() {
        let multi_line =
            "    try:\n        assert f()\n    except (\n        ValueError,\n        AssertionError,\n    ):\n        pass\n";
        assert_eq!(py(multi_line), vec![(3, 4)]);
        assert_eq!(
            py("    try:\n        assert f()\n    except* AssertionError:\n        pass\n"),
            vec![(3, 4)]
        );
        assert_eq!(
            py("    try:\n        assert f()\n    except builtins.Exception as e:\n        print(e)\n"),
            vec![(3, 4)]
        );
        // Controls: tuples and groups of other types.
        assert_eq!(
            py("    try:\n        assert f()\n    except (\n        ValueError,\n        KeyError,\n    ):\n        pass\n"),
            NONE
        );
        assert_eq!(
            py("    try:\n        assert f()\n    except* ValueError:\n        pass\n"),
            NONE
        );
    }

    #[test]
    fn python_suppress_is_a_handler_and_an_assertion_is_neutralized_once() {
        assert_eq!(
            py("    with contextlib.suppress(AssertionError):\n        assert f()\n"),
            vec![(3, 2)]
        );
        assert_eq!(
            py("    with open(p) as fh, suppress(Exception):\n        assert f()\n"),
            vec![(3, 2)]
        );
        assert_eq!(
            py("    with suppress(KeyError):\n        assert f()\n"),
            NONE
        );
        assert_eq!(py("    with open(p) as fh:\n        assert f()\n"), NONE);
        // Inside both a swallowing `try` and a `suppress`: one entry, for the nearest.
        assert_eq!(
            py("    with suppress(AssertionError):\n        try:\n            assert f()\n        except AssertionError:\n            pass\n"),
            vec![(4, 5)]
        );
    }

    #[test]
    fn python_soft_assertions_and_retry_loops_are_read_from_what_follows() {
        let collect = "    errors = []\n    try:\n        assert f()\n    except AssertionError as e:\n        errors.append(e)\n";
        assert_eq!(py(&format!("{collect}    assert not errors\n")), NONE);
        assert_eq!(py(collect), vec![(4, 5)]);
        assert_eq!(
            py(&format!("{collect}    assert not others\n")),
            vec![(4, 5)]
        );
        // A loop around the `try` names the list and holds the assertion: not a check.
        assert_eq!(
            py("    errors = []\n    for case in cases:\n        try:\n            assert f(case)\n        except AssertionError as e:\n            errors.append(e)\n        log(case)\n"),
            vec![(5, 6)]
        );
        assert_eq!(
            py("    errors = []\n    for case in cases:\n        try:\n            assert f(case)\n        except AssertionError as e:\n            errors.append(e)\n    assert not errors\n"),
            NONE
        );
        // A statement before the `try` that names the list does not count.
        assert_eq!(
            py("    errors = []\n    assert not errors\n    try:\n        assert f()\n    except AssertionError as e:\n        errors.append(e)\n"),
            vec![(5, 6)]
        );

        let retry = "    for _ in range(3):\n        try:\n            assert f()\n            break\n        except AssertionError:\n            backoff()\n";
        assert_eq!(
            py(&format!(
                "{retry}    else:\n        pytest.fail(\"never\")\n"
            )),
            NONE
        );
        assert_eq!(py(retry), vec![(4, 6)]);
        // The loop's `else` does not fail.
        assert_eq!(
            py(&format!("{retry}    else:\n        print(\"never\")\n")),
            vec![(4, 6)]
        );
        // No way out of the loop on success: the `else` always runs.
        assert_eq!(
            py("    for _ in range(3):\n        try:\n            assert f()\n        except AssertionError:\n            backoff()\n    else:\n        pytest.fail(\"never\")\n"),
            vec![(4, 5)]
        );
    }

    const UNWIND: &str = "    let r = std::panic::catch_unwind(|| assert_eq!(add(2, 2), 4));\n";

    #[test]
    fn rust_result_use_is_read_from_the_tree_not_the_text() {
        for checked in [
            "    assert!(matches!(r, Err(_)));\n",
            "    r.unwrap();\n",
            "    if let Err(e) = r {\n        std::panic::resume_unwind(e);\n    }\n",
            "    match r {\n        Ok(()) => {}\n        Err(e) => std::panic::resume_unwind(e),\n    }\n",
            "    let failed = r.is_err();\n    assert!(failed);\n",
        ] {
            assert_eq!(rs(&format!("{UNWIND}{checked}")), NONE, "{checked}");
        }
        for dropped in [
            "    // r.is_err() is not looked at\n",
            "    let other_r: Result<(), ()> = Ok(());\n    assert!(other_r.is_ok());\n",
            "    let _ = r;\n",
            "    drop(r);\n",
            "    println!(\"{:?}\", r);\n",
            "    match r {\n        Ok(()) => {}\n        Err(_) => {}\n    }\n",
            // The arm that fails is the one taken when the assertion held.
            "    match r {\n        Ok(()) => panic!(\"no\"),\n        Err(_) => {}\n    }\n",
            "    let failed = r.is_err();\n",
        ] {
            assert_eq!(rs(&format!("{UNWIND}{dropped}")), vec![(3, 3)], "{dropped}");
        }
    }

    /// A later `let` of the same name is a use of the name: the earlier binding is
    /// looked at when the later one is. Each binding is judged by the uses after it.
    #[test]
    fn rust_bindings_of_one_name_are_each_judged_by_the_uses_after_them() {
        // Neither is looked at: both assertions are caught.
        assert_eq!(rs(&format!("{UNWIND}{UNWIND}")), vec![(3, 3), (4, 4)]);
        // The second is looked at, and the first is read as handed on to it.
        assert_eq!(
            rs(&format!("{UNWIND}{UNWIND}    assert!(r.is_ok());\n")),
            NONE
        );
        // The first is looked at before the second is bound, the second never.
        assert_eq!(
            rs(&format!("{UNWIND}    assert!(r.is_ok());\n{UNWIND}")),
            vec![(5, 5)]
        );
        // Another name in between is judged by its own uses.
        let other = "    let s = std::panic::catch_unwind(|| assert!(f()));\n";
        assert_eq!(
            rs(&format!("{UNWIND}{other}{UNWIND}    drop(s);\n")),
            vec![(3, 3), (4, 4), (5, 5)]
        );
        // A use in a function inside the test is a use in the test's function.
        assert_eq!(
            rs(&format!(
                "{UNWIND}    fn inner() {{\n        let r = 1;\n        assert!(r == 1);\n    }}\n"
            )),
            NONE
        );
        // A binding in a function inside the test is judged by the uses in that
        // function: one after it in the test is of another `r`.
        assert_eq!(
            rs(&format!(
                "    fn inner() {{\n    {UNWIND}    }}\n    let r = 1;\n    assert!(r == 1);\n"
            )),
            vec![(4, 4)]
        );
        // Twelve bindings of one name with nothing looking at any: twelve are caught.
        let twelve: Vec<(usize, usize)> = (3..15).map(|line| (line, line)).collect();
        assert_eq!(rs(&UNWIND.repeat(12)), twelve);
    }

    /// The steps one extraction of a Rust test with `body` counts.
    fn rs_steps(body: &str) -> u64 {
        let src = format!("#[test]\nfn t() {{\n{body}}}\n");
        let (facts, counted) = crate::ast::ancestry::steps(|| {
            crate::ast::rust::RustPack.extract("tests/t.rs", &src, &AssertVocabulary::default())
        });
        facts.unwrap();
        counted
    }

    /// Unwind results bound and not looked at cost steps in proportion to their number.
    /// Before #672 the function was walked for each binding, so four times the bindings
    /// cost sixteen times the steps; and a later binding of the same name was asked
    /// again by every earlier one, so each binding more of one name doubled the steps:
    /// fourteen cost nineteen times what ten did.
    #[test]
    fn rust_unwind_results_cost_steps_in_proportion_to_their_number() {
        let named = |n: usize| -> String {
            (0..n)
                .map(|i| format!("    let r{i} = std::panic::catch_unwind(|| assert!(f({i})));\n"))
                .collect()
        };
        let (few, many) = (rs_steps(&named(40)), rs_steps(&named(160)));
        assert!(many < 5 * few, "{few} steps for 40 names, {many} for 160");
        let (ten, fourteen) = (rs_steps(&UNWIND.repeat(10)), rs_steps(&UNWIND.repeat(14)));
        assert!(
            fourteen < 2 * ten,
            "{ten} steps for 10 bindings of one name, {fourteen} for 14"
        );
    }

    /// The steps one extraction of `src` under `path` counts, and how many assertions
    /// it read as caught.
    fn steps_and_caught(path: &str, src: &str) -> (u64, usize) {
        let registry = crate::ast::default_registry();
        let pack = registry.find_pack(path).expect("a pack for the path");
        let (facts, counted) =
            crate::ast::ancestry::steps(|| pack.extract(path, src, &AssertVocabulary::default()));
        let caught = facts
            .unwrap()
            .tests
            .iter()
            .map(|t| t.caught_assertions.len())
            .sum();
        (counted, caught)
    }

    /// A handler is read once for the assertions it catches. Before #672 a Python `try`
    /// had its handlers read again for each assertion in its body, and each reading
    /// walked the handler, the body of a retried `try`, and the statements after a
    /// `try` that keeps its error, once for each line that kept it: four times the lines
    /// cost 13 to 60 times the steps. A JavaScript `.catch(name)` read the whole file
    /// for the function `name` each time.
    #[test]
    fn handlers_cost_steps_in_proportion_to_the_assertions_they_hold() {
        type Source = (&'static str, &'static str, fn(usize) -> String);
        let sources: [Source; 4] = [
            (
                "a Python handler that keeps its error",
                "tests/test_m.py",
                |n| {
                    let lines =
                        |line: fn(usize) -> String| -> String { (0..n).map(line).collect() };
                    format!(
                        "def test_x():\n    errs = []\n    try:\n{}    except AssertionError as e:\n{}{}",
                        lines(|i| format!("        assert a == {i}\n")),
                        lines(|_| "        errs.append(e)\n".to_string()),
                        lines(|i| format!("    x{i} = {i}\n"))
                    )
                },
            ),
            (
                "a Python handler of many statements",
                "tests/test_m.py",
                |n| {
                    let lines =
                        |line: fn(usize) -> String| -> String { (0..n).map(line).collect() };
                    format!(
                        "def test_x():\n    try:\n{}    except AssertionError:\n{}",
                        lines(|i| format!("        assert a == {i}\n")),
                        lines(|i| format!("        print({i})\n"))
                    )
                },
            ),
            ("a Python try in a loop", "tests/test_m.py", |n| {
                let asserts: String = (0..n)
                    .map(|i| format!("            assert a == {i}\n"))
                    .collect();
                format!(
                        "def test_x():\n    for i in range(3):\n        try:\n{asserts}        except AssertionError:\n            pass\n"
                    )
            }),
            (
                "JavaScript handlers passed by name",
                "tests/m.test.js",
                |n| {
                    let chains: String = (0..n)
                        .map(|i| {
                            format!("  p{i}.then(() => {{ expect(a).toBe({i}); }}).catch(quiet);\n")
                        })
                        .collect();
                    format!("function quiet() {{}}\ntest('t', () => {{\n{chains}}});\n")
                },
            ),
        ];
        for (name, path, source) in sources {
            let ((few, held), (many, more)) = (
                steps_and_caught(path, &source(40)),
                steps_and_caught(path, &source(160)),
            );
            assert_eq!((held, more), (40, 160), "{name}");
            assert!(many < 5 * few, "{name}: {few} steps for 40, {many} for 160");
        }
    }

    /// Each Python `try` is read by its own handlers, and each JavaScript `.catch(name)`
    /// by the function of its own name.
    #[test]
    fn a_handler_read_once_is_read_for_its_own_try_and_its_own_name() {
        assert_eq!(
            py("    try:\n        assert a\n    except AssertionError:\n        pass\n    try:\n        assert b\n    except AssertionError:\n        raise\n    try:\n        assert c\n    except AssertionError:\n        pass\n"),
            vec![(3, 4), (11, 12)]
        );
        let src = "function quiet() {}\nfunction loud(e) { throw e; }\ntest('a', () => {\n  p.then(() => { expect(a).toBe(1); }).catch(quiet);\n  q.then(() => { expect(a).toBe(2); }).catch(loud);\n  r.then(() => { expect(a).toBe(3); }).catch(quiet);\n});\n";
        assert_eq!(
            caught_lines(&crate::ast::javascript::JavaScriptPack, "a.test.js", src),
            vec![(4, 4), (6, 6)]
        );
    }

    /// A closure `catch_unwind` runs inside another one holds some of the assertions of
    /// the other: each is caught once, by the outermost call whose result nothing looks
    /// at, and a function declared in a closure keeps its own.
    #[test]
    fn rust_closures_one_inside_another_are_each_read_by_their_own_assertions() {
        // Both results thrown away: the inner assertion is the outer call's too.
        assert_eq!(
            rs("    let _ = std::panic::catch_unwind(|| {\n        assert!(a());\n        let _ = std::panic::catch_unwind(|| {\n            assert!(b());\n        });\n    });\n"),
            vec![(4, 3), (6, 3)]
        );
        // The outer result is looked at, the inner one is not.
        assert_eq!(
            rs("    let r = std::panic::catch_unwind(|| {\n        assert!(a());\n        let _ = std::panic::catch_unwind(|| {\n            assert!(b());\n        });\n    });\n    r.unwrap();\n"),
            vec![(6, 5)]
        );
        // The inner result is looked at, the outer one is not: both are the outer's.
        assert_eq!(
            rs("    let _ = std::panic::catch_unwind(|| {\n        assert!(a());\n        let r = std::panic::catch_unwind(|| {\n            assert!(b());\n        });\n        r.unwrap();\n    });\n"),
            vec![(4, 3), (6, 3), (8, 3)]
        );
        // A function declared in the closure is not run by it: the assertion of the
        // call in that function is caught by that call, and by no other.
        assert_eq!(
            rs("    let _ = std::panic::catch_unwind(|| {\n        assert!(a());\n        fn inner() {\n            let _ = std::panic::catch_unwind(|| {\n                assert!(b());\n            });\n        }\n        assert!(c());\n    });\n"),
            vec![(4, 3), (10, 3), (7, 6)]
        );
        // Two calls side by side, and a third after a closure that held one.
        assert_eq!(
            rs("    let _ = std::panic::catch_unwind(|| {\n        let _ = std::panic::catch_unwind(|| assert!(a()));\n    });\n    let _ = std::panic::catch_unwind(|| assert!(b()));\n    let _ = std::panic::catch_unwind(|| assert!(c()));\n"),
            vec![(4, 3), (6, 6), (7, 7)]
        );
        // A closure with no assertion of its own around one that has some.
        assert_eq!(
            rs("    let r = std::panic::catch_unwind(|| {\n        fn inner() {\n            assert!(a());\n        }\n    });\n"),
            NONE
        );
    }

    /// Closures `catch_unwind` runs, one inside another, cost steps in proportion to
    /// their number. Before #672 each closure was walked for its own call and again for
    /// every call around it, so four times the depth cost about sixteen times the steps.
    #[test]
    fn rust_unwind_closures_one_inside_another_cost_steps_in_proportion_to_their_number() {
        let nested = |n: usize| -> String {
            let open: String = (0..n)
                .map(|i| {
                    format!("let r{i} = std::panic::catch_unwind(|| {{ assert_eq!(f({i}), {i});\n")
                })
                .collect();
            format!("{open}{}", "});\n".repeat(n))
        };
        let (few, many) = (rs_steps(&nested(40)), rs_steps(&nested(160)));
        assert_eq!(rs(&nested(40)).len(), 40);
        assert!(many < 5 * few, "{few} steps for 40 deep, {many} for 160");
    }

    /// A Kotlin `runCatching` result bound to a name is looked at when the name is used
    /// after the binding in the function or lambda that holds it, and nowhere else.
    #[test]
    fn kotlin_run_catching_bound_to_a_name_is_judged_by_the_uses_in_its_own_function() {
        let caught = |src: &str| caught_lines(&crate::ast::kotlin::KotlinPack, "ATest.kt", src);
        let bind = "val r = runCatching { assertEquals(4, add(2, 2)) }\n";
        // Used after the binding.
        assert_eq!(kotlin(&format!("        {bind}        check(r)\n")), NONE);
        // Never used, and used only before the binding.
        assert_eq!(kotlin(&format!("        {bind}")), vec![(4, 4)]);
        assert_eq!(
            kotlin(&format!("        check(r)\n        {bind}")),
            vec![(5, 5)]
        );
        // Another name used after it.
        assert_eq!(
            kotlin(&format!("        {bind}        check(other)\n")),
            vec![(4, 4)]
        );
        // Two bindings of one name: the first is followed by the second's name, the
        // second by nothing.
        assert_eq!(
            kotlin(&format!("        {bind}        {bind}")),
            vec![(5, 5)]
        );
        // The same name in two functions: a use in one is not a use in the other.
        assert_eq!(
            caught(&format!(
                "class ATest {{\n    @Test\n    fun t() {{\n        {bind}    }}\n    @Test\n    fun u() {{\n        {bind}        check(r)\n    }}\n}}\n"
            )),
            vec![(4, 4)]
        );
        assert_eq!(
            caught(&format!(
                "class ATest {{\n    @Test\n    fun t() {{\n        {bind}        check(r)\n    }}\n    @Test\n    fun u() {{\n        {bind}    }}\n}}\n"
            )),
            vec![(9, 9)]
        );
        // A binding in a lambda is judged by the uses in the lambda: one after the
        // lambda is of another name.
        assert_eq!(
            kotlin(&format!(
                "        items.forEach {{\n            {bind}        }}\n        check(r)\n"
            )),
            vec![(5, 5)]
        );
        assert_eq!(
            kotlin(&format!(
                "        items.forEach {{\n            {bind}            check(r)\n        }}\n"
            )),
            NONE
        );
    }

    /// `runCatching` results bound and not used cost steps in proportion to their
    /// number. Before #672 the function was walked for each binding, so four times the
    /// bindings cost about sixteen times the steps.
    #[test]
    fn kotlin_run_catching_results_cost_steps_in_proportion_to_their_number() {
        let source = |n: usize| -> String {
            let body: String = (0..n)
                .map(|i| format!("        val r{i} = runCatching {{ assertEquals(1, f({i})) }}\n"))
                .collect();
            format!("class MTest {{\n    @Test\n    fun t() {{\n{body}    }}\n}}\n")
        };
        let path = "src/test/kotlin/MTest.kt";
        let ((few, held), (many, more)) = (
            steps_and_caught(path, &source(40)),
            steps_and_caught(path, &source(160)),
        );
        assert_eq!((held, more), (40, 160));
        assert!(many < 5 * few, "{few} steps for 40, {many} for 160");
    }

    /// A chain of promise calls holds a `.catch()` for each link: an assertion is caught
    /// by the outermost `.catch()` around it that swallows, and a chain that was read is
    /// not what the next one is read by.
    #[test]
    fn javascript_catch_calls_of_one_chain_each_catch_the_assertions_before_them() {
        let then = |i: usize| format!(".then(() => {{ expect(a).toBe({i}); }})");
        let (quiet, loud) = (".catch(() => {})", ".catch((e) => { throw e; })");
        // Both swallow: both assertions are the outer one's.
        assert_eq!(
            js(&format!(
                "  p\n    {}\n    {quiet}\n    {}\n    {quiet};\n",
                then(1),
                then(2)
            )),
            vec![(3, 6), (5, 6)]
        );
        // The outer one rethrows: the inner one catches what stands before it.
        assert_eq!(
            js(&format!(
                "  p\n    {}\n    {quiet}\n    {}\n    {loud};\n",
                then(1),
                then(2)
            )),
            vec![(3, 4)]
        );
        // The inner one rethrows: the outer one catches both.
        assert_eq!(
            js(&format!(
                "  p\n    {}\n    {loud}\n    {}\n    {quiet};\n",
                then(1),
                then(2)
            )),
            vec![(3, 6), (5, 6)]
        );
        // Two chains, and a third: each is read by its own `.catch()`.
        assert_eq!(
            js(&format!(
                "  p{}{quiet}{}{quiet};\n  q{}{quiet};\n  r{}{loud};\n  s{}{quiet};\n",
                then(1),
                then(2),
                then(3),
                then(4),
                then(5)
            )),
            vec![(2, 2), (2, 2), (3, 3), (5, 5)]
        );
        // A chain in the handler of another is outside the chain that was read.
        assert_eq!(
            js(&format!(
                "  p{}.catch(() => {{\n    q{}{quiet};\n  }});\n",
                then(1),
                then(2)
            )),
            vec![(2, 2), (3, 3)]
        );
    }

    /// A chain of `.then().catch()` links costs steps in proportion to their number.
    /// Before #672 the chain was walked once for each `.catch()` of it, so four times
    /// the links cost about sixteen times the steps.
    #[test]
    fn javascript_promise_chains_cost_steps_in_proportion_to_their_links() {
        let source = |n: usize| -> String {
            let links: String = (0..n)
                .map(|i| format!(".then(() => {{ expect(a).toBe({i}); }}).catch(() => {{}})"))
                .collect();
            format!("test('t', () => {{\n  p{links};\n}});\n")
        };
        let path = "tests/m.test.js";
        let ((few, held), (many, more)) = (
            steps_and_caught(path, &source(40)),
            steps_and_caught(path, &source(160)),
        );
        assert_eq!((held, more), (40, 160));
        assert!(many < 5 * few, "{few} steps for 40, {many} for 160");
    }

    #[test]
    fn rust_returned_result_and_wrapped_closure() {
        let returned = "#[test]\nfn t() -> std::thread::Result<()> {\n    std::panic::catch_unwind(|| assert!(f()))\n}\n";
        assert_eq!(
            caught_lines(&crate::ast::rust::RustPack, "src/lib.rs", returned),
            NONE
        );
        let wrapped = "std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| assert!(f())))";
        assert_eq!(rs(&format!("    let _ = {wrapped};\n")), vec![(3, 3)]);
        assert_eq!(
            rs(&format!(
                "    let r = {wrapped};\n    assert!(r.is_err());\n"
            )),
            NONE
        );
        // Another wrapper is not known to run the closure.
        assert_eq!(
            rs("    let _ = std::panic::catch_unwind(later(|| assert!(f())));\n"),
            NONE
        );
    }

    #[test]
    fn javascript_done_reject_and_promise_catch() {
        let try_with = |handler: &str| {
            js(&format!(
                "  try {{\n    expect(add(2, 2)).toBe(4);\n  }} catch (e) {{\n    {handler}\n  }}\n"
            ))
        };
        assert_eq!(try_with("done(e);"), NONE);
        assert_eq!(try_with("reject(e);"), NONE);
        assert_eq!(try_with("done();"), vec![(3, 4)]);
        assert_eq!(try_with("resolve();"), vec![(3, 4)]);

        let then = "  return load().then((v) => expect(v).toBe(4))";
        assert_eq!(js(&format!("{then}.catch(() => {{}});\n")), vec![(2, 2)]);
        assert_eq!(
            js(&format!("{then}.catch((e) => console.log(e));\n")),
            vec![(2, 2)]
        );
        assert_eq!(js(&format!("{then}.catch((e) => {{ throw e; }});\n")), NONE);
        assert_eq!(js(&format!("{then}.catch((e) => done(e));\n")), NONE);
        assert_eq!(js(&format!("{then}.catch(done);\n")), NONE);
        assert_eq!(js(&format!("{then}.finally(() => {{}});\n")), NONE);
        assert_eq!(js("  return load().catch(() => {});\n"), NONE);
        // Inside a swallowing `try` as well: the assertion is neutralized once.
        assert_eq!(
            js("  try {\n    expect(load()).resolves.toBe(4)\n      .catch(() => {});\n  } catch (e) {}\n").len(),
            1
        );
    }

    #[test]
    fn java_catch_types_are_read_from_the_type_node() {
        for unrelated in [
            "catch (InterruptedException e) {\n        }",
            "catch (Exception e) {\n        }",
            "catch (java.io.IOException myError) {\n        }",
            "catch (IllegalStateException | java.io.IOException e) {\n        }",
            // The first handler that always catches the failure rethrows it.
            "catch (AssertionError e) {\n            throw e;\n        } catch (Throwable t) {\n        }",
        ] {
            assert_eq!(java(unrelated), NONE, "{unrelated}");
        }
        for catching in [
            "catch (Throwable t) {\n        }",
            "catch (final java.lang.AssertionError e) {\n        }",
            "catch (IllegalStateException | AssertionError e) {\n        }",
            // A type that is not known is reported.
            "catch (CheckFailure e) {\n        }",
            "catch (CheckFailure e) {\n            throw e;\n        } catch (Error e) {\n        }",
        ] {
            assert_eq!(java(catching).len(), 1, "{catching}");
        }
        let with_resources = "class ATest {\n    @Test\n    void t() {\n        try (AutoCloseable r = open()) {\n            assertEquals(4, add(2, 2));\n        } catch (AssertionError e) {\n        }\n    }\n}\n";
        assert_eq!(
            caught_lines(&crate::ast::java::JavaPack, "ATest.java", with_resources),
            vec![(5, 6)]
        );
    }

    #[test]
    fn kotlin_try_catch_and_run_catching() {
        let try_with = |catch: &str| {
            kotlin(&format!(
                "        try {{\n            assertEquals(4, add(2, 2))\n        }} {catch}\n"
            ))
        };
        assert_eq!(
            try_with("catch (e: AssertionError) {\n        }"),
            vec![(5, 6)]
        );
        assert_eq!(try_with("catch (e: Throwable) {\n        }"), vec![(5, 6)]);
        assert_eq!(
            try_with("catch (e: CheckFailure) {\n        }"),
            vec![(5, 6)]
        );
        assert_eq!(try_with("catch (e: Exception) {\n        }"), NONE);
        assert_eq!(
            try_with("catch (e: AssertionError) {\n            throw e\n        }"),
            NONE
        );
        assert_eq!(try_with("finally {\n        }"), NONE);
        // Kotest matchers are assertions too.
        assert_eq!(
            kotlin("        try {\n            add(2, 2) shouldBe 4\n        } catch (e: AssertionError) {\n        }\n"),
            vec![(5, 6)]
        );

        let block = "runCatching {\n            assertEquals(4, add(2, 2))\n        }";
        assert_eq!(kotlin(&format!("        {block}\n")), vec![(5, 4)]);
        assert_eq!(
            kotlin(&format!("        {block}.getOrNull()\n")),
            vec![(5, 4)]
        );
        assert_eq!(kotlin(&format!("        val r = {block}\n")), vec![(5, 4)]);
        assert_eq!(kotlin(&format!("        {block}.getOrThrow()\n")), NONE);
        assert_eq!(
            kotlin(&format!("        {block}.onFailure {{ throw it }}\n")),
            NONE
        );
        assert_eq!(
            kotlin(&format!(
                "        val r = {block}\n        assertTrue(r.isFailure)\n"
            )),
            NONE
        );
    }

    #[test]
    fn csharp_types_and_assertions_are_read_from_their_nodes() {
        let try_with = |catch: &str| {
            csharp(&format!(
                "        try {{\n            Assert.Equal(4, Add(2, 2));\n        }} {catch}\n"
            ))
        };
        assert_eq!(try_with("catch (IOException) {\n        }"), NONE);
        assert_eq!(
            try_with("catch (System.IO.IOException e) {\n        }"),
            NONE
        );
        assert_eq!(try_with("catch {\n        }"), vec![(5, 6)]);
        assert_eq!(try_with("catch (CheckFailure) {\n        }"), vec![(5, 6)]);
        assert_eq!(
            try_with("catch (Exception e) when (e.Message != null) {\n            throw;\n        } catch {\n        }"),
            vec![(5, 8)]
        );
        assert_eq!(
            try_with("catch (Exception) {\n            throw;\n        } catch {\n        }"),
            NONE
        );
        // Text that mentions an assertion is not one, in the body or in the handler.
        assert_eq!(
            csharp("        try {\n            Console.WriteLine(\"Assert.Equal done\");\n        } catch (Exception) {\n        }\n"),
            NONE
        );
        assert_eq!(
            try_with("catch (Exception e) {\n            Console.WriteLine(\"Assert.Fail \" + e);\n        }"),
            vec![(5, 6)]
        );
        assert_eq!(
            try_with("catch (Exception e) {\n            Assert.Fail(e.Message);\n        }"),
            NONE
        );
        // A project's own `...Assert` class is one; a class merely containing the word is not.
        assert_eq!(
            csharp("        try {\n            MoneyAssert.Equal(4, Add(2, 2));\n        } catch (Exception) {\n        }\n"),
            vec![(5, 6)]
        );
        assert_eq!(
            csharp("        try {\n            AssertLog.Write(4);\n        } catch (Exception) {\n        }\n"),
            NONE
        );
        // An expected-exception call is not a swallowed assertion.
        assert_eq!(
            csharp("        try {\n            Assert.Throws<IOException>(() => Add(2, 2));\n        } catch (Exception) {\n        }\n"),
            NONE
        );
    }

    #[test]
    fn go_deferred_function_that_marks_the_test_failed_is_not_a_swallowing_handler() {
        assert_eq!(go("t.Fail()"), NONE);
        assert_eq!(go("require.Fail(t, \"panicked\")"), NONE);
        assert_eq!(go("assert.Failf(t, \"panicked\", \"%v\", r)"), NONE);
    }

    /// #627: a check that panics is caught by a deferred `recover()` that only logs or
    /// formats the error, and not by one that fails the test.
    #[test]
    fn go_recovering_defer_swallows_a_panicking_check_unless_it_fails_the_test() {
        assert_eq!(go("log.Println(r)"), vec![(8, 3)]);
        assert_eq!(go("t.Fatal(r)"), NONE);
        assert_eq!(go("log.Println(r.(error).Error())"), vec![(8, 3)]);
    }

    /// A Go test whose deferred function runs `deferred` and whose body then runs `check`;
    /// `mustEqual` is a function of the file that panics.
    fn go_with(deferred: &str, check: &str) -> Vec<(usize, usize)> {
        let src = format!(
            "package p\nfunc TestA(t *testing.T) {{\n\tdefer func() {{\n\t\t{deferred}\n\t}}()\n\t{check}\n}}\nfunc mustEqual(a, b int) {{\n\tif a != b {{\n\t\tpanic(\"not equal\")\n\t}}\n}}\n"
        );
        caught_lines(&crate::ast::r#go::GoPack, "p_test.go", &src)
    }

    /// #627: `require`, `assert` and `t.Fatal` do not panic, so a `recover()` does not
    /// catch them; a check that panics is caught unless the deferred function fails
    /// the test.
    #[test]
    fn go_recover_catches_a_check_that_panics_and_nothing_else() {
        const PANICS: &str = "mustEqual(4, add(2, 2))";
        for not_caught in [
            "require.Equal(t, 4, add(2, 2))",
            "assert.Equal(t, 4, add(2, 2))",
            "if add(2, 2) != 4 {\n\t\tt.Fatal(\"no\")\n\t}",
            "other(4, add(2, 2))",
        ] {
            assert_eq!(go_with("_ = recover()", not_caught), NONE, "{not_caught}");
        }
        assert_eq!(go_with("_ = recover()", PANICS), vec![(6, 3)]);
        assert_eq!(
            go_with(
                "_ = recover()",
                "if add(2, 2) != 4 {\n\t\tpanic(\"no\")\n\t}"
            ),
            vec![(7, 3)]
        );
        for swallowing in [
            "log.Println(recover())",
            "if r := recover(); r != nil {\n\t\t\tlog.Println(r.(error).Error())\n\t\t}",
            "_ = fmt.Errorf(\"%v\", recover())",
            // `err.Error()` takes no argument: it formats, where `t.Error(r)` reports.
            "err := asError(recover())\n\t\tlog.Println(err.Error())",
        ] {
            assert_eq!(go_with(swallowing, PANICS).len(), 1, "{swallowing}");
        }
        for failing in [
            "if r := recover(); r != nil {\n\t\t\tt.Fatal(r)\n\t\t}",
            "if r := recover(); r != nil {\n\t\t\tt.Error(r)\n\t\t}",
            "if r := recover(); r != nil {\n\t\t\tt.Errorf(\"%v\", r)\n\t\t}",
            "if r := recover(); r != nil {\n\t\t\tpanic(r)\n\t\t}",
            "require.Nil(t, recover())",
            "s.Require().Nil(recover())",
            // No `recover()`: the panic is not caught at all.
            "cleanup()",
            // A `recover()` in a function literal of the deferred function recovers nothing.
            "go func() {\n\t\t\t_ = recover()\n\t\t}()",
        ] {
            assert_eq!(go_with(failing, PANICS), NONE, "{failing}");
        }
        // A subtest is a function of its own: the `defer` of the test around it does not
        // catch what the subtest's function panics with.
        assert_eq!(
            go_with(
                "_ = recover()",
                "t.Run(\"sub\", func(t *testing.T) {\n\t\tmustEqual(4, add(2, 2))\n\t})"
            ),
            NONE
        );
    }

    /// #627: a class is read by what it may be an instance of.
    #[test]
    fn python_class_reach_is_read_from_the_tables_and_the_bases_of_the_file() {
        use super::{py_name_reach, PyClasses, Reach};
        let mut classes = PyClasses::new();
        classes.insert("FromValue", vec!["ValueError"]);
        classes.insert("FromException", vec!["Exception"]);
        classes.insert("FromAssertion", vec!["AssertionError"]);
        classes.insert("Chained", vec!["FromValue"]);
        classes.insert("Mixed", vec!["FromValue", "FromAssertion"]);
        classes.insert("FromElsewhere", vec!["Imported"]);
        classes.insert("Computed", vec![""]);
        classes.insert("Loop", vec!["Loop"]);
        classes.insert("Plain", vec![]);
        let reach = |name: &str| py_name_reach(name, &classes, &mut Vec::new());
        for always in ["AssertionError", "Exception", "BaseException"] {
            assert_eq!(reach(always), Reach::Always, "{always}");
        }
        for never in [
            "KeyError",
            "OSError",
            "IOError",
            "JSONDecodeError",
            "FromValue",
            "FromException",
            "Chained",
            "Plain",
        ] {
            assert_eq!(reach(never), Reach::Never, "{never}");
        }
        for maybe in [
            "CheckFailed",
            "FromAssertion",
            "Mixed",
            "FromElsewhere",
            "Computed",
            "Loop",
        ] {
            assert_eq!(reach(maybe), Reach::Maybe, "{maybe}");
        }
    }

    /// #627: a Python handler that may catch the failure is reported only when its body
    /// does nothing with it.
    #[test]
    fn python_handler_for_an_unknown_class_is_reported_only_when_it_plainly_swallows() {
        let with = |handler: &str| py(&format!("    try:\n        assert f()\n{handler}"));
        for swallowing in [
            "    except CheckFailed:\n        pass\n",
            "    except CheckFailed:\n        ...\n",
            "    except CheckFailed as e:\n        print(e)\n",
            "    except CheckFailed as e:\n        self.log.error(e)\n",
            "    except CheckFailed as e:\n        warnings.warn(str(e))\n",
            "    except (KeyError, mod.CheckFailed):\n        pass\n",
        ] {
            assert_eq!(with(swallowing), vec![(3, 4)], "{swallowing}");
        }
        for other in [
            "    except CheckFailed:\n        raise\n",
            "    except CheckFailed:\n        return\n",
            "    except CheckFailed as e:\n        record(e)\n",
            "    except CheckFailed as e:\n        dialog.error(e)\n",
            "    except CheckFailed as e:\n        print(e)\n        seen = True\n",
            "    except KeyError:\n        pass\n",
        ] {
            assert_eq!(with(other), NONE, "{other}");
        }
        // A handler that always catches the failure is judged as before.
        assert_eq!(
            with("    except AssertionError as e:\n        record(e)\n"),
            vec![(3, 4)]
        );
        // A handler that may catch it and does not plainly swallow is passed over: the
        // next one decides.
        assert_eq!(
            with("    except CheckFailed:\n        raise\n    except Exception:\n        pass\n"),
            vec![(3, 6)]
        );
    }

    #[test]
    fn python_positive_controls() {
        let src = r#"
def test_py_bare_except():
    try:
        assert 1 == 2
    except:
        pass

def test_py_assertion_error():
    try:
        assert add(2, 2) == 4
    except AssertionError:
        pass

def test_py_tuple_except():
    try:
        self.assertEqual(a, b)
    except (ValueError, AssertionError):
        pass
"#;
        let caught = caught_lines(&crate::ast::python::PythonPack, "tests/test_x.py", src);
        assert_eq!(caught, vec![(4, 5), (10, 11), (16, 17)]);
    }

    #[test]
    fn python_negative_controls() {
        let src = r#"
import pytest

def test_raises_context_manager():
    with pytest.raises(ValueError):
        add(2, 2)

def test_reraise():
    try:
        assert 1 == 2
    except AssertionError:
        raise

def test_fail_call():
    try:
        assert 1 == 2
    except AssertionError:
        pytest.fail("failed")

def test_assert_in_handler():
    try:
        assert 1 == 2
    except AssertionError as e:
        assert "1 == 2" in str(e)

def test_other_exception():
    try:
        assert 1 == 2
    except ValueError:
        pass
"#;
        let caught = caught_lines(&crate::ast::python::PythonPack, "tests/test_x.py", src);
        assert!(
            caught.is_empty(),
            "expected no caught assertions, got {caught:?}"
        );
    }

    #[test]
    fn rust_positive_controls() {
        let src = r#"
#[test]
fn test_let_wildcard() {
    let _ = std::panic::catch_unwind(|| assert_eq!(add(2, 2), 4));
}

#[test]
fn test_expr_statement() {
    std::panic::catch_unwind(|| assert_eq!(add(2, 2), 4)).ok();
}

#[test]
fn test_unused_binding() {
    let _res = std::panic::catch_unwind(|| {
        assert!(1 == 2);
    });
}
"#;
        let caught = caught_lines(&crate::ast::rust::RustPack, "src/lib.rs", src);
        assert_eq!(caught, vec![(4, 4), (9, 9), (15, 14)]);
    }

    #[test]
    fn rust_negative_controls() {
        let src = r#"
#[test]
fn test_normal_assert() {
    assert_eq!(add(2, 2), 4);
}

#[test]
fn test_catch_unwind_asserted() {
    let res = std::panic::catch_unwind(|| assert_eq!(add(2, 2), 4));
    assert!(res.is_err());
}

#[test]
fn test_catch_unwind_direct_assert() {
    assert!(std::panic::catch_unwind(|| assert_eq!(add(2, 2), 4)).is_err());
}

#[test]
fn test_catch_unwind_unwrap_err() {
    std::panic::catch_unwind(|| assert_eq!(add(2, 2), 4)).unwrap_err();
}
"#;
        let caught = caught_lines(&crate::ast::rust::RustPack, "src/lib.rs", src);
        assert!(
            caught.is_empty(),
            "expected no caught assertions, got {caught:?}"
        );
    }

    #[test]
    fn javascript_positive_and_negative_controls() {
        let pos_src = r#"
test('caught', () => {
    try {
        expect(add(2, 2)).toBe(4);
    } catch (e) {}
});
"#;
        let caught = caught_lines(&crate::ast::javascript::JavaScriptPack, "test.js", pos_src);
        assert_eq!(caught, vec![(4, 5)]);

        let neg_src = r#"
test('rethrow', () => {
    try {
        expect(add(2, 2)).toBe(4);
    } catch (e) {
        throw e;
    }
});
test('fail', () => {
    try {
        expect(add(2, 2)).toBe(4);
    } catch (e) {
        fail("error");
    }
});
test('normal', () => {
    expect(add(2, 2)).toBe(4);
});
"#;
        let neg_caught = caught_lines(&crate::ast::javascript::JavaScriptPack, "test.js", neg_src);
        assert!(
            neg_caught.is_empty(),
            "expected no caught assertions, got {neg_caught:?}"
        );
    }

    #[test]
    fn java_positive_and_negative_controls() {
        let pos_src = r#"
class TestA {
    @Test
    void testCaught() {
        try {
            assertEquals(4, add(2, 2));
        } catch (AssertionError e) {
        }
    }
}
"#;
        let caught = caught_lines(&crate::ast::java::JavaPack, "TestA.java", pos_src);
        assert_eq!(caught, vec![(6, 7)]);

        let neg_src = r#"
class TestB {
    @Test
    void testRethrow() {
        try {
            assertEquals(4, add(2, 2));
        } catch (AssertionError e) {
            throw e;
        }
    }
    @Test
    void testNormal() {
        assertEquals(4, add(2, 2));
    }
}
"#;
        let neg_caught = caught_lines(&crate::ast::java::JavaPack, "TestB.java", neg_src);
        assert!(
            neg_caught.is_empty(),
            "expected no caught assertions, got {neg_caught:?}"
        );
    }

    #[test]
    fn go_positive_and_negative_controls() {
        let pos_src = r#"
package p
import "testing"
func TestCaught(t *testing.T) {
    defer func() {
        _ = recover()
    }()
    mustEqual(4, add(2, 2))
}
func mustEqual(a, b int) {
    if a != b {
        panic("not equal")
    }
}
"#;
        let caught = caught_lines(&crate::ast::r#go::GoPack, "p_test.go", pos_src);
        assert_eq!(caught, vec![(8, 5)]);
        // An assertion that does not panic is not caught by the `recover()`.
        let not_caught =
            pos_src.replace("mustEqual(4, add(2, 2))", "require.Equal(t, 4, add(2, 2))");
        assert_eq!(
            caught_lines(&crate::ast::r#go::GoPack, "p_test.go", &not_caught),
            NONE
        );

        let neg_src = r#"
package p
import "testing"
func TestNormal(t *testing.T) {
    require.Equal(t, 4, add(2, 2))
}
"#;
        let neg_caught = caught_lines(&crate::ast::r#go::GoPack, "p_test.go", neg_src);
        assert!(
            neg_caught.is_empty(),
            "expected no caught assertions, got {neg_caught:?}"
        );
    }

    #[test]
    fn csharp_positive_and_negative_controls() {
        let pos_src = r#"
public class TestClass {
    [Fact]
    public void TestMethod() {
        try {
            Assert.Equal(4, add(2, 2));
        } catch (Exception) {
        }
    }
}
"#;
        let caught = caught_lines(&crate::ast::csharp::CSharpPack, "TestClass.cs", pos_src);
        assert_eq!(caught, vec![(6, 7)]);

        let neg_src = r#"
public class TestClass2 {
    [Fact]
    public void TestMethod() {
        try {
            Assert.Equal(4, add(2, 2));
        } catch (Exception e) {
            throw;
        }
    }
}
"#;
        let neg_caught = caught_lines(&crate::ast::csharp::CSharpPack, "TestClass2.cs", neg_src);
        assert!(
            neg_caught.is_empty(),
            "expected no caught assertions, got {neg_caught:?}"
        );
    }

    fn extracted(path: &str, src: &str) -> Vec<crate::ast::TestFn> {
        crate::ast::default_registry()
            .find_pack(path)
            .unwrap()
            .extract(path, src, &AssertVocabulary::default())
            .unwrap()
            .tests
    }

    /// A swallowed assertion is its node: two on one line are two, and the nested calls
    /// of one assertion (`expect(x)` inside `expect(x).toBe(1)`) are one.
    #[test]
    fn swallowed_assertions_are_counted_by_node_not_by_line() {
        let py = extracted(
            "tests/test_t.py",
            "def test_a():\n    try:\n        assert g() == 2; assert h() == 3\n    except AssertionError:\n        pass\n",
        );
        assert_eq!(py[0].caught_assertions.len(), 2, "{:?}", py[0]);
        assert_eq!(py[0].effective_asserts(), 0);
        let js = extracted(
            "a.test.js",
            "test('a', () => {\n  try {\n    expect(g()).toBe(2); expect(h()).toBe(3);\n  } catch (e) {}\n});\n",
        );
        assert_eq!(js[0].caught_assertions.len(), 2, "{:?}", js[0]);
        assert_eq!((js[0].total_asserts, js[0].effective_asserts()), (2, 0));
        // Control: one assertion in the handler is one, in each language.
        let js = extracted(
            "a.test.js",
            "test('a', () => {\n  expect(f()).toBe(1);\n  try {\n    expect(g()).toBe(2);\n  } catch (e) {}\n});\n",
        );
        assert_eq!(js[0].caught_assertions.len(), 1, "{:?}", js[0]);
        assert_eq!(js[0].effective_asserts(), 1);
    }

    /// A swallowed assertion that the pack also counts as a tautology is marked, and is
    /// taken out of the effective count once.
    #[test]
    fn a_swallowed_tautology_counts_against_the_test_once() {
        let cases = [
            (
                "tests/test_t.py",
                "def test_a():\n    assert g() == 2\n    try:\n        assert True\n    except AssertionError:\n        pass\n",
            ),
            (
                "tests/t.rs",
                "#[test]\nfn a() {\n    assert_eq!(g(), 2);\n    let _ = std::panic::catch_unwind(|| assert!(true));\n}\n",
            ),
            (
                "a.test.js",
                "test('a', () => {\n  expect(g()).toBe(2);\n  try {\n    expect(1).toBe(1);\n  } catch (e) {}\n});\n",
            ),
        ];
        for (path, src) in cases {
            let t = &extracted(path, src)[0];
            assert_eq!((t.total_asserts, t.tautologies), (2, 1), "{path}: {t:?}");
            assert_eq!(t.caught_assertions.len(), 1, "{path}: {t:?}");
            assert!(t.caught_assertions[0].tautology, "{path}: {t:?}");
            assert_eq!(t.effective_asserts(), 1, "{path}: {t:?}");
        }
        // Control: a swallowed assertion that is not a tautology is not marked, and a
        // tautology beside it is still taken out as well.
        let t = &extracted(
            "tests/test_t.py",
            "def test_a():\n    assert True\n    assert f() == 1\n    try:\n        assert g() == 2\n    except AssertionError:\n        pass\n",
        )[0];
        assert!(!t.caught_assertions[0].tautology, "{t:?}");
        assert_eq!((t.total_asserts, t.effective_asserts()), (3, 1), "{t:?}");
    }
}
