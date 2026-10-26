//! Production code that asks whether a test runner is loaded.
//!
//! A function can return what its tests expect only while a test runner is loaded: no
//! test is edited and no assertion drops, so no test-facing gate sees it (#824). The
//! `stub-bodies` gate reports the reads listed here that a change adds outside test code.
//! Each read is an exact shape of the tree, from a closed list of signals whose only
//! purpose is to tell whether a runner is present:
//!
//! - Python: `"pytest" in sys.modules` and `"unittest" in sys.modules` (also
//!   `sys.modules.keys()` and `sys.modules.get("pytest")`); `PYTEST_CURRENT_TEST` or
//!   `PYTEST_XDIST_WORKER` read through `os.environ.get(..)`, `os.environ[..]`,
//!   `.. in os.environ` or `os.getenv(..)`.
//! - JavaScript and TypeScript: `process.env.JEST_WORKER_ID`, `process.env.VITEST`,
//!   `process.env.VITEST_WORKER_ID` (also in brackets), and `import.meta.vitest`.
//! - Rust: `cfg!(test)` and `cfg!(not(test))`.
//! - Go: `testing.Testing()`, `flag.Lookup("test.v")` and
//!   `strings.HasSuffix(os.Args[0], ".test")`, each package under the name the file
//!   imports it as.
//!
//! A read counts where its value decides something: a condition (`if`, `elif`, `while`,
//! a ternary, a `match` guard, a `case`), an operand of `and` / `or`, or a value assigned
//! or returned; parentheses, a negation and a comparison pass the value through. A read
//! inside test code is not one: a test, a test file (left out by the pack), or a Rust
//! item compiled in test builds only (`#[cfg(test)]`, `#![cfg(test)]`).
//!
//! Nothing here follows a value: a probe made in a helper and called, a flag the runner's
//! configuration sets, a module imported under another name, or an environment variable
//! a project's own harness sets is not read.

use tree_sitter::Node;

/// The languages whose probes are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Python,
    JavaScript,
    Rust,
    Go,
}

/// What a probe reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Signal {
    /// `sys.modules` holds the named runner module.
    PythonModule(&'static str),
    /// The named environment variable a test runner sets.
    RunnerVariable(&'static str),
    /// `import.meta.vitest`, which Vitest defines for in-source tests.
    ImportMetaVitest,
    /// `cfg!(test)`.
    RustCfgTest,
    /// Go's `testing.Testing()`.
    GoTesting,
    /// Go's `flag.Lookup("test.v")`.
    GoTestFlag,
    /// Go's `strings.HasSuffix(os.Args[0], ".test")`.
    GoTestBinaryName,
}

impl Signal {
    /// What the probe reads, as a noun phrase for a finding's message.
    pub fn label(self) -> String {
        match self {
            Signal::PythonModule(m) => format!("a check of whether `{m}` is loaded"),
            Signal::RunnerVariable(v) => {
                format!("a read of the test runner's environment variable `{v}`")
            }
            Signal::ImportMetaVitest => "a check of `import.meta.vitest`".to_string(),
            Signal::RustCfgTest => "`cfg!(test)` outside test-only code".to_string(),
            Signal::GoTesting => "a call of `testing.Testing()`".to_string(),
            Signal::GoTestFlag => "a lookup of the `test.v` flag".to_string(),
            Signal::GoTestBinaryName => "a check that the program name ends in `.test`".to_string(),
        }
    }
}

/// One probe read outside test code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// 1-based line.
    pub line: usize,
    pub signal: Signal,
    /// The function the probe is in; `<module>` at the top level.
    pub subject: String,
}

const PYTHON_MODULES: &[&str] = &["pytest", "unittest"];
const PYTHON_VARIABLES: &[&str] = &["PYTEST_CURRENT_TEST", "PYTEST_XDIST_WORKER"];
const JS_VARIABLES: &[&str] = &["JEST_WORKER_ID", "VITEST", "VITEST_WORKER_ID"];

