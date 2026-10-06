//! Helper calls whose check runs in a loop: `for r in rows: check(r)`, or a helper that
//! asserts inside a loop of its own.
//!
//! `assertion-reduction` holds a same-file helper to the checks it has: three inline
//! assertions replaced by one call to a helper holding one are a drop of two. One check
//! that runs once per element is the exception, since it can stand for any number of
//! inline assertions, so a drop that comes with more such calls is read as a refactor
//! and noted. This pass counts them, from the syntax tree, into
//! `HelperReach::looped`.
//!
//! A node is in a loop when, between it and the function it belongs to, it sits in
//!
//! - the body of a loop statement or of a comprehension (`LoopSpec::loops`); the loop's
//!   header (the collection, the condition) is not its body; or
//! - a closure (`LoopSpec::closures`) passed to a call of an iterator method
//!   (`LoopSpec::iterators`: `.forEach(...)`, `.each do ... end`, `.for_each(|x| ...)`).
//!
//! A counted helper call (`TestFn::counted_helper_calls`, a call to a same-file helper
//! that checks) is a looped one when a call of that name in the test is in a loop, or
//! when every same-file helper of that name has a check candidate in a loop of its body.
//! A check candidate is a call or a failure statement (`LoopSpec::calls`,
//! `LoopSpec::exits`): which calls assert is the pack's to know, so a loop in a helper
//! that only calls something else is read as a looping check too. That errs toward the
//! reading the gate had before for every helper; a helper with no loop is not affected.
//! An assertion spelled as an infix operator (`x shouldBe y`) is not a candidate.
//!
//! The same walk counts, per helper, the failure exits an equality comparison guards
//! (`if a != b { panic!() }`, `raise X unless a == b`, `if a == b { .. } else { throw }`):
//! such an exit is an equality check written by hand. A pack counts a failure exit as a
//! check and never as an equality check, so an `assert_eq!` rewritten that way in a
//! helper would read as weakened. `HelperReach::equality_exits` is what
//! `assertion-reduction` sets against a strength-only shortfall. An exit under any other
//! condition (`<`, a truthiness test, a call, none) is not counted.

use super::{helper_leaf, ParsedFileFacts};
use tree_sitter::Node;

/// How a pack's grammar spells loops and calls.
pub struct LoopSpec {
    /// Loop statements and comprehensions.
    pub loops: &'static [&'static str],
    /// Closures, lambdas and blocks passed to a call.
    pub closures: &'static [&'static str],
    /// Methods and functions that run a closure once per element.
    pub iterators: &'static [&'static str],
    /// Calls: the node kind and the field holding the callee (empty: the first named
    /// child).
    pub calls: &'static [(&'static str, &'static str)],
    /// Statements that fail without being a call (`raise`, `throw`, `assert`).
    pub exits: &'static [&'static str],
    /// Functions and macros whose call is a failure exit (`panic!`, `abort()`).
    pub exit_calls: &'static [&'static str],
}

/// Conditionals: the exit they guard is in a branch, the comparison in `condition`.
const CONDITIONALS: &[&str] = &[
    "if_statement",
    "if_expression",
    "if",
    "unless",
    "if_modifier",
    "unless_modifier",
    "guard_statement",
];

/// Equality and inequality operators, as the token of a binary comparison.
const EQUALITY_OPERATORS: &[&str] = &["==", "!=", "===", "!=="];

/// Methods and functions that compare for equality (`!a.equals(b)`).
const EQUALITY_METHODS: &[&str] = &[
    "eq",
    "ne",
    "equals",
    "Equals",
    "eql?",
    "equal?",
    "isEqual",
    "isEqualToString",
    "DeepEqual",
    "Equal",
];

const C_LOOPS: &[&str] = &["for_statement", "while_statement", "do_statement"];
const EACH: &[&str] = &["forEach", "for_each", "each", "foreach", "ForEach"];

pub const PYTHON: LoopSpec = LoopSpec {
    loops: &[
        "for_statement",
        "while_statement",
        "list_comprehension",
        "set_comprehension",
        "dictionary_comprehension",
        "generator_expression",
    ],
    closures: &[],
    iterators: &[],
    calls: &[("call", "function")],
    exits: &["raise_statement", "assert_statement"],
    exit_calls: &[],
};

