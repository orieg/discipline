//! Expected exceptions and panics inside assertions and test attributes:
//! `#[should_panic(expected = "...")]`, `pytest.raises(...)`, `assertThrows(...)`,
//! `toThrow(...)`, `Assert.Throws<...>`, `expect { }.to raise_error(...)`.
//!
//! Widening what an existing test accepts as expected failure keeps assertion count
//! and strength unchanged, so a count cannot see `#[should_panic(expected = "...")]`
//! become bare `#[should_panic]`, or `pytest.raises(ValueError, match="...")` become
//! `pytest.raises(Exception)`. Each expected exception or panic is recorded with its
//! skeleton, kind, exception type, and matcher. `assertion-reduction` pairs base and
//! head expected exceptions by skeleton and reports when an expected failure was widened.

use super::bounds::{text, walk};
use super::TestFn;
use tree_sitter::Node;

/// One expected exception or panic of one test.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExpectedException {
    pub line: usize,
    /// The assertion's skeleton used for pairing between base and head.
    pub skeleton: String,
    /// The assertion kind: e.g. "should_panic", "pytest.raises", "assertThrows", "toThrow", "Assert.Throws".
    pub kind: String,
    /// The expected exception type (e.g. "ValueError", "IllegalArgumentException", "TypeError"), if any.
    pub exception_type: Option<String>,
    /// The matcher (expected pattern, message, or substring), if any.
    pub matcher: Option<String>,
}

/// An expected exception or panic that was widened between base and head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Widened {
    pub line: usize,
    pub skeleton: String,
    pub detail: String,
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    if (t.starts_with('"') && t.ends_with('"'))
        || (t.starts_with('\'') && t.ends_with('\''))
        || (t.starts_with('`') && t.ends_with('`'))
    {
        if t.len() >= 2 {
            t[1..t.len() - 1].to_string()
        } else {
            t.to_string()
        }
    } else {
        t.to_string()
    }
}

/// Ancestor rank for exception types across supported languages:
/// 2: Root exception hierarchy (`BaseException`, `Throwable`).
/// 1: General exception hierarchy (`Exception`, `Error`, `System.Exception`, `StandardError`).
/// 0: Specific exception (`ValueError`, `IllegalArgumentException`, `TypeError`, `CustomError`, etc.).
fn ancestor_rank(type_name: &str) -> usize {
    let clean = type_name
        .rsplit('.')
        .next()
        .unwrap_or(type_name)
        .trim_end_matches(".class");
    match clean {
        "BaseException" | "Throwable" => 2,
        "Exception" | "Error" | "StandardError" => 1,
        _ => 0,
    }
}

/// Returns whether `h` is a widening of `b`.
pub fn is_widened(b: &ExpectedException, h: &ExpectedException) -> Option<String> {
    // 1. Matcher removed (e.g. #[should_panic(expected = "foo")] -> #[should_panic] or match= dropped).
    let matcher_dropped = b.matcher.is_some() && h.matcher.is_none();

    // 2. Exception type removed / dropped (e.g. toThrow(TypeError) -> toThrow()).
    let type_dropped = b.exception_type.is_some() && h.exception_type.is_none();

    // 3. Exception type moved to broader ancestor (e.g. ValueError -> Exception).
    let type_widened = match (&b.exception_type, &h.exception_type) {
        (Some(b_type), Some(h_type)) => {
            let b_clean = b_type
                .rsplit('.')
                .next()
                .unwrap_or(b_type)
                .trim_end_matches(".class");
            let h_clean = h_type
                .rsplit('.')
                .next()
                .unwrap_or(h_type)
                .trim_end_matches(".class");
            if b_clean != h_clean && ancestor_rank(h_clean) > ancestor_rank(b_clean) {
                Some(format!("from `{b_clean}` to `{h_clean}`"))
            } else {
                None
            }
        }
        _ => None,
    };

    // 4. Moved from exact Assert.Throws to generic Assert.ThrowsAny.
    let throws_any = (b.kind == "Assert.Throws" || b.kind == "Assert.ThrowsAsync")
        && (h.kind == "Assert.ThrowsAny" || h.kind == "Assert.ThrowsAnyAsync");

    if matcher_dropped {
        if let Some(tw) = type_widened {
            Some(format!(
                "expected exception type widened {tw} and matcher was removed"
            ))
        } else {
            Some("expected pattern or message matcher was removed".to_string())
        }
    } else if type_dropped {
        Some("expected exception type was removed".to_string())
    } else if let Some(tw) = type_widened {
        Some(format!("expected exception type widened {tw}"))
    } else if throws_any {
        Some("expected exception checked with `ThrowsAny` instead of exact type".to_string())
    } else {
        None
    }
}