/// The probes of the tree under `root`, outside the lines `is_test_line` says are test
/// code.
pub fn read(lang: Lang, root: Node, src: &str, is_test_line: &dyn Fn(usize) -> bool) -> Vec<Probe> {
    let imports = match lang {
        Lang::Go => go_imports(root, src),
        _ => GoImports::default(),
    };
    let mut out = Vec::new();
    // The ancestors of the node being read, root first.
    let mut chain: Vec<Node> = Vec::new();
    let mut stack = vec![(root, 0usize)];
    while let Some((node, depth)) = stack.pop() {
        chain.truncate(depth);
        if let Some(signal) = signal(lang, node, src, &imports) {
            let line = node.start_position().row + 1;
            if !is_test_line(line)
                && decides(lang, node, &chain, src)
                && !(lang == Lang::Rust && rust_test_only(&chain, src))
            {
                out.push(Probe {
                    line,
                    signal,
                    subject: subject(lang, &chain, src),
                });
            }
            continue;
        }
        chain.push(node);
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push((child, depth + 1));
        }
    }
    out
}

/// The probes of `head` the change adds: each signal in each function, counted, less
/// what the base side held. A probe that moved within its function is not new.
pub fn new_probes(base: &[Probe], head: &[Probe]) -> Vec<Probe> {
    let mut held: std::collections::HashMap<(Signal, &str), usize> =
        std::collections::HashMap::new();
    for p in base {
        *held.entry((p.signal, p.subject.as_str())).or_default() += 1;
    }
    let mut out = Vec::new();
    for p in head {
        match held.get_mut(&(p.signal, p.subject.as_str())) {
            Some(n) if *n > 0 => *n -= 1,
            _ => out.push(p.clone()),
        }
    }
    out
}

fn text<'a>(node: Node, src: &'a str) -> &'a str {
    node.utf8_text(src.as_bytes()).unwrap_or("")
}

fn named_children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.named_children(&mut cursor).collect();
    children
}

/// `a.b.c` for an attribute or member chain of plain names; `None` for anything else.
fn dotted(node: Node, src: &str) -> Option<String> {
    match node.kind() {
        "identifier" => Some(text(node, src).to_string()),
        "attribute" => Some(format!(
            "{}.{}",
            dotted(node.child_by_field_name("object")?, src)?,
            text(node.child_by_field_name("attribute")?, src)
        )),
        "member_expression" => Some(format!(
            "{}.{}",
            dotted(node.child_by_field_name("object")?, src)?,
            text(node.child_by_field_name("property")?, src)
        )),
        "selector_expression" => Some(format!(
            "{}.{}",
            dotted(node.child_by_field_name("operand")?, src)?,
            text(node.child_by_field_name("field")?, src)
        )),
        _ => None,
    }
}

/// The value of a plain string literal with no prefix, escape or interpolation.
fn string_value<'a>(node: Node, src: &'a str) -> Option<&'a str> {
    let parts = named_children(node);
    match node.kind() {
        // Python: `string_start`, `string_content`, `string_end`.
        "string" if parts.len() == 3 && parts[1].kind() == "string_content" => {
            let start = text(parts[0], src);
            matches!(start, "\"" | "'").then(|| text(parts[1], src))
        }
        // JavaScript: one `string_fragment`.
        "string" if parts.len() == 1 && parts[0].kind() == "string_fragment" => {
            Some(text(parts[0], src))
        }
        "interpreted_string_literal"
            if parts.len() == 1 && parts[0].kind() == "interpreted_string_literal_content" =>
        {
            Some(text(parts[0], src))
        }
        "raw_string_literal" => text(node, src)
            .strip_prefix('`')
            .and_then(|t| t.strip_suffix('`')),
        _ => None,
    }
}

