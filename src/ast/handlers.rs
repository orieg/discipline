//! Error handlers that swallow: `except: pass`, `catch (e) {}`, `rescue; end`, a
//! discarded `Result` (`let _ = fallible();`, `fallible().ok();`).
//!
//! A failure an agent cannot fix is easy to hide behind an empty handler; the tests then
//! pass because the error never surfaces. One walker with per-language node kinds finds
//! each handler and judges its body the way `functions.rs` judges a function body.

use tree_sitter::Node;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwallowSite {
    pub line: usize,
    /// `empty-handler` or `discarded-result`.
    pub kind: &'static str,
    /// The handler's first line, trimmed.
    pub snippet: String,
}

pub struct HandlerSpec {
    /// Node kinds that are an error handler with a body (`catch_clause`, `except_clause`).
    pub handler_kinds: &'static [&'static str],
    /// Field or child kind holding the handler body.
    pub body_fields: &'static [&'static str],
    /// Node kinds ignored when counting statements.
    pub ignored_kinds: &'static [&'static str],
    /// Statement texts that swallow when they are the whole body (after trimming `;`).
    pub trivial: &'static [&'static str],
    /// Node kinds of a statement that may discard a result (`let_declaration`,
    /// `expression_statement`); judged by `discards`.
    pub discard_kinds: &'static [&'static str],
    /// Whether a statement's text discards a result.
    pub discards: fn(&str) -> bool,
}

fn text<'a>(node: Node, src: &'a str) -> &'a str {
    node.utf8_text(src.as_bytes()).unwrap_or("")
}

fn first_line(t: &str) -> String {
    t.lines().next().unwrap_or("").trim().to_string()
}

/// Whether a handler body does nothing with the error.
fn body_swallows(body: Node, src: &str, spec: &HandlerSpec) -> bool {
    let mut cursor = body.walk();
    let stmts: Vec<Node> = body
        .named_children(&mut cursor)
        .filter(|c| !spec.ignored_kinds.contains(&c.kind()))
        .collect();
    let mut all = body.walk();
    let has_other_named = body.named_children(&mut all).next().is_some();
    match stmts.len() {
        // No statement at all (a comment inside the block does not count), or a body
        // that is a bare expression rather than a statement list.
        0 if has_other_named => true,
        0 => {
            let inner = text(body, src)
                .trim()
                .trim_start_matches('{')
                .trim_end_matches('}')
                .trim();
            inner.is_empty() || spec.trivial.contains(&inner.trim_end_matches(';').trim())
        }
        1 => {
            let t = text(stmts[0], src).trim().trim_end_matches(';').trim();
            spec.trivial.contains(&t)
        }
        _ => false,
    }
}

/// Sites in `root`, in source order. `is_test_line` excludes handlers inside tests.
pub fn extract(
    root: Node,
    src: &str,
    spec: &HandlerSpec,
    is_test_line: &dyn Fn(usize) -> bool,
) -> Vec<SwallowSite> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let line = node.start_position().row + 1;
        if spec.handler_kinds.contains(&node.kind()) && !is_test_line(line) {
            let body = spec.body_fields.iter().find_map(|f| {
                node.child_by_field_name(f).or_else(|| {
                    let mut cursor = node.walk();
                    let found = node.children(&mut cursor).find(|c| c.kind() == *f);
                    found
                })
            });
            let swallows = match body {
                Some(b) => body_swallows(b, src, spec),
                // A handler with no body node at all (`except: pass` on one line in some
                // grammars) is judged by its own text.
                None => {
                    let t = text(node, src);
                    let after = t.split_once([':', '{']).map(|(_, r)| r).unwrap_or("");
                    let after = after.trim().trim_end_matches('}').trim();
                    after.is_empty() || spec.trivial.contains(&after.trim_end_matches(';').trim())
                }
            };
            if swallows {
                out.push(SwallowSite {
                    line,
                    kind: "empty-handler",
                    snippet: first_line(text(node, src)),
                });
            }
        } else if spec.discard_kinds.contains(&node.kind()) && !is_test_line(line) {
            let t = text(node, src);
            if (spec.discards)(t) {
                out.push(SwallowSite {
                    line,
                    kind: "discarded-result",
                    snippet: first_line(t),
                });
            }
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
    out
}

pub fn no_discard(_: &str) -> bool {
    false
}

