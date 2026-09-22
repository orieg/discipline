//! Testing-effort budgets read from the syntax tree: proptest `cases`, quickcheck
//! `tests` / `gen_size`, Hypothesis `max_examples` / `deadline`, fast-check `numRuns`.
//!
//! A budget is a named integer literal in a configuration position: a struct field
//! initialiser, a builder-method argument, a keyword argument or an object pair. The
//! same word in a string, a comment or an unrelated assignment (`min_tests = 40`) is not
//! one, which is what the line patterns this replaces could not tell apart.

use tree_sitter::Node;

/// One budget as the tree shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetSite {
    /// What the number is a budget of (`proptest cases`).
    pub subject: &'static str,
    pub value: u64,
    pub line: usize,
}

/// A key-value node shape: the node kind, the field holding the key, the field holding
/// the value.
pub struct KeyValueShape {
    pub kind: &'static str,
    pub key_field: &'static str,
    pub value_field: &'static str,
}

pub struct BudgetSpec {
    /// Key-value shapes (`field_initializer`, `keyword_argument`, `pair`).
    pub key_values: &'static [KeyValueShape],
    /// Names in a key-value position and their subjects.
    pub keys: &'static [(&'static str, &'static str)],
    /// Call node kind, the field holding the callee, and the field holding the arguments.
    pub call_kind: &'static str,
    pub callee_field: &'static str,
    pub arguments_field: &'static str,
    /// Method names whose first integer argument is a budget, and their subjects.
    pub methods: &'static [(&'static str, &'static str)],
    /// Node kinds of an integer literal.
    pub integer_kinds: &'static [&'static str],
    /// Node kinds of a macro's token tree, where `key : integer` triples are read as
    /// key-value pairs (`proptest! { #![proptest_config(ProptestConfig { cases: 1000, .. })] }`).
    pub token_tree_kinds: &'static [&'static str],
}

fn text<'a>(node: Node, src: &'a str) -> &'a str {
    node.utf8_text(src.as_bytes()).unwrap_or("")
}

fn integer(node: Node, src: &str, spec: &BudgetSpec) -> Option<u64> {
    if !spec.integer_kinds.contains(&node.kind()) {
        return None;
    }
    let t: String = text(node, src)
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect();
    t.parse().ok()
}

/// Budgets in `root`, in source order.
pub fn extract(root: Node, src: &str, spec: &BudgetSpec) -> Vec<BudgetSite> {
    let mut out: Vec<(usize, BudgetSite)> = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let line = node.start_position().row + 1;
        if spec.token_tree_kinds.contains(&node.kind()) {
            let mut cursor = node.walk();
            let tokens: Vec<Node> = node.children(&mut cursor).collect();
            for w in tokens.windows(3) {
                let key = text(w[0], src);
                if w[0].kind() == "identifier" && w[1].kind() == ":" {
                    if let Some((_, subject)) = spec.keys.iter().find(|(name, _)| *name == key) {
                        if let Some(value) = integer(w[2], src, spec) {
                            out.push((
                                w[0].start_byte(),
                                BudgetSite {
                                    subject,
                                    value,
                                    line: w[0].start_position().row + 1,
                                },
                            ));
                        }
                    }
                }
            }
        }
        if let Some(shape) = spec.key_values.iter().find(|s| s.kind == node.kind()) {
            if let (Some(k), Some(v)) = (
                node.child_by_field_name(shape.key_field),
                node.child_by_field_name(shape.value_field),
            ) {
                let key = text(k, src).trim().trim_matches(['"', '\'']);
                if let Some((_, subject)) = spec.keys.iter().find(|(name, _)| *name == key) {
                    if let Some(value) = integer(v, src, spec) {
                        out.push((
                            node.start_byte(),
                            BudgetSite {
                                subject,
                                value,
                                line,
                            },
                        ));
                    }
                }
            }
        } else if node.kind() == spec.call_kind {
            let callee = node
                .child_by_field_name(spec.callee_field)
                .map(|c| text(c, src))
                .unwrap_or("");
            let method = callee.rsplit(['.', ':']).next().unwrap_or(callee).trim();
            if let Some((_, subject)) = spec.methods.iter().find(|(name, _)| *name == method) {
                let first = node
                    .child_by_field_name(spec.arguments_field)
                    .and_then(|a| {
                        let mut cursor = a.walk();
                        let found = a.named_children(&mut cursor).next();
                        found
                    });
                if let Some(value) = first.and_then(|a| integer(a, src, spec)) {
                    // Keyed on the argument so `.tests(300).gen_size(40)` reads in
                    // source order (the outer call starts first).
                    out.push((
                        first.map(|a| a.start_byte()).unwrap_or(node.start_byte()),
                        BudgetSite {
                            subject,
                            value,
                            line,
                        },
                    ));
                }
            }
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
    out.sort_by_key(|(at, _)| *at);
    out.into_iter().map(|(_, site)| site).collect()
}

#[cfg(all(
    test,
    feature = "lang-rust",
    feature = "lang-python",
    feature = "lang-javascript"
))]
mod pack_tests {
    use crate::ast::{default_registry, AssertVocabulary, Fact};

    fn budgets(path: &str, src: &str) -> Vec<(&'static str, u64, usize)> {
        let reg = default_registry();
        let pack = reg.find_pack(path).unwrap();
        assert!(pack.supplies(Fact::Budgets));
        pack.extract(path, src, &AssertVocabulary::default())
            .unwrap()
            .budgets
            .into_iter()
            .map(|b| (b.subject, b.value, b.line))
            .collect()
    }

    #[test]
    fn rust_proptest_and_quickcheck_budgets_come_from_config_positions_only() {
        let got = budgets(
            "tests/prop.rs",
            "// cases: 9999 in a comment is not a budget\nproptest! {\n    #![proptest_config(ProptestConfig { cases: 1000, max_shrink_iters: 500, .. ProptestConfig::default() })]\n    fn p(x in any::<u32>()) { prop_assert!(x >= 0); }\n}\nfn q() {\n    let c = ProptestConfig::with_cases(250);\n    let s = \"tests = 77\";\n    QuickCheck::new().tests(300).gen_size(40).quickcheck(f as fn(u32) -> bool);\n    let min_tests = 40;\n}\n",
        );
        assert_eq!(
            got,
            vec![
                ("proptest cases", 1000, 3),
                ("proptest max_shrink_iters", 500, 3),
                ("proptest cases", 250, 7),
                ("quickcheck tests", 300, 9),
                ("quickcheck gen_size", 40, 9),
            ]
        );
    }

    #[test]
    fn python_hypothesis_and_javascript_fast_check_budgets() {
        let py = budgets(
            "tests/test_prop.py",
            "from hypothesis import given, settings\n\n@settings(max_examples=500, deadline=2000)\n@given(st.integers())\ndef test_p(x):\n    # max_examples=1 here is a comment\n    assert x == x\n\nnote = 'max_examples=3'\n",
        );
        assert_eq!(
            py,
            vec![
                ("hypothesis max_examples", 500, 3),
                ("hypothesis deadline", 2000, 3)
            ]
        );
        let js = budgets(
            "src/a.test.ts",
            "test('p', () => {\n  fc.assert(fc.property(fc.integer(), (x) => x === x), { numRuns: 1000 });\n  const s = 'numRuns: 5';\n  // numRuns: 6\n});\n",
        );
        assert_eq!(js, vec![("fast-check numRuns", 1000, 2)]);
    }
}