pub const RUST: LoopSpec = LoopSpec {
    loops: &["for_expression", "while_expression", "loop_expression"],
    closures: &["closure_expression"],
    iterators: &["for_each", "try_for_each", "all", "any"],
    calls: &[
        ("call_expression", "function"),
        ("macro_invocation", "macro"),
    ],
    exits: &[],
    exit_calls: &["panic", "unreachable"],
};

pub const GO: LoopSpec = LoopSpec {
    loops: &["for_statement"],
    closures: &[],
    iterators: &[],
    calls: &[("call_expression", "function")],
    exits: &[],
    exit_calls: &["panic"],
};

pub const JAVASCRIPT: LoopSpec = LoopSpec {
    loops: &[
        "for_statement",
        "for_in_statement",
        "while_statement",
        "do_statement",
    ],
    closures: &["arrow_function", "function_expression"],
    iterators: &["forEach", "every", "some", "map"],
    calls: &[("call_expression", "function")],
    exits: &["throw_statement"],
    exit_calls: &[],
};

pub const JAVA: LoopSpec = LoopSpec {
    loops: &[
        "for_statement",
        "enhanced_for_statement",
        "while_statement",
        "do_statement",
    ],
    closures: &["lambda_expression"],
    iterators: &["forEach", "forEachOrdered", "allMatch", "anyMatch"],
    calls: &[("method_invocation", "name")],
    exits: &["throw_statement", "assert_statement"],
    exit_calls: &[],
};

pub const CSHARP: LoopSpec = LoopSpec {
    loops: &[
        "for_statement",
        "foreach_statement",
        "while_statement",
        "do_statement",
    ],
    closures: &["lambda_expression", "anonymous_method_expression"],
    iterators: &["ForEach", "All", "Any"],
    calls: &[("invocation_expression", "function")],
    exits: &["throw_statement", "throw_expression"],
    exit_calls: &[],
};

pub const KOTLIN: LoopSpec = LoopSpec {
    loops: &["for_statement", "while_statement", "do_while_statement"],
    closures: &["lambda_literal"],
    iterators: &[
        "forEach",
        "forEachIndexed",
        "onEach",
        "all",
        "any",
        "repeat",
    ],
    calls: &[("call_expression", "")],
    exits: &["throw_expression"],
    exit_calls: &[],
};

pub const SWIFT: LoopSpec = LoopSpec {
    loops: &["for_statement", "while_statement", "repeat_while_statement"],
    closures: &["lambda_literal"],
    iterators: &["forEach", "allSatisfy"],
    calls: &[("call_expression", ""), ("macro_invocation", "")],
    exits: &["throw_keyword"],
    exit_calls: &["fatalError", "preconditionFailure"],
};

pub const SCALA: LoopSpec = LoopSpec {
    loops: &["for_expression", "while_expression", "do_while_expression"],
    closures: &["lambda_expression"],
    iterators: &["foreach", "forall", "exists"],
    calls: &[("call_expression", "function")],
    exits: &["throw_expression"],
    exit_calls: &[],
};

pub const RUBY: LoopSpec = LoopSpec {
    loops: &["for", "while", "until"],
    closures: &["block", "do_block"],
    iterators: &[
        "each",
        "each_with_index",
        "each_pair",
        "each_key",
        "each_value",
        "each_slice",
        "times",
        "upto",
        "downto",
        "all?",
        "any?",
    ],
    calls: &[("call", "method")],
    exits: &[],
    exit_calls: &["raise", "fail"],
};

pub const PHP: LoopSpec = LoopSpec {
    loops: &[
        "for_statement",
        "foreach_statement",
        "while_statement",
        "do_statement",
    ],
    closures: &["anonymous_function", "arrow_function"],
    iterators: &["array_map", "array_walk"],
    calls: &[
        ("function_call_expression", "function"),
        ("member_call_expression", "name"),
        ("nullsafe_member_call_expression", "name"),
        ("scoped_call_expression", "name"),
    ],
    exits: &["throw_expression"],
    exit_calls: &[],
};

pub const C: LoopSpec = LoopSpec {
    loops: &[
        "for_statement",
        "for_range_loop",
        "while_statement",
        "do_statement",
    ],
    closures: &["lambda_expression"],
    iterators: EACH,
    calls: &[("call_expression", "function")],
    exits: &["throw_statement"],
    exit_calls: &["abort", "exit", "terminate"],
};