/// The one listed name `node` is a string literal of.
fn listed(node: Node, src: &str, names: &[&'static str]) -> Option<&'static str> {
    let value = string_value(node, src)?;
    names.iter().copied().find(|n| *n == value)
}

/// The call's arguments, without punctuation or comments.
fn arguments(call: Node) -> Vec<Node> {
    call.child_by_field_name("arguments")
        .map(|args| {
            named_children(args)
                .into_iter()
                .filter(|a| !a.kind().contains("comment"))
                .collect()
        })
        .unwrap_or_default()
}

/// `x` or `x.keys()`, where `x` is the chain `name`.
fn is_mapping(node: Node, src: &str, name: &str) -> bool {
    if dotted(node, src).as_deref() == Some(name) {
        return true;
    }
    node.kind() == "call"
        && arguments(node).is_empty()
        && node
            .child_by_field_name("function")
            .and_then(|f| dotted(f, src))
            .is_some_and(|f| f.strip_suffix(".keys") == Some(name))
}

fn signal(lang: Lang, node: Node, src: &str, imports: &GoImports) -> Option<Signal> {
    match lang {
        Lang::Python => python_signal(node, src),
        Lang::JavaScript => js_signal(node, src),
        Lang::Rust => rust_signal(node, src),
        Lang::Go => go_signal(node, src, imports),
    }
}

fn python_signal(node: Node, src: &str) -> Option<Signal> {
    match node.kind() {
        // `"pytest" in sys.modules`, `"PYTEST_CURRENT_TEST" not in os.environ`.
        "comparison_operator" => {
            let operands = named_children(node);
            let [left, right] = operands.as_slice() else {
                return None;
            };
            let mut cursor = node.walk();
            let operator: Vec<&str> = node
                .children(&mut cursor)
                .filter(|c| !c.is_named())
                .map(|c| text(c, src))
                .collect();
            if !matches!(operator.join(" ").as_str(), "in" | "not in") {
                return None;
            }
            if let Some(m) = listed(*left, src, PYTHON_MODULES) {
                return is_mapping(*right, src, "sys.modules").then_some(Signal::PythonModule(m));
            }
            let v = listed(*left, src, PYTHON_VARIABLES)?;
            is_mapping(*right, src, "os.environ").then_some(Signal::RunnerVariable(v))
        }
        // `sys.modules.get("pytest")`, `os.environ.get(..)`, `os.getenv(..)`.
        "call" => {
            let function = dotted(node.child_by_field_name("function")?, src)?;
            let first = *arguments(node).first()?;
            match function.as_str() {
                "sys.modules.get" => listed(first, src, PYTHON_MODULES).map(Signal::PythonModule),
                "os.environ.get" | "os.getenv" => {
                    listed(first, src, PYTHON_VARIABLES).map(Signal::RunnerVariable)
                }
                _ => None,
            }
        }
        // `os.environ["PYTEST_CURRENT_TEST"]`.
        "subscript" => {
            let value = node.child_by_field_name("value")?;
            let key = node.child_by_field_name("subscript")?;
            (dotted(value, src).as_deref() == Some("os.environ"))
                .then(|| listed(key, src, PYTHON_VARIABLES))
                .flatten()
                .map(Signal::RunnerVariable)
        }
        _ => None,
    }
}

fn js_signal(node: Node, src: &str) -> Option<Signal> {
    match node.kind() {
        "member_expression" => {
            let object = node.child_by_field_name("object")?;
            let property = text(node.child_by_field_name("property")?, src);
            if object.kind() == "meta_property" && text(object, src) == "import.meta" {
                return (property == "vitest").then_some(Signal::ImportMetaVitest);
            }
            (dotted(object, src).as_deref() == Some("process.env"))
                .then(|| JS_VARIABLES.iter().copied().find(|v| *v == property))
                .flatten()
                .map(Signal::RunnerVariable)
        }
        "subscript_expression" => {
            let object = node.child_by_field_name("object")?;
            let index = node.child_by_field_name("index")?;
            (dotted(object, src).as_deref() == Some("process.env"))
                .then(|| listed(index, src, JS_VARIABLES))
                .flatten()
                .map(Signal::RunnerVariable)
        }
        _ => None,
    }
}

fn rust_signal(node: Node, src: &str) -> Option<Signal> {
    if node.kind() != "macro_invocation"
        || node
            .child_by_field_name("macro")
            .is_none_or(|m| m.kind() != "identifier" || text(m, src) != "cfg")
    {
        return None;
    }
    let tree = named_children(node)
        .into_iter()
        .find(|c| c.kind() == "token_tree")?;
    let is_test = |nodes: &[Node]| {
        nodes.len() == 1 && nodes[0].kind() == "identifier" && text(nodes[0], src) == "test"
    };
    let inner = named_children(tree);
    // `cfg!(test)`, or `cfg!(not(test))`.
    let negated = matches!(inner.as_slice(), [not, args]
        if not.kind() == "identifier" && text(*not, src) == "not"
            && args.kind() == "token_tree" && is_test(&named_children(*args)));
    (is_test(&inner) || negated).then_some(Signal::RustCfgTest)
}

/// The names a Go file imports the packages a probe calls under.
#[derive(Debug, Default)]
struct GoImports {
    testing: Option<String>,
    flag: Option<String>,
    strings: Option<String>,
    os: Option<String>,
}

fn go_imports(root: Node, src: &str) -> GoImports {
    let mut out = GoImports::default();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "source_file" | "import_declaration" | "import_spec_list" => {
                stack.extend(named_children(node));
            }
            "import_spec" => {
                let Some(path) = node
                    .child_by_field_name("path")
                    .and_then(|p| string_value(p, src))
                else {
                    continue;
                };
                // A dot or blank import gives no name to call through.
                let name = match node.child_by_field_name("name") {
                    Some(n) if n.kind() == "package_identifier" => Some(text(n, src).to_string()),
                    Some(_) => None,
                    None => Some(path.to_string()),
                };
                let slot = match path {
                    "testing" => &mut out.testing,
                    "flag" => &mut out.flag,
                    "strings" => &mut out.strings,
                    "os" => &mut out.os,
                    _ => continue,
                };
                *slot = name;
            }
            _ => {}
        }
    }
    out
}

