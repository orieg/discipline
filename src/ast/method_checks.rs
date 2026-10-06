//! Checks a test makes through a method called on a receiver: `v.done()`, where `done` is
//! a method defined in the same file whose body asserts.
//!
//! A pack follows the same-file functions a test calls by name (`check(r)`, and the calls
//! on `self` / `this` it resolves) and counts their assertions into the test. A method
//! called on some other receiver is not resolved there: the receiver's type is not known.
//! This pass reads those calls from the syntax tree and credits the test with what a
//! same-file helper of that name checks, into `TestFn::method_checks`, which only
//! `vacuous-tests` reads. The test's assertion counts are left as the pack made them, so
//! `assertion-reduction` is not affected.
//!
//! A call on the object itself (`self`, `this`, `$this`, `super`) is the pack's to read:
//! it resolves those where it follows them, and where it does not (a closure the test
//! never runs) the call is not credited here either.
//!
//! The rule: the method name is matched against the last segment of every helper the
//! pack recorded for the file (`ParsedFileFacts::test_helpers`: functions, and methods of
//! an `impl` block, a class, a trait, an extension). One helper of that name is followed,
//! with the helpers it calls, as far as a helper is followed anywhere else
//! (`HELPER_DEPTH`). Of several helpers of that name the one that checks least counts,
//! because which one the receiver runs is not known here: when one of them asserts
//! nothing, the call is credited with nothing.

use super::{helper_leaf, ParsedFileFacts};
use tree_sitter::Node;

/// How a pack's grammar spells a method call on a receiver.
pub struct ReceiverCalls {
    /// Calls whose callee is a member access: the call kind, the field holding the callee
    /// (empty: the first named child), the member-access kind, and the field of it that
    /// holds the member's name (empty: its last named child).
    pub member: &'static [(&'static str, &'static str, &'static str, &'static str)],
    /// Calls that carry the receiver and the method themselves: the call kind, the
    /// receiver field and the method field.
    pub direct: &'static [(&'static str, &'static str, &'static str)],
}

fn text<'a>(node: Node, src: &'a str) -> &'a str {
    node.utf8_text(src.as_bytes()).unwrap_or("")
}

/// The last named leaf of a name node: `done` of a Swift `.done` suffix.
fn leaf_name<'a>(node: Node, src: &'a str) -> &'a str {
    let mut cur = node;
    loop {
        let mut cursor = cur.walk();
        let last = cur.named_children(&mut cursor).last();
        match last {
            Some(child) => cur = child,
            None => return text(cur, src),
        }
    }
}

/// Receivers that are the object the calling code belongs to.
const OWN_OBJECT: &[&str] = &["self", "Self", "this", "$this", "super", "base"];

/// The method `node` calls on a receiver other than the object itself, when it is such
/// a call.
fn receiver_method<'a>(node: Node, src: &'a str, spec: &ReceiverCalls) -> Option<&'a str> {
    let other = |receiver: Option<Node>| {
        receiver.is_some_and(|r| !OWN_OBJECT.contains(&text(r, src).trim()))
    };
    let kind = node.kind();
    for (call, callee_field, member, name_field) in spec.member {
        if kind != *call {
            continue;
        }
        let callee = if callee_field.is_empty() {
            node.named_child(0)
        } else {
            node.child_by_field_name(callee_field)
        };
        let Some(callee) = callee.filter(|c| c.kind() == *member) else {
            continue;
        };
        if !other(callee.named_child(0)) {
            return None;
        }
        let name = if name_field.is_empty() {
            let mut cursor = callee.walk();
            let last = callee.named_children(&mut cursor).last();
            last
        } else {
            callee.child_by_field_name(name_field)
        };
        return name.map(|n| leaf_name(n, src));
    }
    for (call, receiver, method) in spec.direct {
        if kind == *call && other(node.child_by_field_name(receiver)) {
            return node.child_by_field_name(method).map(|n| leaf_name(n, src));
        }
    }
    None
}

/// Credits each test with the checks of the same-file helpers its receiver calls name.
/// Run after the pack has resolved the calls it follows itself, which are skipped here.
pub fn count(root: Node, src: &str, facts: &mut ParsedFileFacts, spec: &ReceiverCalls) {
    if facts.tests.is_empty() || facts.test_helpers.is_empty() {
        return;
    }
    let mut credits: Vec<(usize, usize)> = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if let Some(method) = receiver_method(node, src, spec) {
            let line = node.start_position().row + 1;
            let test = facts
                .tests
                .iter()
                .enumerate()
                .filter(|(_, t)| t.line <= line && line <= t.end_line.max(t.line))
                .min_by_key(|(_, t)| t.end_line.saturating_sub(t.line));
            if let Some((at, test)) = test {
                let counted = test
                    .counted_helper_calls
                    .iter()
                    .any(|call| helper_leaf(call) == method);
                if !counted {
                    let checks = facts.least_helper_named(method).unwrap_or(0);
                    if checks > 0 {
                        credits.push((at, checks));
                    }
                }
            }
        }
        let mut cursor = node.walk();
        stack.extend(node.children(&mut cursor));
    }
    for (at, checks) in credits {
        facts.tests[at].method_checks += checks;
    }
}

#[cfg(all(test, feature = "lang-rust", feature = "lang-python"))]
mod tests {
    use crate::ast::{default_registry, AssertVocabulary, TestFn};