pub const OBJC: LoopSpec = LoopSpec {
    loops: C_LOOPS,
    closures: &[],
    iterators: &[],
    calls: &[
        ("call_expression", "function"),
        ("message_expression", "method"),
    ],
    exits: &["throw_statement"],
    exit_calls: &["abort"],
};

fn text<'a>(node: Node, src: &'a str) -> &'a str {
    node.utf8_text(src.as_bytes()).unwrap_or("")
}

/// The last named leaf under `node`: the name a callee ends in (`check` of `a.b.check`).
fn last_leaf<'a>(node: Node, src: &'a str) -> &'a str {
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

/// The callee of `node`, when `node` is a call.
fn callee<'t>(node: Node<'t>, spec: &LoopSpec) -> Option<Node<'t>> {
    let (_, field) = spec.calls.iter().find(|(kind, _)| *kind == node.kind())?;
    if field.is_empty() {
        node.named_child(0)
    } else {
        node.child_by_field_name(field)
    }
}

/// `child` is the body of the loop `looped`: its `body` field, or, in a grammar that
/// names no body, its last named child other than the condition.
fn is_loop_body(looped: Node, child: Node) -> bool {
    if let Some(body) = looped.child_by_field_name("body") {
        return body.id() == child.id();
    }
    let condition = looped.child_by_field_name("condition").map(|c| c.id());
    let mut cursor = looped.walk();
    let last = looped
        .named_children(&mut cursor)
        .filter(|c| Some(c.id()) != condition)
        .last();
    last.is_some_and(|l| l.id() == child.id())
}

/// The closure is an argument of a call to an iterator method.
fn runs_per_element(closure: Node, src: &str, spec: &LoopSpec) -> bool {
    let mut cur = closure.parent();
    for _ in 0..4 {
        let Some(node) = cur else {
            return false;
        };
        if let Some(callee) = callee(node, spec) {
            return closure.start_byte() >= callee.end_byte()
                && spec.iterators.contains(&last_leaf(callee, src));
        }
        cur = node.parent();
    }
    false
}

/// `node` sits in a loop that starts at or after line `floor`, the first line of the
/// function it belongs to.
fn in_loop(node: Node, floor: usize, src: &str, spec: &LoopSpec) -> bool {
    let mut child = node;
    let mut cur = node.parent();
    while let Some(parent) = cur {
        if parent.start_position().row + 1 < floor {
            return false;
        }
        let kind = parent.kind();
        if spec.loops.contains(&kind) && is_loop_body(parent, child) {
            return true;
        }
        if spec.closures.contains(&kind) && runs_per_element(parent, src, spec) {
            return true;
        }
        child = parent;
        cur = parent.parent();
    }
    false
}

/// The condition is an equality or inequality comparison, possibly negated or
/// parenthesized: a binary node with an equality operator token, or a call of an
/// equality method.
fn compares_for_equality(condition: Node, src: &str, spec: &LoopSpec) -> bool {
    let mut node = condition;
    for _ in 0..6 {
        if let Some(callee) = callee(node, spec) {
            return EQUALITY_METHODS.contains(&last_leaf(callee, src));
        }
        let mut cursor = node.walk();
        let operator = node.children(&mut cursor).any(|c| {
            EQUALITY_OPERATORS.contains(&c.kind())
                || (c.child_count() == 0 && EQUALITY_OPERATORS.contains(&text(c, src)))
        });
        if operator {
            return true;
        }
        // A wrapper around one expression: parentheses, a negation, a condition clause.
        if node.named_child_count() != 1 {
            return false;
        }
        match node.named_child(0) {
            Some(inner) => node = inner,
            None => return false,
        }
    }
    false
}

/// `node` is a failure exit: a `raise` / `throw` statement, or a call of a function that
/// does not return (`panic!`, `abort()`).
fn is_failure_exit(node: Node, src: &str, spec: &LoopSpec) -> bool {
    let kind = node.kind();
    if kind != "assert_statement" && spec.exits.contains(&kind) {
        return true;
    }
    callee(node, spec).is_some_and(|c| spec.exit_calls.contains(&last_leaf(c, src)))
}

/// The nearest conditional that holds `node` in one of its branches, at or after line
/// `floor`, compares for equality.
fn guarded_by_equality(node: Node, floor: usize, src: &str, spec: &LoopSpec) -> bool {
    let mut child = node;
    let mut cur = node.parent();
    while let Some(parent) = cur {
        if parent.start_position().row + 1 < floor {
            return false;
        }
        if CONDITIONALS.contains(&parent.kind()) {
            let condition = parent
                .child_by_field_name("condition")
                .or_else(|| parent.named_child(0));
            return condition
                .is_some_and(|c| c.id() != child.id() && compares_for_equality(c, src, spec));
        }
        child = parent;
        cur = parent.parent();
    }
    false
}