/// Pairs `base` and `head` expected exceptions by skeleton, and returns those that were widened.
/// A skeleton that is not exactly once on each side is ambiguous and skipped.
pub fn widened(base: &[ExpectedException], head: &[ExpectedException]) -> Vec<Widened> {
    let once =
        |set: &[ExpectedException], s: &str| set.iter().filter(|x| x.skeleton == s).count() == 1;
    head.iter()
        .filter(|h| once(head, &h.skeleton) && once(base, &h.skeleton))
        .filter_map(|h| {
            let b = base.iter().find(|b| b.skeleton == h.skeleton)?;
            is_widened(b, h).map(|detail| Widened {
                line: h.line,
                skeleton: h.skeleton.clone(),
                detail,
            })
        })
        .collect()
}

fn attribute(tests: &mut [TestFn], exp: ExpectedException) {
    let line = exp.line;
    if let Some(t) = tests
        .iter_mut()
        .filter(|t| t.line <= line && line <= t.end_line.max(t.line))
        .min_by_key(|t| t.end_line.saturating_sub(t.line))
    {
        if !t
            .expected_exceptions
            .iter()
            .any(|e| e.skeleton == exp.skeleton && e.line == exp.line)
        {
            t.expected_exceptions.push(exp);
        }
    }
}

/// Parses a Rust `#[should_panic]` attribute into an [`ExpectedException`].
pub fn parse_rust_should_panic(attr_text: &str, line: usize) -> ExpectedException {
    let matcher = parse_rust_matcher(attr_text);
    ExpectedException {
        line,
        skeleton: "#[should_panic]".to_string(),
        kind: "should_panic".to_string(),
        exception_type: None,
        matcher,
    }
}

fn parse_rust_matcher(text: &str) -> Option<String> {
    let idx = text.find("expected")?;
    let rest = text[idx + "expected".len()..].trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    if rest.starts_with('r') {
        let quote_idx = rest.find('"')?;
        let hashes = &rest[1..quote_idx];
        let end_pattern = format!("\"{hashes}");
        let end_idx = rest[quote_idx + 1..].find(&end_pattern)?;
        Some(rest[quote_idx + 1..quote_idx + 1 + end_idx].to_string())
    } else if let Some(stripped) = rest.strip_prefix('"') {
        let end_idx = stripped.find('"')?;
        Some(stripped[..end_idx].to_string())
    } else {
        None
    }
}

/// Rust: attribute-level `#[should_panic]` is handled directly by `rust.rs`.
pub fn rust(_root: Node, _src: &str, _tests: &mut [TestFn]) {}

/// Python: `pytest.raises(...)` (in `with` statement or call) and `self.assertRaises(...)` / `assertRaisesRegex`.
pub fn python(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    walk(root, &mut |node| {
        match node.kind() {
            "with_statement" => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "with_clause" {
                        let mut c2 = child.walk();
                        for item in child.children(&mut c2) {
                            if item.kind() == "with_item" {
                                inspect_python_with_item(node, item, src, tests);
                            }
                        }
                    } else if child.kind() == "with_item" {
                        inspect_python_with_item(node, child, src, tests);
                    }
                }
                true
            }
            "call" => {
                // Standalone calls like self.assertRaises(ValueError, f, -1). The context
                // manager of a `with` item, bound with `as` or not, is read above.
                if !is_python_with_item_value(node) {
                    inspect_python_standalone_call(node, src, tests);
                }
                true
            }
            _ => true,
        }
    });
}

