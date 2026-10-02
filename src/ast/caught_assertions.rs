//! Assertions whose failure is caught inside the test function:
//! - Python `try: assert ... except AssertionError: pass` (or `except Exception`, `except BaseException`, bare `except:`)
//! - Rust `let _ = std::panic::catch_unwind(|| assert_eq!(...))`
//! - JS / TS `try { expect(...) } catch {}`
//! - Java / Kotlin `try { assertEquals(...) } catch (AssertionError | Throwable e) {}`
//! - C# `try { Assert.Equal(...) } catch (Exception) {}`
//! - Go `defer func() { recover() }()` enclosing assertions
//!
//! An assertion wrapped in a handler that catches its failure and swallows it (neither re-raising
//! nor failing the test nor asserting) cannot fail the test. It drops out of `effective_asserts()`.

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
}

fn attribute(tests: &mut [TestFn], c: CaughtAssertion) {
    let line = c.line;
    if let Some(t) = tests
        .iter_mut()
        .filter(|t| t.line <= line && line <= t.end_line.max(t.line))
        .min_by_key(|t| t.end_line.saturating_sub(t.line))
    {
        if !t
            .caught_assertions
            .iter()
            .any(|existing| existing.line == c.line && existing.handler_line == c.handler_line)
        {
            t.caught_assertions.push(c);
        }
    }
}

fn find_child_by_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let res = node
        .children(&mut cursor)
        .find(|child| child.kind() == kind);
    res
}

// ---------------------------------------------------------------------------
// Python
// ---------------------------------------------------------------------------

fn py_except_catches_assertion_error(clause: Node, src: &str) -> bool {
    let clause_text = text(clause, src);
    let head = clause_text.lines().next().unwrap_or("").trim();
    let Some(rest) = head.strip_prefix("except") else {
        return false;
    };
    let types = rest.split(':').next().unwrap_or("").trim();
    let types = types.split(" as ").next().unwrap_or("").trim();
    if types.is_empty() {
        // Bare `except:` catches BaseException, which includes AssertionError.
        return true;
    }
    let names: Vec<&str> = types
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split(',')
        .map(|t| t.trim().trim_start_matches("builtins."))
        .filter(|t| !t.is_empty())
        .collect();

    names.iter().any(|&n| {
        n == "AssertionError"
            || n == "Exception"
            || n == "BaseException"
            || n.ends_with(".AssertionError")
            || n.ends_with(".Exception")
            || n.ends_with(".BaseException")
    })
}