/// The innermost of `spans` (first and last line) that holds `line`.
fn innermost(spans: &[(usize, usize)], line: usize) -> Option<usize> {
    spans
        .iter()
        .enumerate()
        .filter(|(_, (first, last))| *first <= line && line <= (*last).max(*first))
        .min_by_key(|(_, (first, last))| last.saturating_sub(*first))
        .map(|(at, _)| at)
}

/// Counts each test's looped helper calls into `HelperReach::looped`, and the
/// equality-guarded failure exits of the helpers it calls into
/// `HelperReach::equality_exits`. Run after the pack
/// has resolved the same-file helpers its tests call.
pub fn count(root: Node, src: &str, facts: &mut ParsedFileFacts, spec: &LoopSpec) {
    if facts
        .tests
        .iter()
        .all(|t| t.counted_helper_calls.is_empty())
    {
        return;
    }
    let tests: Vec<(usize, usize)> = facts.tests.iter().map(|t| (t.line, t.end_line)).collect();
    let helpers: Vec<(usize, usize)> = facts
        .test_helpers
        .iter()
        .map(|h| (h.line, h.end_line))
        .collect();
    let mut helper_loops = vec![false; helpers.len()];
    let mut equality_exits = vec![0usize; helpers.len()];
    // Calls made in a loop of a test: the test and the name the callee ends in.
    let mut looped_sites: Vec<(usize, &str)> = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let called = callee(node, spec);
        if called.is_some() || spec.exits.contains(&node.kind()) {
            let line = node.start_position().row + 1;
            if let Some(at) = innermost(&helpers, line) {
                if !helper_loops[at] && in_loop(node, helpers[at].0, src, spec) {
                    helper_loops[at] = true;
                }
                if is_failure_exit(node, src, spec)
                    && guarded_by_equality(node, helpers[at].0, src, spec)
                {
                    equality_exits[at] += 1;
                }
            }
            if let (Some(called), Some(at)) = (called, innermost(&tests, line)) {
                if in_loop(node, tests[at].0, src, spec) {
                    looped_sites.push((at, last_leaf(called, src)));
                }
            }
        }
        let mut cursor = node.walk();
        stack.extend(node.children(&mut cursor));
    }
    let helper_checks_in_a_loop = |name: &str| {
        let mut named = (0..helpers.len())
            .filter(|at| helper_leaf(&facts.test_helpers[*at].name) == name)
            .peekable();
        named.peek().is_some() && named.all(|at| helper_loops[at])
    };
    // Of several helpers of one name, the one with the fewest such exits counts.
    let exits_of = |name: &str| {
        (0..helpers.len())
            .filter(|at| helper_leaf(&facts.test_helpers[*at].name) == name)
            .map(|at| equality_exits[at])
            .min()
            .unwrap_or(0)
    };
    let reach: Vec<(usize, usize)> = facts
        .tests
        .iter()
        .enumerate()
        .map(|(at, test)| {
            let calls = test.counted_helper_calls.iter();
            let checking = calls
                .zip(&test.helper_reach.counted)
                .filter(|(call, checks)| checks.0 > 0 && !call.contains('|'))
                .map(|(call, _)| helper_leaf(call));
            let (mut looped, mut exits) = (0, 0);
            for name in checking {
                if looped_sites.contains(&(at, name)) || helper_checks_in_a_loop(name) {
                    looped += 1;
                }
                exits += exits_of(name);
            }
            (looped, exits)
        })
        .collect();
    for (test, (looped, exits)) in facts.tests.iter_mut().zip(reach) {
        test.helper_reach.looped = looped;
        test.helper_reach.equality_exits = exits;
    }
}

#[cfg(all(test, feature = "lang-rust", feature = "lang-python"))]
mod tests {
    use crate::ast::{default_registry, AssertVocabulary};

    /// `HelperReach::looped` of the one test in `src`.
    fn looped(path: &str, src: &str) -> usize {
        let facts = default_registry()
            .find_pack(path)
            .unwrap()
            .extract(path, src, &AssertVocabulary::default())
            .unwrap();
        assert_eq!(facts.tests.len(), 1, "{:?}", facts.tests);
        facts.tests[0].helper_reach.looped
    }

