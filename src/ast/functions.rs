//! Function-body facts shared by the language packs: which functions a file defines and
//! whether each body does anything.
//!
//! One walker, one classifier; per-language tables name the function node kinds, where
//! the body sits, what counts as a stub marker (`todo!()`, `raise NotImplementedError`,
//! `throw new UnsupportedOperationException`) and what counts as a trivial body
//! (`return null`, `pass`). The `stub-bodies` gate compares these facts base against head.

use tree_sitter::Node;

/// What a function body amounts to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyShape {
    /// The whole body is a not-implemented marker; `String` is the marker text.
    Stub(String),
    /// No statements at all.
    Empty,
    /// One statement that returns a constant or nothing (`return null`, `pass`).
    Trivial(String),
    /// Anything else.
    Substantive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionFacts {
    pub name: String,
    pub line: usize,
    pub end_line: usize,
    pub shape: BodyShape,
    /// A test function or a test-file member: judged by the test gates, not here.
    pub is_test: bool,
}

/// Per-language description of function nodes.
pub struct FunctionSpec {
    /// Node kinds that define a function or method with a body.
    pub function_kinds: &'static [&'static str],
    /// Field holding the name, tried in order; the first present wins.
    pub name_fields: &'static [&'static str],
    /// Field holding the body, tried in order (`body`, or a kind name for a child).
    pub body_fields: &'static [&'static str],
    /// Node kinds ignored when counting statements (comments, docstrings).
    pub ignored_kinds: &'static [&'static str],
    /// Whether the node is an abstract, overload or interface member: no body to judge.
    pub skip: fn(Node, &str) -> bool,
    /// Whether the function is a test.
    pub is_test: fn(Node, &str, &str) -> bool,
    /// Classify a single statement's text. `None` = substantive.
    pub classify: fn(&str) -> Option<BodyShape>,
}

fn text<'a>(node: Node, src: &'a str) -> &'a str {
    node.utf8_text(src.as_bytes()).unwrap_or("")
}

/// Walk `root` and collect every function `spec` describes.
pub fn extract(root: Node, src: &str, path: &str, spec: &FunctionSpec) -> Vec<FunctionFacts> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if spec.function_kinds.contains(&node.kind()) {
            if let Some(f) = describe(node, src, path, spec) {
                out.push(f);
            }
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        // Push in reverse so the walk visits in source order.
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
    out
}

fn describe(node: Node, src: &str, path: &str, spec: &FunctionSpec) -> Option<FunctionFacts> {
    if (spec.skip)(node, src) {
        return None;
    }
    let name = spec
        .name_fields
        .iter()
        .find_map(|f| node.child_by_field_name(f))
        .map(|n| text(n, src).trim().to_string())
        .filter(|n| !n.is_empty())
        .or_else(|| declarator_name(node, src))?;
    let body = spec.body_fields.iter().find_map(|f| {
        node.child_by_field_name(f).or_else(|| {
            let mut cursor = node.walk();
            let found = node.children(&mut cursor).find(|c| c.kind() == *f);
            found
        })
    })?;
    let shape = classify_body(body, src, spec);
    Some(FunctionFacts {
        name,
        line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        shape,
        is_test: (spec.is_test)(node, src, path),
    })
}

/// `const f = () => {}` and `let g = function() {}`: the name is on the declarator.
fn declarator_name(node: Node, src: &str) -> Option<String> {
    let parent = node.parent()?;
    if parent.kind() != "variable_declarator" && parent.kind() != "pair" {
        return None;
    }
    parent
        .child_by_field_name("name")
        .or_else(|| parent.child_by_field_name("key"))
        .map(|n| text(n, src).trim().trim_matches(['"', '\'']).to_string())
}

/// Statements of a body, ignoring comments, docstrings and structural punctuation.
fn statements<'a>(body: Node<'a>, spec: &FunctionSpec) -> Vec<Node<'a>> {
    let mut cursor = body.walk();
    body.named_children(&mut cursor)
        .filter(|c| !spec.ignored_kinds.contains(&c.kind()))
        // A docstring: an expression statement that is only a string literal.
        .filter(|c| {
            !(c.kind() == "expression_statement"
                && c.named_child_count() == 1
                && c.named_child(0).is_some_and(|s| s.kind() == "string"))
        })
        .collect()
}

pub fn classify_body(body: Node, src: &str, spec: &FunctionSpec) -> BodyShape {
    // An expression body (`=> throw new NotImplementedException()`, Ruby's last
    // expression) has no statement list: judge its text directly.
    let stmts = statements(body, spec);
    match stmts.len() {
        0 => {
            let t = text(body, src).trim();
            let inner = t
                .trim_start_matches(['{', '='])
                .trim_start_matches('>')
                .trim_end_matches('}')
                .trim();
            if inner.is_empty() {
                BodyShape::Empty
            } else {
                (spec.classify)(inner).unwrap_or(BodyShape::Substantive)
            }
        }
        1 => {
            let t = text(stmts[0], src).trim();
            (spec.classify)(t).unwrap_or(BodyShape::Substantive)
        }
        _ => BodyShape::Substantive,
    }
}