fn go_signal(node: Node, src: &str, imports: &GoImports) -> Option<Signal> {
    if node.kind() != "call_expression" {
        return None;
    }
    let function = node.child_by_field_name("function")?;
    if function.kind() != "selector_expression" {
        return None;
    }
    let package = function.child_by_field_name("operand")?;
    if package.kind() != "identifier" {
        return None;
    }
    let package = Some(text(package, src));
    let field = text(function.child_by_field_name("field")?, src);
    let args = arguments(node);
    if package == imports.testing.as_deref() && field == "Testing" && args.is_empty() {
        return Some(Signal::GoTesting);
    }
    if package == imports.flag.as_deref()
        && field == "Lookup"
        && args.len() == 1
        && string_value(args[0], src) == Some("test.v")
    {
        return Some(Signal::GoTestFlag);
    }
    if package == imports.strings.as_deref()
        && field == "HasSuffix"
        && args.len() == 2
        && string_value(args[1], src) == Some(".test")
    {
        let program = args[0];
        let os_args = imports.os.as_deref().map(|os| format!("{os}.Args"));
        let first = program.kind() == "index_expression"
            && program
                .child_by_field_name("operand")
                .and_then(|o| dotted(o, src))
                .is_some_and(|o| Some(o) == os_args)
            && program
                .child_by_field_name("index")
                .is_some_and(|i| i.kind() == "int_literal" && text(i, src) == "0");
        return first.then_some(Signal::GoTestBinaryName);
    }
    None
}

/// What a node climbing from a probe toward the root does with the probe's value.
enum Step {
    /// Passes it through: parentheses, a negation, a comparison.
    Through,
    /// Decides on it: a condition, an operand of `and` / `or`, an assigned or returned
    /// value.
    Decides,
    /// Anything else: an argument, a string, an arithmetic operand.
    Neither,
}

/// The operator token of a binary or unary node.
fn operator<'a>(node: Node, src: &'a str) -> Option<&'a str> {
    node.child_by_field_name("operator")
        .or_else(|| {
            let mut cursor = node.walk();
            let found = node.children(&mut cursor).find(|c| !c.is_named());
            found
        })
        .map(|o| text(o, src))
}