    fn python(helper_body: &str, test_body: &str) -> usize {
        looped(
            "tests/test_t.py",
            &format!("def check(r):\n{helper_body}\ndef test_t():\n    rs = make()\n{test_body}"),
        )
    }

    const ASSERTS: &str = "    assert r.a == 1\n";

    #[test]
    fn a_helper_called_in_a_loop_of_the_test_is_a_looped_call() {
        assert_eq!(python(ASSERTS, "    for r in rs:\n        check(r)\n"), 1);
        assert_eq!(
            python(ASSERTS, "    while rs:\n        check(rs.pop())\n"),
            1
        );
        assert_eq!(python(ASSERTS, "    [check(r) for r in rs]\n"), 1);
        assert_eq!(python(ASSERTS, "    all(check(r) for r in rs)\n"), 1);
        // Controls: a straight-line call, a call in the loop's header, a call after
        // the loop, and a looped call to a helper that checks nothing.
        assert_eq!(python(ASSERTS, "    check(rs)\n"), 0);
        assert_eq!(
            python(ASSERTS, "    for r in check(rs):\n        pass\n"),
            0
        );
        assert_eq!(
            python(ASSERTS, "    for r in rs:\n        pass\n    check(rs)\n"),
            0
        );
        assert_eq!(
            python("    print(r)\n", "    for r in rs:\n        check(r)\n"),
            0
        );
    }

    #[test]
    fn a_helper_that_checks_in_a_loop_of_its_own_is_a_looped_call() {
        let call = "    check(rs)\n";
        assert_eq!(
            python("    for x in r:\n        assert x.a == 1\n", call),
            1
        );
        assert_eq!(
            python(
                "    for x in r:\n        if x.a != 1:\n            raise ValueError(x)\n",
                call
            ),
            1
        );
        // Controls: the check beside the loop, and a loop whose header alone calls.
        assert_eq!(
            python("    for x in r:\n        pass\n    assert r.a == 1\n", call),
            0
        );
        assert_eq!(
            python(
                "    for x in items(r):\n        pass\n    assert r.a == 1\n",
                call
            ),
            0
        );
    }

    #[test]
    fn a_closure_passed_to_an_iterator_method_is_a_loop() {
        let rust = |helper_body: &str, test_body: &str| {
            looped(
                "tests/t.rs",
                &format!("fn check(r: &R) {{\n{helper_body}}}\n\n#[test]\nfn t() {{\n    let rs = make();\n{test_body}}}\n"),
            )
        };
        let asserts = "    assert_eq!(r.a, 1);\n";
        assert_eq!(rust(asserts, "    rs.iter().for_each(|r| check(r));\n"), 1);
        assert_eq!(
            rust(asserts, "    for r in &rs {\n        check(r);\n    }\n"),
            1
        );
        assert_eq!(
            rust(
                "    for x in r.items() {\n        assert_eq!(x.a, 1);\n    }\n",
                "    check(&rs);\n"
            ),
            1
        );
        // Controls: a closure passed to a method that does not iterate, and a closure
        // that is the iterator's receiver rather than its argument.
        assert_eq!(
            rust(asserts, "    rs.first().map_or((), |r| check(r));\n"),
            0
        );
        assert_eq!(rust(asserts, "    check(&rs);\n"), 0);
    }

    /// Of two same-file helpers of one name, both must check in a loop: which one a call
    /// runs is not known.
    #[test]
    fn two_helpers_of_one_name_must_both_loop() {
        let file = |second: &str| {
            format!("struct A;\nstruct B;\nimpl A {{\n    fn check(r: &R) {{\n        for x in r.items() {{\n            assert_eq!(x.a, 1);\n        }}\n    }}\n}}\nimpl B {{\n    fn check(r: &R) {{\n{second}    }}\n}}\n\n#[test]\nfn t() {{\n    let rs = make();\n    A::check(&rs);\n}}\n")
        };
        let facts = |src: &str| {
            let f = default_registry()
                .find_pack("tests/t.rs")
                .unwrap()
                .extract("tests/t.rs", src, &AssertVocabulary::default())
                .unwrap();
            (f.tests[0].helper_checks, f.tests[0].helper_reach.looped)
        };
        let looping = "        for x in r.items() {\n            assert_eq!(x.b, 2);\n        }\n";
        let (checks, both) = facts(&file(looping));
        let (_, one) = facts(&file("        assert_eq!(r.b, 2);\n"));
        assert_eq!((checks, both, one), (1, 1, 0));
    }