/// Rust: `let _ = f(...)` and `f(...).ok();` throw a `Result` away. A `let _ = ` binding of
/// a plain identifier or literal is not a discarded result.
pub fn rust_discards(t: &str) -> bool {
    let t = t.trim().trim_end_matches(';').trim();
    if let Some(rest) = t.strip_prefix("let _ =") {
        let rest = rest.trim();
        return rest.contains('(') && !rest.starts_with("std::mem::") && !rest.starts_with("mem::");
    }
    t.ends_with(".ok()") && !t.starts_with("let ") && !t.contains("=")
}

/// Go: `_ = err`, `_, _ = f()`, `x, _ := f()` where the dropped value is the error.
pub fn go_discards(t: &str) -> bool {
    let t = t.trim();
    if t == "_ = err" || t.starts_with("_ = err") {
        return true;
    }
    // A trailing `_` on the left of `=` / `:=` of a multi-value call drops the last value.
    if let Some((lhs, rhs)) = t.split_once([':', '=']) {
        let lhs = lhs.trim().trim_end_matches(':').trim();
        let rhs = rhs.trim_start_matches('=').trim();
        return lhs.contains(',') && lhs.ends_with('_') && rhs.contains('(');
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_and_go_discard_patterns() {
        assert!(rust_discards("let _ = file.sync_all();"));
        assert!(rust_discards("tx.commit().ok();"));
        assert!(!rust_discards("let _ = guard;"));
        assert!(!rust_discards("let _ = std::mem::replace(&mut a, b);"));
        assert!(!rust_discards("let ok = parse(s).ok();"));
        assert!(go_discards("_ = err"));
        assert!(go_discards("n, _ := w.Write(b)"));
        assert!(go_discards("_, _ = io.Copy(dst, src)"));
        assert!(!go_discards("n, err := w.Write(b)"));
        assert!(!go_discards("_ = x"));
    }
}

#[cfg(all(
    test,
    feature = "lang-python",
    feature = "lang-javascript",
    feature = "lang-java",
    feature = "lang-rust",
    feature = "lang-go"
))]
mod pack_tests {
    use crate::ast::{default_registry, AssertVocabulary, Fact};

    fn sites(path: &str, src: &str) -> Vec<(usize, &'static str)> {
        let reg = default_registry();
        let pack = reg.find_pack(path).unwrap();
        assert!(pack.supplies(Fact::Handlers));
        pack.extract(path, src, &AssertVocabulary::default())
            .unwrap()
            .swallowed
            .into_iter()
            .map(|s| (s.line, s.kind))
            .collect()
    }

    #[test]
    fn python_empty_except_is_a_site_and_a_handling_one_is_not() {
        let got = sites(
            "pkg/a.py",
            "def f():\n    try:\n        g()\n    except ValueError:\n        pass\n    try:\n        g()\n    except Exception as e:\n        log.warning(e)\n        raise\n    try:\n        g()\n    except OSError:\n        # nothing to do\n        return None\n\n\
             def test_f():\n    try:\n        f()\n    except Exception:\n        pass\n",
        );
        assert_eq!(got, vec![(4, "empty-handler"), (13, "empty-handler")]);
    }

    #[test]
    fn javascript_and_java_empty_catch() {
        let js = sites(
            "src/a.ts",
            "async function f() {\n  try { await g(); } catch (e) {}\n  try { await g(); } catch (e) { console.error(e); throw e; }\n  try { await g(); } catch { return null; }\n  try { await g(); } catch (e) {} // best effort\n}\n",
        );
        assert_eq!(
            js,
            vec![
                (2, "empty-handler"),
                (4, "empty-handler"),
                (5, "empty-handler")
            ]
        );
        let java = sites(
            "src/main/java/A.java",
            "class A {\n  void f() {\n    try { g(); } catch (IOException e) { }\n    try { g(); } catch (IOException e) { throw new RuntimeException(e); }\n  }\n}\n",
        );
        assert_eq!(java, vec![(3, "empty-handler")]);
    }

    #[test]
    fn rust_and_go_discarded_results() {
        let rs = sites(
            "src/a.rs",
            "fn f() {\n    let _ = file.sync_all();\n    tx.commit().ok();\n    let _guard = lock();\n    let v = parse(s).ok();\n}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() { let _ = f(); }\n}\n",
        );
        assert_eq!(rs, vec![(2, "discarded-result"), (3, "discarded-result")]);
        let go = sites(
            "pkg/a.go",
            "package a\nfunc F() {\n    n, _ := w.Write(b)\n    _ = err\n    n, err := w.Write(b)\n    _ = n\n}\n",
        );
        assert_eq!(go, vec![(3, "discarded-result"), (4, "discarded-result")]);
    }
}