/// Whether `call` is the context manager of a `with` item: its value, or the value an
/// `as` binding wraps (`with f() as e:` parses as `with_item > as_pattern > call`).
fn is_python_with_item_value(call: Node) -> bool {
    let Some(parent) = call.parent() else {
        return false;
    };
    match parent.kind() {
        "with_item" => true,
        "as_pattern" => {
            parent.parent().is_some_and(|g| g.kind() == "with_item")
                && parent.named_child(0).is_some_and(|c| c.id() == call.id())
        }
        _ => false,
    }
}

fn inspect_python_with_item(with_stmt: Node, item: Node, src: &str, tests: &mut [TestFn]) {
    let call = if let Some(val) = item.child_by_field_name("value") {
        val
    } else {
        let mut found = None;
        let mut cursor = item.walk();
        for c in item.children(&mut cursor) {
            if c.kind() == "call" {
                found = Some(c);
                break;
            }
        }
        let Some(c) = found else {
            return;
        };
        c
    };
    // `with pytest.raises(E) as e:` wraps the call in an `as_pattern`.
    let call = if call.kind() == "as_pattern" {
        let Some(inner) = call.named_child(0) else {
            return;
        };
        inner
    } else {
        call
    };
    let Some(func) = call.child_by_field_name("function") else {
        return;
    };
    let func_text = text(func, src);

    if func_text == "pytest.raises" || func_text.ends_with(".pytest.raises") {
        let Some(args) = call.child_by_field_name("arguments") else {
            return;
        };
        let mut cursor = args.walk();
        let named: Vec<Node> = args.named_children(&mut cursor).collect();

        let mut exception_type = None;
        let mut matcher = None;

        for arg in &named {
            if arg.kind() == "keyword_argument" {
                if let Some(name) = arg.child_by_field_name("name") {
                    if text(name, src) == "match" {
                        if let Some(val) = arg.child_by_field_name("value") {
                            matcher = Some(unquote(text(val, src)));
                        }
                    }
                }
            } else if exception_type.is_none() {
                exception_type = Some(text(*arg, src).to_string());
            }
        }

        // Skeleton: with_stmt text with the call's arguments replaced by (#)
        let (ws, we) = (with_stmt.start_byte(), with_stmt.end_byte());
        let (as_pos, ae_pos) = (args.start_byte(), args.end_byte());
        let skeleton_raw = if as_pos >= ws && ae_pos <= we {
            format!("{}(#){}", &src[ws..as_pos], &src[ae_pos..we])
        } else {
            format!("with pytest.raises(#): {}", text(with_stmt, src))
        };
        let skeleton = collapse_ws(&skeleton_raw);

        attribute(
            tests,
            ExpectedException {
                line: call.start_position().row + 1,
                skeleton,
                kind: "pytest.raises".to_string(),
                exception_type,
                matcher,
            },
        );
    } else if func_text.ends_with("assertRaises") || func_text.ends_with("assertRaisesRegex") {
        let is_regex = func_text.ends_with("assertRaisesRegex");
        let Some(args) = call.child_by_field_name("arguments") else {
            return;
        };
        let mut cursor = args.walk();
        let named: Vec<Node> = args.named_children(&mut cursor).collect();

        let exception_type = named.first().map(|a| text(*a, src).to_string());
        let matcher = if is_regex && named.len() >= 2 {
            Some(unquote(text(named[1], src)))
        } else {
            None
        };

        // Normalize assertRaisesRegex -> assertRaises in skeleton
        let (ws, we) = (with_stmt.start_byte(), with_stmt.end_byte());
        let (as_pos, ae_pos) = (args.start_byte(), args.end_byte());
        let mut raw = if as_pos >= ws && ae_pos <= we {
            format!("{}(#){}", &src[ws..as_pos], &src[ae_pos..we])
        } else {
            format!("with self.assertRaises(#): {}", text(with_stmt, src))
        };
        if is_regex {
            raw = raw.replace("assertRaisesRegex", "assertRaises");
        }
        let skeleton = collapse_ws(&raw);

        attribute(
            tests,
            ExpectedException {
                line: call.start_position().row + 1,
                skeleton,
                kind: "assertRaises".to_string(),
                exception_type,
                matcher,
            },
        );
    }
}

