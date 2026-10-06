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
//! - Go `defer func() { recover() }()` enclosing assertions
//!
//! An assertion wrapped in a handler that catches its failure and swallows it (neither re-raising
//! nor failing the test nor asserting) cannot fail the test. It drops out of `effective_asserts()`.
//!
//! Whether the handler catches the failure is read from the type it names (`Reach`): the
//! language's assertion failure type and its ancestors always catch it, a list of standard
//! classes that an assertion failure is not an instance of never do, and a type on neither
//! list may. A handler that may catch it and swallows is reported: a class of the project
//! under review can extend the failure type, and passing it would leave a way to hide a
//! failure behind a new class name. Python is the exception: only `AssertionError`,
//! `Exception` and `BaseException` are read as catching it.

use super::bounds::{text, walk};
use super::TestFn;
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
    /// Whether the pack also counts the assertion as a tautology
    /// (`TestFn::tautology_spans`). Such an assertion is already out of the effective
    /// count, and being swallowed does not take it out again.
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

fn attribute(tests: &mut [TestFn], mut c: CaughtAssertion) {
    let line = c.line;
    if let Some(t) = tests
        .iter_mut()
        .filter(|t| t.line <= line && line <= t.end_line.max(t.line))
        .min_by_key(|t| t.end_line.saturating_sub(t.line))
    {
        if !t
            .caught_assertions
            .iter()
            // One assertion is neutralized once, whatever number of handlers enclose it.
            // The assertion is its node, so two on one line are two; a node inside
            // another (`expect(x)` in `expect(x).toBe(1)`) is the same assertion.
            .any(|existing| existing.span.0 < c.span.1 && c.span.0 < existing.span.1)
        {
            c.tautology = t
                .tautology_spans
                .iter()
                .any(|t| t.0 < c.span.1 && c.span.0 < t.1);
            t.caught_assertions.push(c);
        }
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
    let mut last = "";
    walk(node, &mut |n| {
        if n.child_count() == 0 && n.is_named() {
            last = text(n, src);
        }
        true
    });
    last
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

/// `AssertionError` and its ancestors. A class the file does not explain is read as
/// unrelated in Python.
const PY_CATCHING: &[&str] = &["AssertionError", "Exception", "BaseException"];

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

fn py_except_catches_assertion_error(clause: Node, src: &str) -> bool {
    let exprs = py_except_exprs(clause);
    if exprs.is_empty() {
        // Bare `except:` catches BaseException, which includes AssertionError.
        return true;
    }
    let mut names = Vec::new();
    for e in exprs {
        py_type_names(e, src, &mut names);
    }
    names.iter().any(|n| PY_CATCHING.contains(n))
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
fn py_fails(node: Node, src: &str) -> bool {
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

fn py_except_body_swallows(clause: Node, src: &str) -> bool {
    let Some(body) = clause
        .child_by_field_name("body")
        .or_else(|| find_child_by_kind(clause, "block"))
    else {
        return true;
    };
    !py_fails(body, src)
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
fn py_error_kept_and_checked(try_stmt: Node, clause: Node, src: &str) -> bool {
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
    let mut scope = try_stmt;
    while let Some(p) = scope.parent() {
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
            && py_fails(n, src)
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
fn py_retry_fails_after_last_attempt(try_stmt: Node, src: &str) -> bool {
    let mut enclosing_loop = None;
    let mut cur = try_stmt.parent();
    while let Some(p) = cur {
        match p.kind() {
            "for_statement" | "while_statement" => {
                enclosing_loop = Some(p);
                break;
            }
            "function_definition" | "class_definition" | "lambda" => break,
            _ => {}
        }
        cur = p.parent();
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
        .is_some_and(|e| py_fails(e, src));
    if (breaks || returns) && else_fails {
        return true;
    }
    if returns && !breaks {
        let mut sibling = lp.next_named_sibling();
        while let Some(s) = sibling {
            if py_always_fails(s, src) {
                return true;
            }
            sibling = s.next_named_sibling();
        }
    }
    false
}

/// Whether a `with` statement has a `suppress(...)` item (`contextlib.suppress`) that
/// lists `AssertionError` or one of its ancestors.
fn py_with_suppresses_assertion_error(with_stmt: Node, src: &str) -> bool {
    let mut suppresses = false;
    let Some(clause) = find_child_by_kind(with_stmt, "with_clause") else {
        return false;
    };
    walk(clause, &mut |n| {
        if suppresses {
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
            suppresses = names.iter().any(|name| PY_CATCHING.contains(name));
        }
        false
    });
    suppresses
}

pub fn python(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    walk(root, &mut |node| {
        let is_assertion = match node.kind() {
            "assert_statement" => true,
            "call" => {
                let callee = node
                    .child_by_field_name("function")
                    .map_or("", |f| text(f, src));
                callee.starts_with("self.assert")
                    && !callee.contains("assertRaises")
                    && !callee.contains("assertWarns")
                    && !callee.contains("assertLogs")
            }
            _ => false,
        };
        if !is_assertion {
            return true;
        }

        // Walk up to the nearest enclosing handler that swallows the failure.
        let mut cur = node.parent();
        while let Some(p) = cur {
            if matches!(p.kind(), "function_definition" | "class_definition") {
                break;
            }
            if p.kind() == "with_statement" {
                let in_body = p
                    .child_by_field_name("body")
                    .is_some_and(|b| within(node, b));
                if in_body && py_with_suppresses_assertion_error(p, src) {
                    attribute(
                        tests,
                        CaughtAssertion {
                            line: node.start_position().row + 1,
                            span: site(node).1,
                            tautology: false,
                            handler_line: p.start_position().row + 1,
                            detail: "contextlib.suppress discards the AssertionError".to_string(),
                        },
                    );
                    break;
                }
            }
            if p.kind() == "try_statement" {
                let inside_try_body = p
                    .child_by_field_name("body")
                    .is_some_and(|b| within(node, b));
                if inside_try_body {
                    let mut cursor = p.walk();
                    let matching_clause = p.children(&mut cursor).find(|c| {
                        c.kind() == "except_clause" && py_except_catches_assertion_error(*c, src)
                    });
                    if let Some(clause) = matching_clause {
                        if py_except_body_swallows(clause, src)
                            && !py_error_kept_and_checked(p, clause, src)
                            && !py_retry_fails_after_last_attempt(p, src)
                        {
                            attribute(
                                tests,
                                CaughtAssertion {
                                    line: node.start_position().row + 1,
                                    span: site(node).1,
                                    tautology: false,
                                    handler_line: clause.start_position().row + 1,
                                    detail:
                                        "AssertionError caught without re-raise or test failure"
                                            .to_string(),
                                },
                            );
                            break;
                        }
                    }
                }
            }
            cur = p.parent();
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

fn rs_closure_contains_assert(closure: Node, src: &str) -> Vec<Site> {
    let mut asserts = Vec::new();
    walk(closure, &mut |n| {
        if n.kind() == "function_item" {
            return false;
        }
        if n.kind() == "macro_invocation" {
            if RS_ASSERT_MACROS.contains(&rs_macro_name(n, src)) {
                asserts.push(site(n));
            }
        } else if n.kind() == "call_expression" {
            if let Some(f) = n.child_by_field_name("function") {
                if f.kind() == "field_expression" {
                    if let Some(field) = f.child_by_field_name("field") {
                        let method = text(field, src);
                        if method == "unwrap" || method == "expect" {
                            asserts.push(site(n));
                        }
                    }
                }
            }
        }
        true
    });
    asserts
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

/// Whether a later use of the binding `name` looks at the outcome it holds.
fn rs_binding_is_checked(binding: Node, name: &str, src: &str) -> bool {
    let mut scope = binding;
    while let Some(p) = scope.parent() {
        if scope.kind() == "function_item" {
            break;
        }
        scope = p;
    }
    let mut checked = false;
    walk(scope, &mut |n| {
        if checked {
            return false;
        }
        if n.kind() == "identifier" && n.start_byte() >= binding.end_byte() && text(n, src) == name
        {
            checked = !rs_value_is_discarded(n, src);
        }
        !checked
    });
    checked
}

/// Whether the value of `value` (the `catch_unwind` call, or a later use of the binding
/// that holds its result) is thrown away without anything looking at the outcome.
///
/// The value is followed outwards to where it ends up. It is looked at when a method that
/// panics on one side is called on it, when `?` or `return` hands it on, when it is the
/// value of the test function, when an asserting or panicking macro names it, and when an
/// `if` / `match` on it has a branch that fails. It is thrown away as a statement, in
/// `let _ =` or `_ =`, in `drop(..)`, in a binding nothing looks at, and in an `if` /
/// `match` no branch of which fails. A shape not listed is followed further out, so a
/// value this does not understand is reported, not passed.
fn rs_value_is_discarded(value: Node, src: &str) -> bool {
    let mut cur = value;
    while let Some(p) = cur.parent() {
        match p.kind() {
            "expression_statement" => return true,
            "try_expression" | "return_expression" => return false,
            // The value of the function body is the value of the test.
            "function_item" | "closure_expression" | "source_file" => return false,
            "token_tree" => {
                // A use inside a macro: its arguments are tokens, so the macro decides.
                let mut m = p;
                while m.kind() == "token_tree" {
                    match m.parent() {
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
                let callee = p
                    .parent()
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
                    || !rs_binding_is_checked(p, text(pattern, src), src);
            }
            "assignment_expression" => {
                let Some(left) = p.child_by_field_name("left") else {
                    return true;
                };
                return left.kind() != "identifier"
                    || !rs_binding_is_checked(p, text(left, src), src);
            }
            "match_expression" => {
                if p.child_by_field_name("value") == Some(cur) {
                    return !rs_fails(p, src);
                }
            }
            "if_expression" => {
                if p.child_by_field_name("condition") == Some(cur) {
                    return !rs_fails(p, src);
                }
            }
            "let_condition" | "let_chain" => {
                let mut branch = p;
                while branch.kind() != "if_expression" && branch.kind() != "while_expression" {
                    match branch.parent() {
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
                return !rs_fails(branch, src);
            }
            _ => {}
        }
        cur = p;
    }
    true
}

pub fn rust(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
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

        let inner_asserts = rs_closure_contains_assert(closure, src);
        if inner_asserts.is_empty() {
            return true;
        }

        if rs_value_is_discarded(node, src) {
            let handler_line = node.start_position().row + 1;
            for (line, span) in inner_asserts {
                attribute(
                    tests,
                    CaughtAssertion {
                        line,
                        span,
                        tautology: false,
                        handler_line,
                        detail: "std::panic::catch_unwind with discarded result".to_string(),
                    },
                );
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

fn js_is_assertion(call: Node, src: &str) -> bool {
    let callee = js_callee(call, src);
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
fn js_handler_swallows(body: Node, src: &str) -> bool {
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
                || (callee == "done" && has_argument);
        }
        !fails
    });
    !fails
}

fn js_catch_body_swallows(catch_clause: Node, src: &str) -> bool {
    let Some(body) = catch_clause
        .child_by_field_name("body")
        .or_else(|| find_child_by_kind(catch_clause, "statement_block"))
    else {
        return true;
    };
    js_handler_swallows(body, src)
}

/// `<promise>.catch(<function that swallows>)`: the assertions in the chain before it.
fn js_promise_catch(call: Node, src: &str, tests: &mut [TestFn]) {
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
    let handler = call
        .child_by_field_name("arguments")
        .and_then(|a| a.named_child(0))
        .filter(|h| matches!(h.kind(), "arrow_function" | "function_expression"));
    let Some(body) = handler.and_then(|h| h.child_by_field_name("body")) else {
        // A named handler (`.catch(done)`) is not read.
        return;
    };
    if !js_handler_swallows(body, src) {
        return;
    }
    let (Some(chain), Some(property)) = (
        function.child_by_field_name("object"),
        function.child_by_field_name("property"),
    ) else {
        return;
    };
    let handler_line = property.start_position().row + 1;
    walk(chain, &mut |n| {
        if n.kind() == "call_expression" && js_is_assertion(n, src) {
            attribute(
                tests,
                CaughtAssertion {
                    line: n.start_position().row + 1,
                    span: site(n).1,
                    tautology: false,
                    handler_line,
                    detail: "promise .catch() swallows assertion error".to_string(),
                },
            );
        }
        true
    });
}

pub fn javascript(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    walk(root, &mut |node| {
        if node.kind() == "call_expression" {
            js_promise_catch(node, src, tests);
            return true;
        }
        if node.kind() != "try_statement" {
            return true;
        }
        let Some(body) = node.child_by_field_name("body") else {
            return true;
        };
        let catch_clause = node
            .child_by_field_name("handler")
            .or_else(|| find_child_by_kind(node, "catch_clause"));
        let Some(clause) = catch_clause else {
            return true;
        };

        if !js_catch_body_swallows(clause, src) {
            return true;
        }

        let handler_line = clause.start_position().row + 1;
        walk(body, &mut |n| {
            if JS_FUNCTIONS.contains(&n.kind()) || n.kind() == "try_statement" {
                return false;
            }
            if n.kind() == "call_expression" && js_is_assertion(n, src) {
                attribute(
                    tests,
                    CaughtAssertion {
                        line: n.start_position().row + 1,
                        span: site(n).1,
                        tautology: false,
                        handler_line,
                        detail: "try/catch swallows assertion error".to_string(),
                    },
                );
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

fn java_catch_body_swallows(clause: Node, src: &str) -> bool {
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
        if matches!(
            n.kind(),
            "class_declaration" | "method_declaration" | "lambda_expression"
        ) {
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
            if name.starts_with("assert") || name.starts_with("assertThat") {
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

pub fn java(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
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
                    java_catch_body_swallows(c, src),
                )
            })
            .collect();
        let Some(clause) = swallowing_handler(&handlers) else {
            return true;
        };
        let handler_line = clause.start_position().row + 1;

        walk(body, &mut |n| {
            if matches!(
                n.kind(),
                "class_declaration"
                    | "method_declaration"
                    | "lambda_expression"
                    | "try_statement"
                    | "try_with_resources_statement"
            ) {
                return false;
            }
            let is_assertion = match n.kind() {
                "assert_statement" => true,
                "method_invocation" => {
                    let name = n.child_by_field_name("name").map_or("", |m| text(m, src));
                    (name.starts_with("assert") && name != "assertThrows")
                        || name.starts_with("assertThat")
                }
                _ => false,
            };
            if is_assertion {
                attribute(
                    tests,
                    CaughtAssertion {
                        line: n.start_position().row + 1,
                        span: site(n).1,
                        tautology: false,
                        handler_line,
                        detail: "AssertionError caught by catch clause".to_string(),
                    },
                );
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
fn kt_is_assertion(node: Node, src: &str) -> bool {
    let name = match node.kind() {
        "call_expression" => kt_callee(node, src),
        "infix_expression" => node.named_child(1).map_or("", |op| text(op, src)),
        _ => return false,
    };
    (name.starts_with("assert") || name.starts_with("should"))
        && !name.starts_with("assertThrows")
        && !name.starts_with("assertFails")
        && !name.starts_with("shouldThrow")
        && !name.starts_with("shouldNotThrow")
}

/// Whether the subtree throws, asserts or calls a function that fails the test.
fn kt_fails(node: Node, src: &str) -> bool {
    let mut fails = false;
    walk(node, &mut |n| {
        if fails || (n != node && KT_SCOPES.contains(&n.kind())) {
            return false;
        }
        fails = n.kind() == "throw_expression"
            || kt_is_assertion(n, src)
            || (n.kind() == "call_expression" && matches!(kt_callee(n, src), "fail" | "error"));
        !fails
    });
    fails
}

/// Lines of the assertions directly in `body` (not in a lambda or a nested `try`).
fn kt_assertions(body: Node, src: &str) -> Vec<Site> {
    let mut lines = Vec::new();
    walk(body, &mut |n| {
        if n != body && (KT_SCOPES.contains(&n.kind()) || n.kind() == "try_expression") {
            return false;
        }
        if kt_is_assertion(n, src) {
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

/// Whether nothing looks at the result of a `runCatching { }` call. It is looked at when
/// `getOrThrow()` is called on it, when a callback chained on it (`onFailure { }`,
/// `fold`, `recover`, `getOrElse`) throws or fails, when it is bound to a name used
/// later, and when it is passed on (an argument, a `return`, an expression body).
fn kt_result_is_unused(call: Node, src: &str) -> bool {
    let mut cur = call;
    while let Some(p) = cur.parent() {
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
                    if kt_fails(lambda, src) {
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
                while let Some(up) = scope.parent() {
                    if scope.kind() == "function_declaration" || scope.kind() == "lambda_literal" {
                        break;
                    }
                    scope = up;
                }
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

pub fn kotlin(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
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
                    let swallows = find_child_by_kind(c, "block").is_none_or(|b| !kt_fails(b, src));
                    (c, kt_catch_reach(c, src), swallows)
                })
                .collect();
            if let Some(handler) = swallowing_handler(&handlers) {
                let handler_line = handler.start_position().row + 1;
                for (line, span) in kt_assertions(body, src) {
                    attribute(
                        tests,
                        CaughtAssertion {
                            line,
                            span,
                            tautology: false,
                            handler_line,
                            detail: "AssertionError caught by catch clause".to_string(),
                        },
                    );
                }
            }
        } else if node.kind() == "call_expression" && kt_callee(node, src) == "runCatching" {
            let lambda = find_child_by_kind(node, "annotated_lambda")
                .and_then(|a| find_child_by_kind(a, "lambda_literal"));
            if let Some(lambda) = lambda {
                if kt_result_is_unused(node, src) {
                    let handler_line = node.start_position().row + 1;
                    for (line, span) in kt_assertions(lambda, src) {
                        attribute(
                            tests,
                            CaughtAssertion {
                                line,
                                span,
                                tautology: false,
                                handler_line,
                                detail: "runCatching with the result unused".to_string(),
                            },
                        );
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

fn csharp_catch_body_swallows(clause: Node, src: &str) -> bool {
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
            || (n.kind() == "invocation_expression" && csharp_assert_method(n, src).is_some());
        !fails
    });
    !fails
}

pub fn csharp(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
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
                    csharp_catch_body_swallows(c, src),
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
                "class_declaration" | "method_declaration" | "lambda_expression" | "try_statement"
            ) {
                return false;
            }
            if n.kind() == "invocation_expression"
                && csharp_assert_method(n, src).is_some_and(|m| !m.starts_with("Throws"))
            {
                attribute(
                    tests,
                    CaughtAssertion {
                        line: n.start_position().row + 1,
                        span: site(n).1,
                        tautology: false,
                        handler_line,
                        detail: "Assertion caught by catch clause".to_string(),
                    },
                );
            }
            true
        });
        true
    });
}

// ---------------------------------------------------------------------------
// Go
// ---------------------------------------------------------------------------

pub fn go(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    walk(root, &mut |node| {
        if node.kind() != "function_declaration" {
            return true;
        }
        let fn_name = node
            .child_by_field_name("name")
            .map_or("", |n| text(n, src));
        if !fn_name.starts_with("Test") {
            return true;
        }

        let Some(body) = node.child_by_field_name("body") else {
            return true;
        };

        let mut recover_defer_line: Option<usize> = None;
        walk(body, &mut |n| {
            if n.kind() == "defer_statement" {
                let defer_text = text(n, src);
                if defer_text.contains("recover()") {
                    let mut re_panics_or_fails = false;
                    walk(n, &mut |d| {
                        if d.kind() == "call_expression" {
                            let callee = d
                                .child_by_field_name("function")
                                .map_or("", |f| text(f, src));
                            // `t.Fail()`, `require.Fail(t, ..)`, `assert.Failf(t, ..)`:
                            // the method name, read from the selector node.
                            let fails_by_name = d
                                .child_by_field_name("function")
                                .filter(|f| f.kind() == "selector_expression")
                                .and_then(|f| f.child_by_field_name("field"))
                                .is_some_and(|f| matches!(text(f, src), "Fail" | "Failf"));
                            if callee == "panic"
                                || callee.contains(".Fatal")
                                || callee.contains(".FailNow")
                                || callee.contains(".Error")
                                || fails_by_name
                            {
                                re_panics_or_fails = true;
                                return false;
                            }
                        }
                        true
                    });
                    if !re_panics_or_fails {
                        recover_defer_line = Some(n.start_position().row + 1);
                    }
                }
            }
            true
        });

        if let Some(handler_line) = recover_defer_line {
            walk(body, &mut |n| {
                if n.kind() == "call_expression" {
                    let callee = n
                        .child_by_field_name("function")
                        .map_or("", |f| text(f, src));
                    if callee.starts_with("require.") || callee.starts_with("assert.") {
                        let line = n.start_position().row + 1;
                        if line > handler_line {
                            attribute(
                                tests,
                                CaughtAssertion {
                                    line,
                                    span: site(n).1,
                                    tautology: false,
                                    handler_line,
                                    detail: "recover() catches assertion panic".to_string(),
                                },
                            );
                        }
                    }
                }
                true
            });
        }
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
            "package p\nfunc TestA(t *testing.T) {{\n\tdefer func() {{\n\t\tif r := recover(); r != nil {{\n\t\t\t{handler}\n\t\t}}\n\t}}()\n\trequire.Equal(t, 4, add(2, 2))\n}}\n"
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
        let tree = parser.parse("a\nb\n", None).unwrap();
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
            "    match r {\n        Ok(()) => panic!(\"no\"),\n        Err(_) => {}\n    }\n",
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
            "    let failed = r.is_err();\n",
        ] {
            assert_eq!(rs(&format!("{UNWIND}{dropped}")), vec![(3, 3)], "{dropped}");
        }
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

    /// Pending the decision on how `recover()` is read (#569): what it reports today.
    #[test]
    fn pinned_go_recovering_defer_reports_as_before() {
        assert_eq!(go("log.Println(r)"), vec![(8, 3)]);
        assert_eq!(go("t.Fatal(r)"), NONE);
        assert_eq!(go("log.Println(r.(error).Error())"), NONE);
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
    require.Equal(t, 4, add(2, 2))
}
"#;
        let caught = caught_lines(&crate::ast::r#go::GoPack, "p_test.go", pos_src);
        assert_eq!(caught, vec![(8, 5)]);

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