fn py_except_body_swallows(clause: Node, src: &str) -> bool {
    let Some(body) = clause
        .child_by_field_name("body")
        .or_else(|| find_child_by_kind(clause, "block"))
    else {
        return true;
    };

    let mut has_raise = false;
    let mut has_fail = false;
    let mut has_assert = false;

    walk(body, &mut |n| {
        if matches!(
            n.kind(),
            "function_definition" | "class_definition" | "lambda"
        ) {
            return false;
        }
        if n.kind() == "raise_statement" {
            has_raise = true;
            return false;
        }
        if n.kind() == "assert_statement" {
            has_assert = true;
            return false;
        }
        if n.kind() == "call" {
            let callee = n
                .child_by_field_name("function")
                .map_or("", |f| text(f, src));
            if callee.starts_with("self.assert") || callee.starts_with("assert_") {
                has_assert = true;
                return false;
            }
            if callee == "pytest.fail"
                || callee == "self.fail"
                || callee == "fail"
                || callee.ends_with(".fail")
            {
                has_fail = true;
                return false;
            }
        }
        true
    });

    !has_raise && !has_fail && !has_assert
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

        // Walk up to find nearest enclosing try_statement
        let mut cur = node.parent();
        while let Some(p) = cur {
            if matches!(p.kind(), "function_definition" | "class_definition") {
                break;
            }
            if p.kind() == "try_statement" {
                let try_body = p.child_by_field_name("body");
                let inside_try_body = try_body.is_some_and(|b| {
                    node.start_byte() >= b.start_byte() && node.end_byte() <= b.end_byte()
                });
                if inside_try_body {
                    let mut cursor = p.walk();
                    let matching_clause = p.children(&mut cursor).find(|c| {
                        c.kind() == "except_clause" && py_except_catches_assertion_error(*c, src)
                    });
                    if let Some(clause) = matching_clause {
                        if py_except_body_swallows(clause, src) {
                            attribute(
                                tests,
                                CaughtAssertion {
                                    line: node.start_position().row + 1,
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

fn rs_closure_contains_assert(closure: Node, src: &str) -> Vec<usize> {
    let mut asserts = Vec::new();
    walk(closure, &mut |n| {
        if n.kind() == "function_item" {
            return false;
        }
        if n.kind() == "macro_invocation" {
            let macro_name = n.child_by_field_name("macro").map_or("", |m| text(m, src));
            let short = macro_name.rsplit("::").next().unwrap_or(macro_name);
            if RS_ASSERT_MACROS.contains(&short) {
                asserts.push(n.start_position().row + 1);
            }
        } else if n.kind() == "call_expression" {
            if let Some(f) = n.child_by_field_name("function") {
                if f.kind() == "field_expression" {
                    if let Some(field) = f.child_by_field_name("field") {
                        let method = text(field, src);
                        if method == "unwrap" || method == "expect" {
                            asserts.push(n.start_position().row + 1);
                        }
                    }
                }
            }
        }
        true
    });
    asserts
}

fn rs_catch_unwind_is_asserted_or_used(call: Node, src: &str) -> bool {
    let mut cur = call.parent();
    while let Some(p) = cur {
        if p.kind() == "macro_invocation" {
            let mname = p.child_by_field_name("macro").map_or("", |m| text(m, src));
            let short = mname.rsplit("::").next().unwrap_or(mname);
            if RS_ASSERT_MACROS.contains(&short) {
                return true;
            }
        }
        if p.kind() == "field_expression" {
            if let Some(field) = p.child_by_field_name("field") {
                let m = text(field, src);
                if m == "unwrap_err" || m == "expect_err" {
                    return true;
                }
            }
        }
        if p.kind() == "let_declaration" {
            let pat = p
                .child_by_field_name("pattern")
                .map_or("", |pat| text(pat, src))
                .trim();
            if pat == "_" || pat.starts_with('_') {
                return false;
            }
            let mut fn_ancestor = p.parent();
            while let Some(fa) = fn_ancestor {
                if fa.kind() == "function_item" {
                    let fn_text = text(fa, src);
                    let err_check = format!("{pat}.is_err()");
                    let ok_check = format!("{pat}.is_ok()");
                    let unwrap_err = format!("{pat}.unwrap_err()");
                    let expect_err = format!("{pat}.expect_err(");
                    if fn_text.contains(&err_check)
                        || fn_text.contains(&ok_check)
                        || fn_text.contains(&unwrap_err)
                        || fn_text.contains(&expect_err)
                    {
                        return true;
                    }
                    return false;
                }
                fn_ancestor = fa.parent();
            }
            return false;
        }
        if p.kind() == "expression_statement" {
            return false;
        }
        if p.kind() == "function_item" {
            break;
        }
        cur = p.parent();
    }
    false
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
        let closure_node = find_child_by_kind(args, "closure_expression");
        let Some(closure) = closure_node else {
            return true;
        };

        let inner_asserts = rs_closure_contains_assert(closure, src);
        if inner_asserts.is_empty() {
            return true;
        }

        if !rs_catch_unwind_is_asserted_or_used(node, src) {
            let handler_line = node.start_position().row + 1;
            for line in inner_asserts {
                attribute(
                    tests,
                    CaughtAssertion {
                        line,
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

fn js_catch_body_swallows(catch_clause: Node, src: &str) -> bool {
    let Some(body) = catch_clause
        .child_by_field_name("body")
        .or_else(|| find_child_by_kind(catch_clause, "statement_block"))
    else {
        return true;
    };

    let mut has_throw = false;
    let mut has_fail = false;
    let mut has_assert = false;

    walk(body, &mut |n| {
        if matches!(
            n.kind(),
            "arrow_function" | "function_expression" | "function_declaration"
        ) {
            return false;
        }
        if n.kind() == "throw_statement" {
            has_throw = true;
            return false;
        }
        if n.kind() == "call_expression" {
            let callee = n
                .child_by_field_name("function")
                .map_or("", |f| text(f, src));
            if callee == "expect"
                || callee.starts_with("expect(")
                || callee == "assert"
                || callee.starts_with("assert.")
            {
                has_assert = true;
                return false;
            }
            if callee == "fail" || callee == "done.fail" || callee.ends_with(".fail") {
                has_fail = true;
                return false;
            }
        }
        true
    });

    !has_throw && !has_fail && !has_assert
}

pub fn javascript(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    walk(root, &mut |node| {
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
            if matches!(
                n.kind(),
                "arrow_function" | "function_expression" | "function_declaration" | "try_statement"
            ) {
                return false;
            }
            if n.kind() == "call_expression" {
                let callee = n
                    .child_by_field_name("function")
                    .map_or("", |f| text(f, src));
                let is_assert = callee == "expect"
                    || callee.starts_with("expect(")
                    || callee == "assert"
                    || callee.starts_with("assert.");
                let is_expect_chain = {
                    let mut cur = n;
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
                if is_assert || is_expect_chain {
                    attribute(
                        tests,
                        CaughtAssertion {
                            line: n.start_position().row + 1,
                            handler_line,
                            detail: "try/catch swallows assertion error".to_string(),
                        },
                    );
                }
            }
            true
        });
        true
    });
}

// ---------------------------------------------------------------------------
// Java
// ---------------------------------------------------------------------------

fn java_catch_catches_assertion_error(clause: Node, src: &str) -> bool {
    let Some(param) = clause
        .child_by_field_name("param")
        .or_else(|| find_child_by_kind(clause, "catch_formal_parameter"))
    else {
        return false;
    };
    let param_text = text(param, src);
    param_text.contains("AssertionError")
        || param_text.contains("Throwable")
        || param_text.contains("Error")
        || param_text.contains("Exception")
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
        if node.kind() != "try_statement" {
            return true;
        }
        let Some(body) = node.child_by_field_name("body") else {
            return true;
        };

        let mut cursor = node.walk();
        let matching_clause = node.children(&mut cursor).find(|c| {
            c.kind() == "catch_clause"
                && java_catch_catches_assertion_error(*c, src)
                && java_catch_body_swallows(*c, src)
        });
        let Some(clause) = matching_clause else {
            return true;
        };
        let handler_line = clause.start_position().row + 1;

        walk(body, &mut |n| {
            if matches!(
                n.kind(),
                "class_declaration" | "method_declaration" | "lambda_expression" | "try_statement"
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
// C#
// ---------------------------------------------------------------------------

fn csharp_catch_catches_assertion_error(clause: Node, src: &str) -> bool {
    let decl = find_child_by_kind(clause, "catch_declaration");
    let Some(decl_node) = decl else {
        // Bare catch { } catches everything
        return true;
    };
    let decl_text = text(decl_node, src);
    decl_text.contains("Exception")
        || decl_text.contains("AssertionException")
        || decl_text.contains("XunitException")
}

fn csharp_catch_body_swallows(clause: Node, src: &str) -> bool {
    let Some(body) = find_child_by_kind(clause, "block") else {
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
        if n.kind() == "invocation_expression" {
            let inv_text = text(n, src);
            if inv_text.contains("Assert.") || inv_text.starts_with("Assert.") {
                if inv_text.contains("Assert.Fail") {
                    has_fail = true;
                } else {
                    has_assert = true;
                }
                return false;
            }
        }
        true
    });

    !has_throw && !has_fail && !has_assert
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
        let matching_clause = node.children(&mut c2).find(|c| {
            c.kind() == "catch_clause"
                && csharp_catch_catches_assertion_error(*c, src)
                && csharp_catch_body_swallows(*c, src)
        });
        let Some(clause) = matching_clause else {
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
            if n.kind() == "invocation_expression" {
                let inv_text = text(n, src);
                if (inv_text.contains("Assert.") || inv_text.starts_with("Assert."))
                    && !inv_text.contains("Assert.Throws")
                {
                    attribute(
                        tests,
                        CaughtAssertion {
                            line: n.start_position().row + 1,
                            handler_line,
                            detail: "Assertion caught by catch clause".to_string(),
                        },
                    );
                }
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
                            if callee == "panic"
                                || callee.contains(".Fatal")
                                || callee.contains(".FailNow")
                                || callee.contains(".Error")
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
}