fn inspect_python_standalone_call(call: Node, src: &str, tests: &mut [TestFn]) {
    let Some(func) = call.child_by_field_name("function") else {
        return;
    };
    let func_text = text(func, src);
    if !func_text.ends_with("assertRaises") && !func_text.ends_with("assertRaisesRegex") {
        return;
    }
    let is_regex = func_text.ends_with("assertRaisesRegex");
    let Some(args) = call.child_by_field_name("arguments") else {
        return;
    };
    let mut cursor = args.walk();
    let named: Vec<Node> = args.named_children(&mut cursor).collect();
    if named.is_empty() {
        return;
    }
    let exception_type = Some(text(named[0], src).to_string());
    let matcher = if is_regex && named.len() >= 2 {
        Some(unquote(text(named[1], src)))
    } else {
        None
    };

    let mask_end = if is_regex && named.len() >= 2 {
        named[1].end_byte()
    } else {
        named[0].end_byte()
    };
    let (cs, ce) = (call.start_byte(), call.end_byte());
    let (ms, me) = (named[0].start_byte(), mask_end);
    let mut raw = format!("{}#{}", &src[cs..ms], &src[me..ce]);
    if is_regex {
        raw = raw.replace("assertRaisesRegex", "assertRaises");
    }
    let skeleton = collapse_ws(&raw);

    attribute(
        tests,
        ExpectedException {
            line: call.start_position().row + 1,
            skeleton,
            kind: "assertRaises".to_string(),
            exception_type,
            matcher,
        },
    );
}