    /// `HelperReach::equality_exits` of the one test in `src`.
    fn exits(path: &str, src: &str) -> usize {
        let facts = default_registry()
            .find_pack(path)
            .unwrap()
            .extract(path, src, &AssertVocabulary::default())
            .unwrap();
        assert_eq!(facts.tests.len(), 1, "{:?}", facts.tests);
        facts.tests[0].helper_reach.equality_exits
    }

    /// #595: a failure exit counts as an equality check written by hand only under an
    /// equality or inequality comparison, read from the condition's syntax node.
    #[test]
    fn a_failure_exit_guarded_by_an_equality_comparison_is_counted() {
        let rust = |helper_body: &str| {
            exits(
                "tests/t.rs",
                &format!("fn check(r: &R) {{\n{helper_body}}}\n\n#[test]\nfn t() {{\n    check(&make());\n}}\n"),
            )
        };
        assert_eq!(
            rust("    if r.a != 1 {\n        panic!(\"a\");\n    }\n"),
            1
        );
        assert_eq!(
            rust("    if !(r.a == 1) {\n        panic!(\"a\");\n    }\n"),
            1
        );
        assert_eq!(
            rust("    if r.a == 1 {\n        log(r);\n    } else {\n        unreachable!(\"a\");\n    }\n"),
            1
        );
        assert_eq!(
            rust("    if !r.name.eq(\"a\") {\n        panic!(\"a\");\n    }\n"),
            1
        );
        assert_eq!(
            rust("    if r.a != 1 {\n        panic!(\"a\");\n    }\n    if r.b != 2 {\n        panic!(\"b\");\n    }\n"),
            2
        );
        // Controls: an ordering comparison, a truthiness test, a call, no condition, an
        // equality that is only part of the condition, an equality in an outer
        // conditional, the text of one in a string, and an exit inside the condition.
        for body in [
            "    if r.a < 1 {\n        panic!(\"a\");\n    }\n",
            "    if r.bad {\n        panic!(\"a\");\n    }\n",
            "    if !r.ok() {\n        panic!(\"a\");\n    }\n",
            "    panic!(\"a\");\n",
            "    if r.strict && r.a != 1 {\n        panic!(\"a\");\n    }\n",
            "    if r.a != 1 {\n        if r.bad {\n            panic!(\"a\");\n        }\n    }\n",
            "    if r.name.contains(\"a != b\") {\n        panic!(\"a\");\n    }\n",
            "    if r.a != 1 {\n        log(r);\n    }\n",
        ] {
            assert_eq!(rust(body), 0, "{body}");
        }

        // Of two helpers of one name, the one with the fewest such exits counts.
        let two = |second: &str| {
            exits(
                "tests/t.rs",
                &format!("struct A;\nstruct B;\nimpl A {{\n    fn check(r: &R) {{\n        if r.a != 1 {{\n            panic!(\"a\");\n        }}\n    }}\n}}\nimpl B {{\n    fn check(r: &R) {{\n{second}    }}\n}}\n\n#[test]\nfn t() {{\n    A::check(&make());\n}}\n"),
            )
        };
        assert_eq!(
            two("        if r.b != 2 {\n            panic!(\"b\");\n        }\n"),
            1
        );
        assert_eq!(
            two("        if r.bad {\n            panic!(\"b\");\n        }\n"),
            0
        );

        let python = |helper_body: &str| {
            exits(
                "tests/test_t.py",
                &format!("def check(r):\n{helper_body}\ndef test_t():\n    check(make())\n"),
            )
        };
        assert_eq!(python("    if r.a != 1:\n        raise ValueError(r)\n"), 1);
        assert_eq!(
            python("    if not r.a == 1:\n        raise ValueError(r)\n"),
            1
        );
        assert_eq!(python("    if r.a < 1:\n        raise ValueError(r)\n"), 0);
        assert_eq!(python("    if not r.a:\n        raise ValueError(r)\n"), 0);
        assert_eq!(
            python("    if r.a is None:\n        raise ValueError(r)\n"),
            0
        );
        // An `assert` statement is an assertion, counted as one by the pack.
        assert_eq!(python("    if r.a != 1:\n        assert r.b\n"), 0);
    }
}