/// Whether the probe's value decides something where it stands.
fn decides(lang: Lang, probe: Node, chain: &[Node], src: &str) -> bool {
    let mut child = probe;
    for &parent in chain.iter().rev() {
        let field = |name: &str| parent.child_by_field_name(name) == Some(child);
        let step = match (lang, parent.kind()) {
            (_, "parenthesized_expression") | (Lang::Go, "expression_list") => Step::Through,
            (Lang::Python, "not_operator" | "comparison_operator") => Step::Through,
            (Lang::Python, "boolean_operator") => Step::Decides,
            (_, "binary_expression") => match operator(parent, src) {
                Some("&&" | "||") => Step::Decides,
                Some("==" | "!=" | "===" | "!==") => Step::Through,
                _ => Step::Neither,
            },
            (_, "unary_expression") => match operator(parent, src) {
                Some("!") => Step::Through,
                _ => Step::Neither,
            },
            (Lang::Python, "if_statement" | "elif_clause" | "while_statement") => {
                decided(field("condition"))
            }
            (Lang::Python, "if_clause") => Step::Decides,
            (Lang::Python, "conditional_expression") => {
                decided(parent.named_child(1) == Some(child))
            }
            (Lang::Python, "assignment" | "augmented_assignment") => decided(field("right")),
            (Lang::Python, "named_expression") => decided(field("value")),
            (Lang::Python, "lambda") => decided(field("body")),
            (Lang::JavaScript, "if_statement" | "while_statement" | "do_statement")
            | (Lang::JavaScript, "for_statement" | "ternary_expression") => {
                decided(field("condition"))
            }
            (Lang::JavaScript, "variable_declarator") => decided(field("value")),
            (Lang::JavaScript, "assignment_expression" | "augmented_assignment_expression") => {
                decided(field("right"))
            }
            (Lang::JavaScript, "arrow_function") => decided(field("body")),
            (Lang::Rust, "if_expression" | "while_expression" | "match_pattern") => {
                decided(field("condition"))
            }
            (Lang::Rust, "let_declaration" | "const_item" | "static_item") => {
                decided(field("value"))
            }
            (Lang::Rust, "assignment_expression" | "compound_assignment_expr") => {
                decided(field("right"))
            }
            (Lang::Rust, "closure_expression") => decided(field("body")),
            // The value of a function's body: its last expression.
            (Lang::Rust, "block") => decided(
                parent.named_child(parent.named_child_count().saturating_sub(1)) == Some(child),
            ),
            (Lang::Go, "if_statement") => decided(field("condition")),
            (Lang::Go, "for_statement") => decided(!field("body")),
            (Lang::Go, "expression_case") => decided(field("value")),
            (Lang::Go, "short_var_declaration" | "assignment_statement") => decided(field("right")),
            (Lang::Go, "var_spec" | "const_spec") => decided(field("value")),
            (_, "return_statement" | "return_expression") => Step::Decides,
            _ => Step::Neither,
        };
        match step {
            Step::Through => child = parent,
            Step::Decides => return true,
            Step::Neither => return false,
        }
    }
    false
}

fn decided(yes: bool) -> Step {
    if yes {
        Step::Decides
    } else {
        Step::Neither
    }
}

/// Whether one of `chain` is a Rust item compiled in test builds only: an item under an
/// outer `#[cfg(test)]`, or a file or module whose inner `#![cfg(test)]` says so.
fn rust_test_only(chain: &[Node], src: &str) -> bool {
    let only_in_tests =
        |a: Node| crate::ast::runner_collection::rust_cfg_only_in_tests(a, src.as_bytes());
    for (i, &node) in chain.iter().enumerate() {
        let children = named_children(node);
        if matches!(node.kind(), "source_file" | "declaration_list")
            && children
                .iter()
                .any(|c| c.kind() == "inner_attribute_item" && only_in_tests(*c))
        {
            return true;
        }
        // The attributes right above the next node of the chain.
        let Some(&item) = chain.get(i + 1) else {
            continue;
        };
        let Some(at) = children.iter().position(|c| c.id() == item.id()) else {
            continue;
        };
        if children[..at]
            .iter()
            .rev()
            .filter(|c| !c.kind().ends_with("comment"))
            .take_while(|c| c.kind() == "attribute_item")
            .any(|a| only_in_tests(*a))
        {
            return true;
        }
    }
    false
}