fn has_word(hay: &str, needles: &[&str]) -> bool {
    let h = hay.to_ascii_lowercase();
    needles.iter().any(|n| h.contains(n))
}

const NOT_IMPLEMENTED_WORDS: &[&str] = &[
    "not implemented",
    "notimplemented",
    "unimplemented",
    "not yet implemented",
    "implement me",
    "todo",
    "tbd",
    "stub",
];

fn strip_semicolon(t: &str) -> &str {
    t.trim().trim_end_matches(';').trim()
}

fn trivial_return(t: &str, constants: &[&str]) -> Option<BodyShape> {
    let t = strip_semicolon(t);
    let rest = t.strip_prefix("return")?.trim();
    if rest.is_empty() || constants.contains(&rest) {
        return Some(BodyShape::Trivial(t.to_string()));
    }
    None
}

pub fn classify_rust(t: &str) -> Option<BodyShape> {
    let t = strip_semicolon(t);
    if t.starts_with("todo!") || t.starts_with("unimplemented!") {
        return Some(BodyShape::Stub(t.to_string()));
    }
    if t.starts_with("panic!") && has_word(t, NOT_IMPLEMENTED_WORDS) {
        return Some(BodyShape::Stub(t.to_string()));
    }
    const TRIVIAL: &[&str] = &[
        "None",
        "Ok(())",
        "Default::default()",
        "Vec::new()",
        "vec![]",
        "String::new()",
        "0",
        "false",
        "true",
        "\"\"",
        "()",
    ];
    if TRIVIAL.contains(&t) {
        return Some(BodyShape::Trivial(t.to_string()));
    }
    trivial_return(t, TRIVIAL)
}

pub fn classify_python(t: &str) -> Option<BodyShape> {
    let t = t.trim();
    if t == "pass" || t == "..." || t.starts_with("raise NotImplemented") {
        return Some(BodyShape::Stub(t.to_string()));
    }
    trivial_return(
        t,
        &["None", "{}", "[]", "\"\"", "''", "0", "False", "True", "()"],
    )
}

pub fn classify_javascript(t: &str) -> Option<BodyShape> {
    let t = strip_semicolon(t);
    if t.starts_with("throw ") && has_word(t, NOT_IMPLEMENTED_WORDS) {
        return Some(BodyShape::Stub(t.to_string()));
    }
    trivial_return(
        t,
        &[
            "null",
            "undefined",
            "{}",
            "[]",
            "\"\"",
            "''",
            "``",
            "0",
            "false",
            "true",
        ],
    )
}

pub fn classify_go(t: &str) -> Option<BodyShape> {
    let t = strip_semicolon(t);
    if t.starts_with("panic(") && has_word(t, NOT_IMPLEMENTED_WORDS) {
        return Some(BodyShape::Stub(t.to_string()));
    }
    trivial_return(
        t,
        &[
            "nil",
            "nil, nil",
            "0",
            "\"\"",
            "false",
            "true",
            "0, nil",
            "\"\", nil",
            "false, nil",
        ],
    )
}

pub fn classify_jvm(t: &str) -> Option<BodyShape> {
    let t = strip_semicolon(t);
    if t.starts_with("throw ")
        && (t.contains("UnsupportedOperationException")
            || t.contains("NotImplementedException")
            || t.contains("NotSupportedException")
            || has_word(t, NOT_IMPLEMENTED_WORDS))
    {
        return Some(BodyShape::Stub(t.to_string()));
    }
    trivial_return(
        t,
        &[
            "null",
            "0",
            "0L",
            "0.0",
            "false",
            "true",
            "\"\"",
            "default",
            "Optional.empty()",
        ],
    )
}

pub fn classify_php(t: &str) -> Option<BodyShape> {
    let t = strip_semicolon(t);
    if t.starts_with("throw ") && has_word(t, NOT_IMPLEMENTED_WORDS) {
        return Some(BodyShape::Stub(t.to_string()));
    }
    trivial_return(t, &["null", "[]", "0", "''", "\"\"", "false", "true"])
}

pub fn classify_ruby(t: &str) -> Option<BodyShape> {
    let t = t.trim();
    if t.starts_with("raise NotImplementedError") || t == "raise NotImplementedError" {
        return Some(BodyShape::Stub(t.to_string()));
    }
    if ["nil", "[]", "{}", "0", "''", "\"\"", "false", "true"].contains(&t) {
        return Some(BodyShape::Trivial(t.to_string()));
    }
    trivial_return(t, &["nil", "[]", "{}", "0", "false", "true"])
}