/// JavaScript / TypeScript: `expect(...).toThrow(...)` and `.toThrowError(...)`.
pub fn javascript(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    walk(root, &mut |node| {
        if node.kind() != "call_expression" {
            return true;
        }
        let Some(callee) = node.child_by_field_name("function") else {
            return true;
        };
        if callee.kind() != "member_expression" {
            return true;
        }
        let Some(prop) = callee.child_by_field_name("property") else {
            return true;
        };
        let prop_text = text(prop, src);
        if prop_text != "toThrow" && prop_text != "toThrowError" {
            return true;
        }
        let Some(obj) = callee.child_by_field_name("object") else {
            return true;
        };

        let mut exception_type = None;
        let mut matcher = None;

        if let Some(args) = node.child_by_field_name("arguments") {
            let mut cursor = args.walk();
            let named: Vec<Node> = args.named_children(&mut cursor).collect();
            for arg in named {
                match arg.kind() {
                    "string" | "regex" => {
                        if matcher.is_none() {
                            matcher = Some(unquote(text(arg, src)));
                        }
                    }
                    "identifier" => {
                        let id_text = text(arg, src);
                        if id_text.chars().next().is_some_and(|c| c.is_uppercase())
                            && exception_type.is_none()
                        {
                            exception_type = Some(id_text.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }

        // Skeleton: obj text + ".toThrow(#)"
        let obj_text = text(obj, src);
        let skeleton = collapse_ws(&format!("{obj_text}.toThrow(#)"));

        attribute(
            tests,
            ExpectedException {
                line: node.start_position().row + 1,
                skeleton,
                kind: "toThrow".to_string(),
                exception_type,
                matcher,
            },
        );
        true
    });
}

/// Java: `assertThrows(...)`, `assertThrowsExactly(...)`, and `@Test(expected = ...)`.
pub fn java(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    walk(root, &mut |node| {
        match node.kind() {
            "method_declaration" => {
                // Check annotations for @Test(expected = Foo.class)
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "modifiers" {
                        let mut mc = child.walk();
                        for m in child.children(&mut mc) {
                            if m.kind() == "annotation" {
                                inspect_java_annotation(m, src, tests);
                            }
                        }
                    } else if child.kind() == "annotation" {
                        inspect_java_annotation(child, src, tests);
                    }
                }
                true
            }
            "method_invocation" => {
                let Some(name) = node.child_by_field_name("name") else {
                    return true;
                };
                let name_text = text(name, src);
                if name_text != "assertThrows" && name_text != "assertThrowsExactly" {
                    return true;
                }
                let Some(args) = node.child_by_field_name("arguments") else {
                    return true;
                };
                let mut cursor = args.walk();
                let named: Vec<Node> = args.named_children(&mut cursor).collect();
                if named.is_empty() {
                    return true;
                }

                // First argument is usually the expected class: e.g. IllegalArgumentException.class
                let class_arg = named[0];
                let class_text = text(class_arg, src).trim_end_matches(".class").trim();
                let exception_type = Some(class_text.to_string());

                let (ns, ne) = (node.start_byte(), node.end_byte());
                let (cs, ce) = (class_arg.start_byte(), class_arg.end_byte());
                let mut raw = format!("{}#{}", &src[ns..cs], &src[ce..ne]);
                if name_text == "assertThrowsExactly" {
                    raw = raw.replace("assertThrowsExactly", "assertThrows");
                }
                let skeleton = collapse_ws(&raw);

                attribute(
                    tests,
                    ExpectedException {
                        line: node.start_position().row + 1,
                        skeleton,
                        kind: "assertThrows".to_string(),
                        exception_type,
                        matcher: None,
                    },
                );
                true
            }
            _ => true,
        }
    });
}

fn inspect_java_annotation(anno: Node, src: &str, tests: &mut [TestFn]) {
    let anno_text = text(anno, src);
    if !anno_text.contains("expected") {
        return;
    }
    let Some(idx) = anno_text.find("expected") else {
        return;
    };
    let rest = anno_text[idx + "expected".len()..].trim_start();
    let Some(rest) = rest.strip_prefix('=') else {
        return;
    };
    let val_str = rest
        .trim_start()
        .trim_end_matches(')')
        .trim()
        .trim_end_matches(".class")
        .trim();
    if val_str.is_empty() {
        return;
    }

    let exp = ExpectedException {
        line: anno.start_position().row + 1,
        skeleton: "@Test(expected = #)".to_string(),
        kind: "test_expected".to_string(),
        exception_type: Some(val_str.to_string()),
        matcher: None,
    };
    attribute(tests, exp);
}

/// The assertion classes of the C# pack (`csharp.rs`, `extract_assertions_in_body`).
const CSHARP_ASSERT_CLASSES: &[&str] = &[
    "Assert",
    "StringAssert",
    "CollectionAssert",
    "ClassicAssert",
];

/// C#: `Assert.Throws<...>` / `Assert.ThrowsAny<...>` and their async, exact and MSTest
/// spellings. The call is an expected exception only as a member of an assertion class:
/// `mock.Setup(..).Throws(..)` configures a double and asserts nothing.
pub fn csharp(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    walk(root, &mut |node| {
        if node.kind() != "invocation_expression" {
            return true;
        }
        let Some(expr) = node.child_by_field_name("function") else {
            return true;
        };
        if expr.kind() != "member_access_expression" {
            return true;
        }
        let (Some(receiver), Some(name)) = (
            expr.child_by_field_name("expression"),
            expr.child_by_field_name("name"),
        ) else {
            return true;
        };
        if receiver.kind() != "identifier" || !CSHARP_ASSERT_CLASSES.contains(&text(receiver, src))
        {
            return true;
        }
        // `Throws<T>` is a `generic_name`: an identifier and a `type_argument_list`.
        let (method, type_args) = if name.kind() == "generic_name" {
            let mut cursor = name.walk();
            let type_args = name
                .children(&mut cursor)
                .find(|c| c.kind() == "type_argument_list");
            (name.child(0).unwrap_or(name), type_args)
        } else {
            (name, None)
        };
        let kind = match text(method, src) {
            "ThrowsAny" | "ThrowsAnyAsync" => "Assert.ThrowsAny",
            "Throws"
            | "ThrowsAsync"
            | "ThrowsExactly"
            | "ThrowsExactlyAsync"
            | "ThrowsException"
            | "ThrowsExceptionAsync" => "Assert.Throws",
            _ => return true,
        };
        let exception_type = type_args
            .and_then(|l| l.named_child(0))
            .map(|t| text(t, src).to_string());

        let Some(args) = node.child_by_field_name("arguments") else {
            return true;
        };
        let args_text = text(args, src);
        let skeleton = collapse_ws(&format!("Assert.Throws#{args_text}"));

        attribute(
            tests,
            ExpectedException {
                line: node.start_position().row + 1,
                skeleton,
                kind: kind.to_string(),
                exception_type,
                matcher: None,
            },
        );
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ast::csharp::CSharpPack;
    use crate::ast::python::PythonPack;
    use crate::ast::{AssertVocabulary, LanguagePack};

    fn py(src: &str) -> Vec<TestFn> {
        PythonPack
            .extract("tests/test_t.py", src, &AssertVocabulary::default())
            .expect("extract succeeds")
            .tests
    }

    fn cs(body: &str) -> Vec<TestFn> {
        let src = format!(
            "using Xunit;\npublic class T {{\n    [Fact]\n    public void Run() {{\n        {body}\n    }}\n}}\n"
        );
        CSharpPack
            .extract("tests/T.cs", &src, &AssertVocabulary::default())
            .expect("extract succeeds")
            .tests
    }

    #[test]
    fn python_raises_as_form_is_read_like_the_plain_form() {
        let plain = py(
            "def test_t():\n    with pytest.raises(ValueError, match=\"neg\"):\n        f(-1)\n",
        );
        let bound = py(
            "def test_t():\n    with pytest.raises(ValueError, match=\"neg\") as e:\n        f(-1)\n",
        );
        assert_eq!(plain[0].expected_exceptions.len(), 1);
        let got = &bound[0].expected_exceptions;
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].kind, "pytest.raises");
        assert_eq!(got[0].exception_type.as_deref(), Some("ValueError"));
        assert_eq!(got[0].matcher.as_deref(), Some("neg"));
        assert_eq!(got[0].skeleton, "with pytest.raises(#) as e: f(-1)");
        assert_eq!(got[0].line, 2);
    }

    #[test]
    fn python_raises_as_form_widened_is_reported_and_unchanged_is_not() {
        let narrow = py(
            "def test_t():\n    with pytest.raises(ValueError, match=\"neg\") as e:\n        f(-1)\n",
        );
        let wide = py("def test_t():\n    with pytest.raises(Exception) as e:\n        f(-1)\n");
        let w = widened(&narrow[0].expected_exceptions, &wide[0].expected_exceptions);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].detail.contains("from `ValueError` to `Exception`"));
        assert!(widened(
            &narrow[0].expected_exceptions,
            &narrow[0].expected_exceptions
        )
        .is_empty());
    }

    #[test]
    fn python_assert_raises_as_form_is_recorded_once_on_the_with_path() {
        let src = |call: &str| {
            format!(
                "class TestC(unittest.TestCase):\n    def test_t(self):\n        with {call} as cm:\n            f(-1)\n"
            )
        };
        let narrow = py(&src("self.assertRaisesRegex(ValueError, \"neg\")"));
        let got = &narrow[0].expected_exceptions;
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].kind, "assertRaises");
        assert_eq!(got[0].exception_type.as_deref(), Some("ValueError"));
        assert_eq!(got[0].matcher.as_deref(), Some("neg"));
        assert_eq!(got[0].skeleton, "with self.assertRaises(#) as cm: f(-1)");

        let wide = py(&src("self.assertRaises(Exception)"));
        let w = widened(got, &wide[0].expected_exceptions);
        assert_eq!(w.len(), 1, "{w:?}");
    }

    #[test]
    fn csharp_mock_setup_throws_is_not_an_expected_exception() {
        let mocked = cs("mock.Setup(m => m.Get()).Throws(new Exception()); sut.Run();");
        assert!(
            mocked[0].expected_exceptions.is_empty(),
            "{:?}",
            mocked[0].expected_exceptions
        );
        assert!(mocked[0].is_vacuous());

        // The same holds for any receiver that is not an assertion class.
        for body in [
            "stub.Throws<InvalidOperationException>(); sut.Run();",
            "mock.Setup(m => m.GetAsync()).ThrowsAsync(new Exception()); sut.Run();",
            "ThrowsHelper(); sut.Run();",
            "Guard.Throws<InvalidOperationException>(() => sut.Run());",
        ] {
            let t = cs(body);
            assert!(t[0].expected_exceptions.is_empty(), "{body}");
            assert!(t[0].is_vacuous(), "{body}");
        }
    }

    #[test]
    fn csharp_assert_throws_is_an_expected_exception_and_not_vacuous() {
        for (body, kind) in [
            (
                "Assert.Throws<InvalidOperationException>(() => sut.Run());",
                "Assert.Throws",
            ),
            (
                "Assert.ThrowsAny<InvalidOperationException>(() => sut.Run());",
                "Assert.ThrowsAny",
            ),
            (
                "Assert.ThrowsExactly<InvalidOperationException>(() => sut.Run());",
                "Assert.Throws",
            ),
            (
                "ClassicAssert.Throws<InvalidOperationException>(() => sut.Run());",
                "Assert.Throws",
            ),
        ] {
            let t = cs(body);
            let got = &t[0].expected_exceptions;
            assert_eq!(got.len(), 1, "{body}: {got:?}");
            assert_eq!(got[0].kind, kind, "{body}");
            assert_eq!(got[0].skeleton, "Assert.Throws#(() => sut.Run())");
            assert!(!t[0].is_vacuous(), "{body}");
        }
        // An assertion that nothing is thrown expects no exception.
        let none = cs("Assert.DoesNotThrow(() => sut.Run());");
        assert!(none[0].expected_exceptions.is_empty());
    }

    #[test]
    fn csharp_assert_throws_reads_its_type_argument() {
        let narrow = cs("Assert.Throws<ArgumentNullException>(() => sut.Run());");
        let wide = cs("Assert.Throws<Exception>(() => sut.Run());");
        let sibling = cs("Assert.Throws<ArgumentException>(() => sut.Run());");
        let n = &narrow[0].expected_exceptions;
        assert_eq!(
            n[0].exception_type.as_deref(),
            Some("ArgumentNullException")
        );
        let w = widened(n, &wide[0].expected_exceptions);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0]
            .detail
            .contains("from `ArgumentNullException` to `Exception`"));
        assert!(widened(n, &sibling[0].expected_exceptions).is_empty());
        assert!(widened(n, n).is_empty());
    }

    #[test]
    fn test_widened_rust_panic_matcher_removed() {
        let b = vec![ExpectedException {
            line: 5,
            skeleton: "#[should_panic]".to_string(),
            kind: "should_panic".to_string(),
            exception_type: None,
            matcher: Some("overflow".to_string()),
        }];
        let h = vec![ExpectedException {
            line: 5,
            skeleton: "#[should_panic]".to_string(),
            kind: "should_panic".to_string(),
            exception_type: None,
            matcher: None,
        }];
        let w = widened(&b, &h);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].line, 5);
        assert!(w[0].detail.contains("matcher was removed"));
    }

    #[test]
    fn test_widened_python_exception_type_and_matcher() {
        let b = vec![ExpectedException {
            line: 10,
            skeleton: "with pytest.raises(#): f(-1)".to_string(),
            kind: "pytest.raises".to_string(),
            exception_type: Some("ValueError".to_string()),
            matcher: Some("negative".to_string()),
        }];
        let h = vec![ExpectedException {
            line: 10,
            skeleton: "with pytest.raises(#): f(-1)".to_string(),
            kind: "pytest.raises".to_string(),
            exception_type: Some("Exception".to_string()),
            matcher: None,
        }];
        let w = widened(&b, &h);
        assert_eq!(w.len(), 1);
        assert!(w[0].detail.contains("widened"));
    }

    #[test]
    fn test_widened_js_to_throw_type_dropped() {
        let b = vec![ExpectedException {
            line: 12,
            skeleton: "expect(() => f()).toThrow(#)".to_string(),
            kind: "toThrow".to_string(),
            exception_type: Some("TypeError".to_string()),
            matcher: None,
        }];
        let h = vec![ExpectedException {
            line: 12,
            skeleton: "expect(() => f()).toThrow(#)".to_string(),
            kind: "toThrow".to_string(),
            exception_type: None,
            matcher: None,
        }];
        let w = widened(&b, &h);
        assert_eq!(w.len(), 1);
        assert!(w[0].detail.contains("type was removed"));
    }

    #[test]
    fn test_widened_csharp_throws_any() {
        let b = vec![ExpectedException {
            line: 20,
            skeleton: "Assert.Throws#(() => {})".to_string(),
            kind: "Assert.Throws".to_string(),
            exception_type: Some("ArgumentException".to_string()),
            matcher: None,
        }];
        let h = vec![ExpectedException {
            line: 20,
            skeleton: "Assert.Throws#(() => {})".to_string(),
            kind: "Assert.ThrowsAny".to_string(),
            exception_type: Some("Exception".to_string()),
            matcher: None,
        }];
        let w = widened(&b, &h);
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn test_negative_control_unchanged() {
        let b = vec![ExpectedException {
            line: 5,
            skeleton: "#[should_panic]".to_string(),
            kind: "should_panic".to_string(),
            exception_type: None,
            matcher: Some("overflow".to_string()),
        }];
        let h = b.clone();
        assert!(widened(&b, &h).is_empty());
    }

    #[test]
    fn test_negative_control_narrowed() {
        let b = vec![ExpectedException {
            line: 10,
            skeleton: "with pytest.raises(#): f(-1)".to_string(),
            kind: "pytest.raises".to_string(),
            exception_type: Some("Exception".to_string()),
            matcher: None,
        }];
        let h = vec![ExpectedException {
            line: 10,
            skeleton: "with pytest.raises(#): f(-1)".to_string(),
            kind: "pytest.raises".to_string(),
            exception_type: Some("ValueError".to_string()),
            matcher: Some("negative".to_string()),
        }];
        assert!(widened(&b, &h).is_empty());
    }

    #[test]
    fn test_negative_control_sibling_exception() {
        let b = vec![ExpectedException {
            line: 10,
            skeleton: "with pytest.raises(#): f(-1)".to_string(),
            kind: "pytest.raises".to_string(),
            exception_type: Some("ValueError".to_string()),
            matcher: None,
        }];
        let h = vec![ExpectedException {
            line: 10,
            skeleton: "with pytest.raises(#): f(-1)".to_string(),
            kind: "pytest.raises".to_string(),
            exception_type: Some("TypeError".to_string()),
            matcher: None,
        }];
        assert!(widened(&b, &h).is_empty());
    }

    #[test]
    fn test_widened_java_assert_throws_to_ancestor() {
        let b = vec![ExpectedException {
            line: 15,
            skeleton: "assertThrows(#, () -> f())".to_string(),
            kind: "assertThrows".to_string(),
            exception_type: Some("IllegalArgumentException".to_string()),
            matcher: None,
        }];
        let h = vec![ExpectedException {
            line: 15,
            skeleton: "assertThrows(#, () -> f())".to_string(),
            kind: "assertThrows".to_string(),
            exception_type: Some("Throwable".to_string()),
            matcher: None,
        }];
        let w = widened(&b, &h);
        assert_eq!(w.len(), 1);
        assert!(w[0].detail.contains("widened"));
    }

    #[test]
    fn test_negative_control_java_sibling_exception() {
        let b = vec![ExpectedException {
            line: 15,
            skeleton: "assertThrows(#, () -> f())".to_string(),
            kind: "assertThrows".to_string(),
            exception_type: Some("IllegalArgumentException".to_string()),
            matcher: None,
        }];
        let h = vec![ExpectedException {
            line: 15,
            skeleton: "assertThrows(#, () -> f())".to_string(),
            kind: "assertThrows".to_string(),
            exception_type: Some("IllegalStateException".to_string()),
            matcher: None,
        }];
        assert!(widened(&b, &h).is_empty());
    }
}