/// The name of the innermost function of `chain`; `<module>` when there is none.
fn subject(lang: Lang, chain: &[Node], src: &str) -> String {
    for (i, &node) in chain.iter().enumerate().rev() {
        let named = match (lang, node.kind()) {
            (Lang::Python, "function_definition")
            | (Lang::Rust, "function_item")
            | (Lang::Go, "function_declaration" | "method_declaration")
            | (Lang::JavaScript, "function_declaration" | "method_definition") => {
                node.child_by_field_name("name")
            }
            (Lang::JavaScript, "function_expression" | "arrow_function") => {
                node.child_by_field_name("name").or_else(|| {
                    i.checked_sub(1)
                        .map(|p| chain[p])
                        .filter(|p| p.kind() == "variable_declarator")
                        .and_then(|p| p.child_by_field_name("name"))
                })
            }
            _ => continue,
        };
        return named.map_or_else(|| "<anonymous>".to_string(), |n| text(n, src).to_string());
    }
    "<module>".to_string()
}

#[cfg(all(
    test,
    feature = "lang-python",
    feature = "lang-javascript",
    feature = "lang-rust",
    feature = "lang-go"
))]
mod tests {
    use super::*;
    use crate::ast::{AssertVocabulary, LanguagePack};

    fn probes(pack: &dyn LanguagePack, path: &str, src: &str) -> Vec<(usize, Signal, String)> {
        pack.extract(path, src, &AssertVocabulary::default())
            .unwrap()
            .runner_probes
            .into_iter()
            .map(|p| (p.line, p.signal, p.subject))
            .collect()
    }

    fn signals(pack: &dyn LanguagePack, path: &str, src: &str) -> Vec<Signal> {
        probes(pack, path, src).into_iter().map(|p| p.1).collect()
    }

    const PY: &dyn LanguagePack = &crate::ast::python::PythonPack;
    const JS: &dyn LanguagePack = &crate::ast::javascript::JavaScriptPack;
    const RS: &dyn LanguagePack = &crate::ast::rust::RustPack;
    const GO: &dyn LanguagePack = &crate::ast::r#go::GoPack;

    #[test]
    fn python_probes_that_decide_are_read() {
        let pytest = Signal::PythonModule("pytest");
        let current = Signal::RunnerVariable("PYTEST_CURRENT_TEST");
        let cases: &[(&str, Signal)] = &[
            ("    if 'pytest' in sys.modules:\n        return 4\n", pytest),
            ("    if \"unittest\" in sys.modules.keys():\n        return 4\n", Signal::PythonModule("unittest")),
            ("    return sys.modules.get('pytest') is not None\n", pytest),
            ("    if os.environ.get('PYTEST_CURRENT_TEST'):\n        return 4\n", current),
            ("    t = os.environ['PYTEST_XDIST_WORKER']\n", Signal::RunnerVariable("PYTEST_XDIST_WORKER")),
            ("    while 'PYTEST_CURRENT_TEST' not in os.environ:\n        pass\n", current),
            ("    return 4 if os.getenv('PYTEST_CURRENT_TEST') else 3\n", current),
            ("    if x:\n        pass\n    elif not os.getenv('PYTEST_CURRENT_TEST') and x:\n        pass\n", current),
        ];
        for (body, want) in cases {
            let src = format!("import os, sys\n\ndef f(x):\n{body}");
            let got = probes(PY, "src/calc.py", &src);
            assert_eq!(got.len(), 1, "{body}: {got:?}");
            assert_eq!(got[0].1, *want, "{body}");
            assert_eq!(got[0].2, "f", "{body}");
        }
    }