    fn tests_of(path: &str, src: &str) -> Vec<TestFn> {
        default_registry()
            .find_pack(path)
            .unwrap()
            .extract(path, src, &AssertVocabulary::default())
            .unwrap()
            .tests
    }

    fn rust(methods: &str) -> TestFn {
        let src = format!(
            "struct A(Vec<u8>);\nstruct B(Vec<u8>);\n{methods}\n#[test]\nfn t() {{\n    let v = make();\n    v.done();\n}}\n"
        );
        tests_of("tests/t.rs", &src).remove(0)
    }

    const ASSERTS_TWICE: &str =
        "fn done(self) { assert_eq!(self.0.len(), 0); assert_eq!(self.0.capacity(), 0); }";
    const ASSERTS_ONCE: &str = "fn done(self) { assert_eq!(self.0.len(), 0); }";
    const ASSERTS_NOTHING: &str = "fn done(self) { drop(self.0); }";
    const TAUTOLOGY: &str = "fn done(self) { assert!(true); }";

    #[test]
    fn a_receiver_call_is_credited_with_the_one_method_of_that_name() {
        let t = rust(&format!("impl A {{ {ASSERTS_TWICE} }}"));
        assert_eq!((t.method_checks, t.total_asserts), (2, 0), "{t:?}");
        assert!(!t.is_vacuous());
        // The pack's own counts are as they were: `assertion-reduction` reads those.
        assert_eq!((t.helper_checks, t.effective_asserts()), (0, 0), "{t:?}");
        // Controls: a method that asserts nothing, one that asserts a tautology, and a
        // method of another name.
        for methods in [
            format!("impl A {{ {ASSERTS_NOTHING} }}"),
            format!("impl A {{ {TAUTOLOGY} }}"),
            format!("impl A {{ {} }}", ASSERTS_ONCE.replace("done", "finish")),
        ] {
            let t = rust(&methods);
            assert_eq!(t.method_checks, 0, "{methods}: {t:?}");
            assert!(t.is_vacuous(), "{methods}");
        }
    }

    #[test]
    fn of_several_methods_of_one_name_the_least_counts() {
        let both = |a: &str, b: &str| rust(&format!("impl A {{ {a} }}\nimpl B {{ {b} }}"));
        assert_eq!(both(ASSERTS_TWICE, ASSERTS_ONCE).method_checks, 1);
        assert_eq!(both(ASSERTS_ONCE, ASSERTS_TWICE).method_checks, 1);
        assert_eq!(both(ASSERTS_TWICE, ASSERTS_NOTHING).method_checks, 0);
        assert_eq!(both(ASSERTS_NOTHING, ASSERTS_TWICE).method_checks, 0);
        // A trait's default method and an implementation of it are two of one name.
        let t = rust(&format!(
            "trait Done {{ {ASSERTS_ONCE} }}\nimpl Done for A {{ {ASSERTS_NOTHING} }}"
        ));
        assert_eq!(t.method_checks, 0, "{t:?}");
    }

    #[test]
    fn a_method_is_followed_as_deep_as_a_helper_is() {
        let chain = |last: &str| {
            rust(&format!(
                "fn third(v: &[u8]) {{ {last} }}\nfn second(v: &[u8]) {{ third(v); }}\nimpl A {{ fn done(self) {{ second(&self.0); }} }}"
            ))
        };
        // The method, the helper it calls and the helper that one calls: `HELPER_DEPTH`.
        assert_eq!(chain("assert_eq!(v.len(), 0);").method_checks, 1);
        assert_eq!(chain("let _ = v;").method_checks, 0);
        // One call further is not followed, for a method as for a function.
        let t = rust(
            "fn fourth(v: &[u8]) { assert_eq!(v.len(), 0); }\nfn third(v: &[u8]) { fourth(v); }\nfn second(v: &[u8]) { third(v); }\nimpl A { fn done(self) { second(&self.0); } }",
        );
        assert_eq!(t.method_checks, 0, "{t:?}");
    }

    /// A call the pack already counted into the test is not counted a second time.
    #[test]
    fn a_call_the_pack_follows_itself_is_not_credited_again() {
        let t = tests_of(
            "tests/test_t.py",
            "class TestA:\n    def check(self, r):\n        assert r.a == 1\n\n    def test_a(self):\n        self.check(make())\n",
        )
        .remove(0);
        assert_eq!((t.total_asserts, t.method_checks), (1, 0), "{t:?}");
        // A helper the pack counted through one call (`C::check(&c)`) is not credited
        // again for a receiver call of the same name beside it.
        let t = tests_of(
            "tests/t.rs",
            "struct C;\nimpl C { fn check(&self) { assert_eq!(1, one()); } }\n#[test]\nfn t() { let c = C; c.check(); C::check(&c); }\n",
        )
        .remove(0);
        assert_eq!((t.total_asserts, t.method_checks), (1, 0), "{t:?}");
        // Control: the same method called on another object is credited here, and only here.
        let t = tests_of(
            "tests/test_t.py",
            "class Checker:\n    def check(self, r):\n        assert r.a == 1\n\ndef test_a():\n    Checker().check(make())\n",
        )
        .remove(0);
        assert_eq!((t.total_asserts, t.method_checks), (0, 1), "{t:?}");
    }
}