pub fn classify_c(t: &str) -> Option<BodyShape> {
    let t = strip_semicolon(t);
    if t == "abort()"
        || t.starts_with("assert(false)")
        || t.starts_with("assert(0)")
        || (t.starts_with("assert(") && has_word(t, NOT_IMPLEMENTED_WORDS))
        || t.starts_with("throw std::logic_error") && has_word(t, NOT_IMPLEMENTED_WORDS)
    {
        return Some(BodyShape::Stub(t.to_string()));
    }
    trivial_return(t, &["0", "nullptr", "NULL", "false", "true", "{}", "\"\""])
}

/// Never skip.
pub fn skip_none(_: Node, _: &str) -> bool {
    false
}

/// Not a test.
pub fn test_none(_: Node, _: &str, _: &str) -> bool {
    false
}

/// Whether `path` matches one of the repository's declared test-scope globs.
pub fn declared_test_path(path: &str, globs: &[String]) -> bool {
    globs.iter().any(|g| {
        globset::Glob::new(g)
            .map(|g| g.compile_matcher().is_match(path))
            .unwrap_or(false)
    })
}

/// A path under a test directory or with a test suffix. Cargo's `benches/` and
/// `examples/` are compiled as their own crates and are not shipped code.
pub fn test_path(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.contains("/tests/")
        || p.contains("/benches/")
        || p.contains("/examples/")
        || p.starts_with("benches/")
        || p.starts_with("examples/")
        || p.contains("/test/")
        || p.starts_with("tests/")
        || p.starts_with("test/")
        || p.contains("/__tests__/")
        || p.ends_with("_test.go")
        || p.ends_with("_test.py")
        || p.ends_with(".test.ts")
        || p.ends_with(".test.js")
        || p.ends_with(".spec.ts")
        || p.ends_with(".spec.js")
        || p.ends_with("test.java")
        || p.ends_with("tests.cs")
        || p.ends_with("_spec.rb")
        || p.ends_with("_test.rb")
        || p.ends_with("test.php")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifiers_tell_stubs_from_trivial_and_substantive_bodies() {
        assert!(matches!(classify_rust("todo!()"), Some(BodyShape::Stub(_))));
        assert!(matches!(
            classify_rust("unimplemented!(\"later\");"),
            Some(BodyShape::Stub(_))
        ));
        assert!(matches!(
            classify_rust("panic!(\"not implemented\")"),
            Some(BodyShape::Stub(_))
        ));
        assert!(classify_rust("panic!(\"index out of range\")").is_none());
        assert!(matches!(classify_rust("None"), Some(BodyShape::Trivial(_))));
        assert!(classify_rust("self.inner.len()").is_none());

        assert!(matches!(
            classify_python("raise NotImplementedError(\"soon\")"),
            Some(BodyShape::Stub(_))
        ));
        assert!(matches!(classify_python("..."), Some(BodyShape::Stub(_))));
        assert!(matches!(
            classify_python("return None"),
            Some(BodyShape::Trivial(_))
        ));
        assert!(classify_python("return compute(x)").is_none());

        assert!(matches!(
            classify_javascript("throw new Error(\"Not implemented\");"),
            Some(BodyShape::Stub(_))
        ));
        assert!(classify_javascript("throw new Error(\"bad input\");").is_none());
        assert!(matches!(
            classify_javascript("return null;"),
            Some(BodyShape::Trivial(_))
        ));

        assert!(matches!(
            classify_go("panic(\"TODO\")"),
            Some(BodyShape::Stub(_))
        ));
        assert!(matches!(
            classify_go("return nil, nil"),
            Some(BodyShape::Trivial(_))
        ));
        assert!(matches!(
            classify_jvm("throw new UnsupportedOperationException();"),
            Some(BodyShape::Stub(_))
        ));
        assert!(matches!(
            classify_jvm("return null;"),
            Some(BodyShape::Trivial(_))
        ));
        assert!(matches!(
            classify_ruby("raise NotImplementedError"),
            Some(BodyShape::Stub(_))
        ));
        assert!(matches!(
            classify_php("throw new \\RuntimeException('not implemented');"),
            Some(BodyShape::Stub(_))
        ));
        assert!(matches!(classify_c("abort()"), Some(BodyShape::Stub(_))));
    }
}

#[cfg(all(
    test,
    feature = "lang-rust",
    feature = "lang-python",
    feature = "lang-javascript",
    feature = "lang-go",
    feature = "lang-java",
    feature = "lang-csharp"
))]
mod pack_tests {
    use super::BodyShape;
    use crate::ast::{default_registry, AssertVocabulary, Fact};