    #[test]
    fn python_reads_that_decide_nothing_or_name_no_runner_are_not_probes() {
        for body in [
            "    print(os.environ.get('PYTEST_CURRENT_TEST'))\n",
            "    if 'pytest' in deps:\n        pass\n",
            "    if os.environ.get('HOME'):\n        pass\n",
            "    if 'requests' in sys.modules:\n        pass\n",
            "    if environ.get('PYTEST_CURRENT_TEST'):\n        pass\n",
        ] {
            let src = format!("import os, sys\n\ndef f(x):\n{body}");
            assert!(signals(PY, "src/calc.py", &src).is_empty(), "{body}");
        }
        let probe = "import sys\n\ndef f():\n    if 'pytest' in sys.modules:\n        return 4\n";
        assert_eq!(
            signals(PY, "src/calc.py", probe).len(),
            1,
            "positive control"
        );
        for test_path in ["tests/test_calc.py", "conftest.py", "src/calc_test.py"] {
            assert!(signals(PY, test_path, probe).is_empty(), "{test_path}");
        }
    }

    #[test]
    fn js_probes_that_decide_are_read_and_node_env_is_not() {
        let cases: &[(&str, Signal)] = &[
            (
                "  if (process.env.JEST_WORKER_ID) return 4;\n",
                Signal::RunnerVariable("JEST_WORKER_ID"),
            ),
            (
                "  const t = !!process.env[\"VITEST\"];\n",
                Signal::RunnerVariable("VITEST"),
            ),
            (
                "  return process.env.VITEST_WORKER_ID !== undefined;\n",
                Signal::RunnerVariable("VITEST_WORKER_ID"),
            ),
            (
                "  if (import.meta.vitest) { return 4; }\n",
                Signal::ImportMetaVitest,
            ),
            (
                "  return x && process.env.JEST_WORKER_ID ? 4 : x;\n",
                Signal::RunnerVariable("JEST_WORKER_ID"),
            ),
        ];
        for (body, want) in cases {
            let src = format!("export function f(x) {{\n{body}  return x * 3;\n}}\n");
            let got = probes(JS, "src/calc.js", &src);
            assert_eq!(got.len(), 1, "{body}: {got:?}");
            assert_eq!(got[0].1, *want, "{body}");
            assert_eq!(got[0].2, "f", "{body}");
        }
        for body in [
            "  if (process.env.NODE_ENV === 'test') return 4;\n",
            "  console.log(process.env.JEST_WORKER_ID);\n",
            "  if (env.JEST_WORKER_ID) return 4;\n",
        ] {
            let src = format!("export function f(x) {{\n{body}  return x * 3;\n}}\n");
            assert!(signals(JS, "src/calc.js", &src).is_empty(), "{body}");
        }
        let probe = "const f = (x) => process.env.VITEST ? 4 : x;\n";
        assert_eq!(
            probes(JS, "src/calc.ts", probe),
            vec![(1, Signal::RunnerVariable("VITEST"), "f".to_string())]
        );
        assert!(signals(JS, "src/calc.test.ts", probe).is_empty());
    }

    #[test]
    fn rust_cfg_test_outside_test_only_items_is_read() {
        let cases = [
            "pub fn f(x: i32) -> i32 {\n    if cfg!(test) {\n        return 4;\n    }\n    x * 3\n}\n",
            "pub fn f() -> bool {\n    cfg!(test)\n}\n",
            "pub fn f(x: i32) -> i32 {\n    match x {\n        _ if cfg!(not(test)) => x,\n        _ => 4,\n    }\n}\n",
            "pub fn f() {\n    let t = !cfg!(test);\n}\n",
        ];
        for src in cases {
            assert_eq!(
                probes(RS, "src/lib.rs", src),
                vec![(
                    2 + usize::from(src.contains("match")),
                    Signal::RustCfgTest,
                    "f".to_string()
                )],
                "{src}"
            );
        }
        let at_module = "const T: bool = cfg!(test);\n";
        assert_eq!(
            probes(RS, "src/lib.rs", at_module),
            vec![(1, Signal::RustCfgTest, "<module>".to_string())]
        );
    }