    fn shapes(path: &str, src: &str) -> Vec<(String, BodyShape, bool)> {
        let reg = default_registry();
        let pack = reg.find_pack(path).expect("pack");
        assert!(pack.supplies(Fact::Functions));
        pack.extract(path, src, &AssertVocabulary::default())
            .unwrap()
            .functions
            .into_iter()
            .map(|f| (f.name, f.shape, f.is_test))
            .collect()
    }

    fn stub(s: &str) -> BodyShape {
        BodyShape::Stub(s.into())
    }

    #[test]
    fn rust_functions_and_tests_and_cfg_test_modules() {
        let got = shapes(
            "src/lib.rs",
            "pub fn a() -> u8 { todo!() }\nfn b() {}\nfn c() -> Option<u8> { None }\n\
             fn d(x: u8) -> u8 { x + 1 }\n#[test]\nfn t() { todo!() }\n\
             #[cfg(test)]\nmod tests { fn helper() { unimplemented!() } }\n\
             trait T { fn m(&self) { todo!() } }\n",
        );
        assert_eq!(
            got,
            vec![
                ("a".into(), stub("todo!()"), false),
                ("b".into(), BodyShape::Empty, false),
                ("c".into(), BodyShape::Trivial("None".into()), false),
                ("d".into(), BodyShape::Substantive, false),
                ("t".into(), stub("todo!()"), true),
                ("m".into(), stub("todo!()"), false),
            ]
        );
    }

    #[test]
    fn python_docstrings_protocols_and_abstract_methods() {
        let got = shapes(
            "pkg/a.py",
            "from typing import Protocol\nfrom abc import abstractmethod\n\
             def a():\n    \"\"\"doc\"\"\"\n    raise NotImplementedError\n\
             def b(x):\n    \"\"\"doc\"\"\"\n    return x * 2\n\
             class P(Protocol):\n    def m(self): ...\n\
             class C:\n    @abstractmethod\n    def n(self): ...\n    def o(self):\n        pass\n\
             def test_z():\n    pass\n",
        );
        assert_eq!(
            got,
            vec![
                ("a".into(), stub("raise NotImplementedError"), false),
                ("b".into(), BodyShape::Substantive, false),
                ("o".into(), stub("pass"), false),
                ("test_z".into(), stub("pass"), true),
            ]
        );
    }

    #[test]
    fn javascript_declarations_methods_arrows_and_test_callbacks() {
        let got = shapes(
            "src/a.ts",
            "export function a() { throw new Error('Not implemented'); }\n\
             const b = () => null;\nconst c = (x: number) => { return x + 1; };\n\
             class K { m() { return null; } abstract n(): void; }\n\
             it('works', () => { expect(1).toBe(1); });\n",
        );
        let names: Vec<(String, bool)> = got.iter().map(|(n, _, t)| (n.clone(), *t)).collect();
        assert!(names.contains(&("a".into(), false)));
        assert!(matches!(&got[0].1, BodyShape::Stub(_)), "{got:?}");
        let m = got.iter().find(|(n, _, _)| n == "m").unwrap();
        assert!(matches!(m.1, BodyShape::Trivial(_)));
        let c = got.iter().find(|(n, _, _)| n == "c").unwrap();
        assert_eq!(c.1, BodyShape::Substantive);
        assert!(
            !got.iter().any(|(n, _, _)| n == "n"),
            "abstract member skipped: {got:?}"
        );
    }

    #[test]
    fn go_java_and_csharp_bodies() {
        let go = shapes(
            "pkg/a.go",
            "package a\nfunc A() error { panic(\"not implemented\") }\nfunc (s *S) B() (int, error) { return 0, nil }\nfunc C() int { return compute() }\n",
        );
        assert!(matches!(go[0].1, BodyShape::Stub(_)));
        assert!(matches!(go[1].1, BodyShape::Trivial(_)));
        assert_eq!(go[2].1, BodyShape::Substantive);

        let java = shapes(
            "src/main/java/A.java",
            "abstract class A {\n  abstract void x();\n  int y() { throw new UnsupportedOperationException(); }\n  int z() { return 1 + 2; }\n  @Test void t() { }\n}\n",
        );
        assert_eq!(java.len(), 3, "{java:?}");
        assert!(matches!(java[0].1, BodyShape::Stub(_)));
        assert_eq!(java[1].1, BodyShape::Substantive);
        assert!(java[2].2);

        let cs = shapes(
            "src/A.cs",
            "class A {\n  public int X() => throw new NotImplementedException();\n  public int Y() { return 0; }\n  public abstract int Z();\n}\ninterface I { int W(); }\n",
        );
        assert_eq!(cs.len(), 2, "{cs:?}");
        assert!(matches!(cs[0].1, BodyShape::Stub(_)), "{cs:?}");
        assert!(matches!(cs[1].1, BodyShape::Trivial(_)));
    }
}