    #[test]
    fn rust_cfg_test_in_test_only_code_or_deciding_nothing_is_not_read() {
        for src in [
            "#[cfg(test)]\nmod tests {\n    fn helper() -> bool {\n        cfg!(test)\n    }\n}\n",
            "#[cfg(all(test, unix))]\nfn helper() -> bool {\n    cfg!(test)\n}\n",
            "#![cfg(test)]\nfn helper() -> bool {\n    cfg!(test)\n}\n",
            "#[test]\nfn t() {\n    if cfg!(test) {\n        return;\n    }\n}\n",
            "pub fn f() -> bool {\n    cfg!(debug_assertions)\n}\n",
            "pub fn f() {\n    println!(\"{}\", cfg!(test));\n    g(cfg!(test));\n}\n",
        ] {
            assert!(signals(RS, "src/lib.rs", src).is_empty(), "{src}");
        }
        let probe = "pub fn f() -> bool {\n    cfg!(test)\n}\n";
        assert!(signals(RS, "tests/it.rs", probe).is_empty());
        assert_eq!(
            signals(RS, "src/lib.rs", probe).len(),
            1,
            "positive control"
        );
    }

    #[test]
    fn go_probes_under_the_imported_name_are_read() {
        let cases: &[(&str, &str, Signal)] = &[
            (
                "\"testing\"",
                "\tif testing.Testing() {\n\t\treturn 4\n\t}\n",
                Signal::GoTesting,
            ),
            (
                "t \"testing\"",
                "\tif !t.Testing() {\n\t\treturn 4\n\t}\n",
                Signal::GoTesting,
            ),
            (
                "\"flag\"",
                "\tif flag.Lookup(\"test.v\") != nil {\n\t\treturn 4\n\t}\n",
                Signal::GoTestFlag,
            ),
            (
                "\"os\"\n\t\"strings\"",
                "\tisTest := strings.HasSuffix(os.Args[0], \".test\")\n\t_ = isTest\n",
                Signal::GoTestBinaryName,
            ),
        ];
        for (imports, body, want) in cases {
            let src = format!("package p\n\nimport (\n\t{imports}\n)\n\nfunc F(x int) int {{\n{body}\treturn x * 3\n}}\n");
            let got = probes(GO, "p.go", &src);
            assert_eq!(got.len(), 1, "{body}: {got:?}");
            assert_eq!(got[0].1, *want, "{body}");
            assert_eq!(got[0].2, "F", "{body}");
        }
    }

    #[test]
    fn go_calls_through_another_package_or_in_a_test_file_are_not_read() {
        let probe =
            "package p\n\nimport \"testing\"\n\nfunc F() bool {\n\treturn testing.Testing()\n}\n";
        assert_eq!(
            signals(GO, "p.go", probe),
            vec![Signal::GoTesting],
            "positive control"
        );
        assert!(signals(GO, "p_test.go", probe).is_empty());
        for src in [
            // `testing` is not the standard package here.
            "package p\n\nimport testing \"example.com/fake\"\n\nfunc F() bool {\n\treturn testing.Testing()\n}\n",
            "package p\n\nimport (\n\t\"fmt\"\n\t\"testing\"\n)\n\nfunc F() {\n\tfmt.Println(testing.Testing())\n}\n",
            "package p\n\nimport \"flag\"\n\nfunc F() bool {\n\treturn flag.Lookup(\"verbose\") != nil\n}\n",
        ] {
            assert!(signals(GO, "p.go", src).is_empty(), "{src}");
        }
    }

    #[test]
    fn only_the_probes_a_change_adds_are_new() {
        let p = |line: usize, subject: &str| Probe {
            line,
            signal: Signal::RustCfgTest,
            subject: subject.to_string(),
        };
        let base = vec![p(3, "f")];
        assert!(new_probes(&base, &[p(9, "f")]).is_empty(), "moved within f");
        assert_eq!(new_probes(&base, &[p(3, "f"), p(5, "f")]), vec![p(5, "f")]);
        assert_eq!(new_probes(&base, &[p(3, "g")]), vec![p(3, "g")]);
        let other = Probe {
            signal: Signal::GoTesting,
            ..p(3, "f")
        };
        assert_eq!(new_probes(&base, std::slice::from_ref(&other)), vec![other]);
    }
}
