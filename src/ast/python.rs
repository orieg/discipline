//! Python language pack: tree-sitter AST extraction of tests, assertions, and escape hatches.

use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser};

use std::collections::{HashMap, HashSet};

use super::functions::{self, FunctionSpec};
use super::{
    AssertVocabulary, EscapeHatchSite, Fact, HelperFacts, LanguagePack, ParsedFileFacts, TestFn,
};

/// Python language pack implementing [`LanguagePack`].
pub struct PythonPack;

impl LanguagePack for PythonPack {
    fn id(&self) -> &'static str {
        "python"
    }

    fn supplies(&self, fact: Fact) -> bool {
        matches!(
            fact,
            Fact::Tests
                | Fact::EscapeHatches
                | Fact::Functions
                | Fact::Handlers
                | Fact::Prose
                | Fact::Budgets
        )
    }

    fn name(&self) -> &'static str {
        "Python"
    }

    fn matches(&self, path: &str) -> bool {
        matches!(super::extension(path), Some("py" | "pyi"))
    }

    fn extract(&self, path: &str, src: &str, vocab: &AssertVocabulary) -> Result<ParsedFileFacts> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .map_err(|e| anyhow!("failed to load the Python grammar: {e}"))?;
        let tree = parser
            .parse(src, None)
            .ok_or_else(|| anyhow!("tree-sitter returned no tree"))?;
        let root = tree.root_node();

        let mut extractor = PythonExtractor {
            dead: super::reach::dead_ranges(root, src, &PY_REACH),
            src: src.as_bytes(),
            vocab,
            is_test_path: is_python_test_path(path),
            facts: ParsedFileFacts {
                has_parse_errors: root.has_error(),
                ..Default::default()
            },
            class_bases: HashMap::new(),
            collected_classes: HashSet::new(),
            class_stack: Vec::new(),
            helpers: HashMap::new(),
            helper_calls: HashMap::new(),
            test_calls: Vec::new(),
        };

        extractor.collect_class_bases(root);
        extractor.compute_collected_classes();
        extractor.collect_comments_and_escape_hatches(root);
        extractor.visit_root(root);
        extractor.resolve_same_file_helpers();
        extractor.facts.functions = functions::extract(root, src, path, &PYTHON_FUNCTIONS);
        super::mocks::count(
            root,
            src,
            &mut extractor.facts.tests,
            &PYTHON_MOCKS,
            &vocab.mock_setup_fns,
            &vocab.mock_assert_fns,
        );
        {
            let tests = &extractor.facts.tests;
            let spans: Vec<(usize, usize)> = tests
                .iter()
                .map(|t| (t.line, t.end_line.max(t.line)))
                .collect();
            // A file in a test directory, or one the repository declares as test scope, is
            // test code line for line.
            let whole_file = super::functions::test_path(path)
                || super::functions::declared_test_path(path, &vocab.test_paths);
            let is_test_line =
                |l: usize| whole_file || spans.iter().any(|(a, b)| *a <= l && l <= *b);
            extractor.facts.swallowed =
                super::handlers::extract(root, src, &PYTHON_HANDLERS, &is_test_line);
        }
        super::retries::mark(root, src, &mut extractor.facts.tests, &PYTHON_RETRIES);
        if super::functions::declared_test_path(path, &vocab.test_paths) {
            for f in &mut extractor.facts.functions {
                f.is_test = true;
            }
        }
        super::calls::count(
            root,
            src,
            &mut extractor.facts.tests,
            &PYTHON_MOCKS,
            super::calls::SLEEP_VOCAB,
            super::calls::sleeps,
        );
        super::calls::count(
            root,
            src,
            &mut extractor.facts.tests,
            &PYTHON_MOCKS,
            super::calls::TRIVIAL_ASSERT_VOCAB,
            super::calls::trivial_asserts,
        );
        super::bounds::python(root, src, &mut extractor.facts.tests);
        super::calls::count_python_assert_statements(root, src, &mut extractor.facts.tests);
        extractor.facts.prose = super::prose::extract(root, src, &["comment", "string"]);
        extractor.facts.budgets = super::budgets::extract(root, src, &PY_BUDGETS);
        Ok(extractor.facts)
    }
}

/// Determines whether a path is conventionally a Python test file.
pub fn is_python_test_path(path: &str) -> bool {
    let filename = path.rsplit('/').next().unwrap_or(path);
    if filename.starts_with("test_") || filename.ends_with("_test.py") {
        return true;
    }
    path.starts_with("tests/")
        || path.contains("/tests/")
        || path.starts_with("test/")
        || path.contains("/test/")
}

struct PythonExtractor<'a> {
    /// Byte ranges no execution reaches (`super::reach`).
    dead: super::reach::DeadRanges,
    src: &'a [u8],
    vocab: &'a AssertVocabulary,
    is_test_path: bool,
    facts: ParsedFileFacts,
    /// Every class defined in the file, keyed by name, with the last dotted
    /// segment of each base (`unittest.TestCase` -> `TestCase`).
    class_bases: HashMap<String, Vec<String>>,
    /// Classes whose `test*` methods pytest or unittest collects.
    collected_classes: HashSet<String>,
    /// For each enclosing class, whether pytest or unittest collects its methods.
    class_stack: Vec<bool>,
    /// Non-test functions and methods of this file, keyed by scope-qualified
    /// name (`check` or `TestOrders::_expect`), with the failure paths in
    /// their own body.
    helpers: HashMap<String, HelperFacts>,
    /// Scope-qualified callees of each helper, followed to `super::HELPER_DEPTH`.
    helper_calls: HashMap<String, Vec<String>>,
    /// Scope-qualified callees of each test, parallel to `facts.tests`.
    test_calls: Vec<Vec<String>>,
}

/// How a function body is scanned for failure paths.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BodyMode {
    /// A collected test: its own statements, including nested blocks.
    Test,
    /// A same-file helper resolved from a test. A `raise` is a failure path,
    /// the way a non-zero `return` from a C/C++ `main` is; nested `def`,
    /// `class` and `lambda` bodies are skipped because defining them runs
    /// none of their statements.
    Helper,
}

fn statement_has_skip_mark(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.starts_with("pytestmark") {
        let rest = trimmed.strip_prefix("pytestmark").unwrap().trim_start();
        if rest.starts_with('=') {
            return rest.contains("skip") || rest.contains("xfail");
        }
    }
    false
}

fn is_assertion_context_manager(text: &str) -> bool {
    text.contains("pytest.raises")
        || text.contains("pytest.warns")
        || text.contains("assertRaises")
        || text.contains("assertRaisesRegex")
        || text.contains("assertWarns")
        || text.contains("assertWarnsRegex")
        || text.contains("assertLogs")
        || text.contains("assertNoLogs")
}

impl<'a> PythonExtractor<'a> {
    fn text(&self, node: Node) -> &'a str {
        node.utf8_text(self.src).unwrap_or("")
    }

    fn collect_class_bases(&mut self, node: Node) {
        if node.kind() == "class_definition" {
            let name = node
                .child_by_field_name("name")
                .map(|n| self.text(n).to_string())
                .unwrap_or_default();
            let mut bases = Vec::new();
            if let Some(supers) = node.child_by_field_name("superclasses") {
                let mut cursor = supers.walk();
                for base in supers.named_children(&mut cursor) {
                    if matches!(base.kind(), "identifier" | "attribute") {
                        let text = self.text(base);
                        bases.push(text.rsplit('.').next().unwrap_or(text).to_string());
                    }
                }
            }
            self.class_bases.entry(name).or_insert(bases);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_class_bases(child);
        }
    }

    /// Whether class `name` is itself a test class.
    ///
    /// pytest collects classes whose name starts with `Test`; unittest
    /// collects `TestCase` subclasses. A base is followed through classes
    /// defined in this file (cycle-guarded); a base defined elsewhere counts
    /// when its own name reads as a test case (`TestCase`, `APITestCase`,
    /// `TestBase`), since discipline does not resolve imports.
    fn class_is_test_case(&self, name: &str) -> bool {
        let mut visited = HashSet::new();
        self.class_is_collected_inner(name, &mut visited)
    }

    /// Classes whose `test*` methods a runner collects: the test classes, plus
    /// every same-file class they inherit from (a mixin's `test_*` methods run
    /// as part of the subclass). The closure walks each base once, so an
    /// inheritance cycle terminates.
    fn compute_collected_classes(&mut self) {
        let mut collected: HashSet<String> = self
            .class_bases
            .keys()
            .filter(|name| self.class_is_test_case(name))
            .cloned()
            .collect();
        let mut stack: Vec<String> = collected.iter().cloned().collect();
        while let Some(name) = stack.pop() {
            if let Some(bases) = self.class_bases.get(&name) {
                for base in bases {
                    if self.class_bases.contains_key(base) && collected.insert(base.clone()) {
                        stack.push(base.clone());
                    }
                }
            }
        }
        self.collected_classes = collected;
    }

    fn class_is_collected_inner<'n>(
        &'n self,
        name: &'n str,
        visited: &mut HashSet<&'n str>,
    ) -> bool {
        if name.starts_with("Test") {
            return true;
        }
        if !visited.insert(name) {
            return false;
        }
        let Some(bases) = self.class_bases.get(name) else {
            return false;
        };
        bases.iter().any(|base| {
            if self.class_bases.contains_key(base.as_str()) {
                self.class_is_collected_inner(base, visited)
            } else {
                base.starts_with("Test") || base.ends_with("TestCase")
            }
        })
    }

    fn collect_comments_and_escape_hatches(&mut self, node: Node) {
        if node.kind() == "comment" {
            let text = self.text(node);
            let line = node.start_position().row + 1;
            let trimmed = text.trim_start_matches('#').trim();

            if trimmed.starts_with("type: ignore") {
                self.facts.escape_hatches.push(EscapeHatchSite::TypeIgnore {
                    line,
                    tool: "mypy".to_string(),
                    snippet: text.to_string(),
                });
            } else if trimmed.starts_with("noqa") || trimmed.starts_with("ruff: noqa") {
                let rule = if let Some(rest) = trimmed.strip_prefix("noqa:") {
                    rest.trim().to_string()
                } else if let Some(rest) = trimmed.strip_prefix("ruff: noqa:") {
                    rest.trim().to_string()
                } else {
                    "all".to_string()
                };
                self.facts
                    .escape_hatches
                    .push(EscapeHatchSite::LinterDisable {
                        line,
                        rule,
                        snippet: text.to_string(),
                    });
            } else if trimmed.starts_with("pragma: no cover") {
                self.facts
                    .escape_hatches
                    .push(EscapeHatchSite::LinterDisable {
                        line,
                        rule: "coverage".to_string(),
                        snippet: text.to_string(),
                    });
            } else if trimmed.starts_with("pylint: disable=") {
                let rule = trimmed
                    .strip_prefix("pylint: disable=")
                    .unwrap_or("")
                    .trim()
                    .to_string();
                self.facts
                    .escape_hatches
                    .push(EscapeHatchSite::LinterDisable {
                        line,
                        rule,
                        snippet: text.to_string(),
                    });
            }
            return;
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_comments_and_escape_hatches(child);
        }
    }

    fn visit_root(&mut self, root: Node) {
        let mut module_ignored = false;
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            if statement_has_skip_mark(self.text(child)) {
                module_ignored = true;
                break;
            }
        }
        let mut scope = Vec::new();
        self.visit_node(root, &mut scope, module_ignored);
    }

    fn visit_node(&mut self, node: Node, scope: &mut Vec<String>, parent_ignored: bool) {
        match node.kind() {
            "function_definition" => {
                self.process_function_with_inherited_ignore(node, scope, None, parent_ignored);
            }
            "decorated_definition" => {
                let mut decorators = Vec::new();
                let mut inner_def = None;

                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "decorator" {
                        decorators.push(child);
                    } else if child.kind() == "function_definition"
                        || child.kind() == "class_definition"
                    {
                        inner_def = Some(child);
                    }
                }

                if let Some(def) = inner_def {
                    if def.kind() == "function_definition" {
                        self.process_function_with_inherited_ignore(
                            def,
                            scope,
                            Some(&decorators),
                            parent_ignored,
                        );
                    } else if def.kind() == "class_definition" {
                        self.process_class(def, scope, Some(&decorators), parent_ignored);
                    }
                }
            }
            "class_definition" => {
                self.process_class(node, scope, None, parent_ignored);
            }
            _ => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.visit_node(child, scope, parent_ignored);
                }
            }
        }
    }

    fn process_class(
        &mut self,
        node: Node,
        scope: &mut Vec<String>,
        class_decorators: Option<&[Node]>,
        parent_ignored: bool,
    ) {
        let class_name = node
            .child_by_field_name("name")
            .map(|n| self.text(n).to_string())
            .unwrap_or_default();

        let class_has_skip_mark = if let Some(body) = node.child_by_field_name("body") {
            let mut cursor = body.walk();
            let has_skip = body
                .children(&mut cursor)
                .any(|c| statement_has_skip_mark(self.text(c)));
            has_skip
        } else {
            false
        };

        let class_is_ignored = parent_ignored
            || class_has_skip_mark
            || class_decorators
                .map(|decs| decs.iter().any(|d| self.is_skip_decorator(*d)))
                .unwrap_or(false);

        let collected = self.collected_classes.contains(&class_name);
        scope.push(class_name);
        self.class_stack.push(collected);

        if let Some(body) = node.child_by_field_name("body") {
            let mut cursor = body.walk();
            for child in body.children(&mut cursor) {
                if child.kind() == "function_definition" {
                    self.process_function_with_inherited_ignore(
                        child,
                        scope,
                        None,
                        class_is_ignored,
                    );
                } else if child.kind() == "decorated_definition" {
                    let mut decorators = Vec::new();
                    let mut inner_fn = None;
                    let mut c = child.walk();
                    for grandchild in child.children(&mut c) {
                        if grandchild.kind() == "decorator" {
                            decorators.push(grandchild);
                        } else if grandchild.kind() == "function_definition" {
                            inner_fn = Some(grandchild);
                        }
                    }
                    if let Some(f) = inner_fn {
                        self.process_function_with_inherited_ignore(
                            f,
                            scope,
                            Some(&decorators),
                            class_is_ignored,
                        );
                    }
                } else {
                    self.visit_node(child, scope, class_is_ignored);
                }
            }
        }

        self.class_stack.pop();
        scope.pop();
    }

    fn process_function_with_inherited_ignore(
        &mut self,
        node: Node,
        scope: &[String],
        decorators: Option<&[Node]>,
        parent_ignored: bool,
    ) {
        let fn_name = node
            .child_by_field_name("name")
            .map(|n| self.text(n).to_string())
            .unwrap_or_default();

        let full_name = if scope.is_empty() {
            fn_name.clone()
        } else {
            format!("{}::{}", scope.join("::"), fn_name)
        };

        if !self.is_collected_test(&fn_name, decorators) {
            self.record_helper(node, full_name);
            return;
        }

        // Check decorators for skips
        let mut ignored = parent_ignored;
        if let Some(decs) = decorators {
            for dec in decs {
                if self.is_skip_decorator(*dec) {
                    ignored = true;
                }
            }
        }

        let line = node.start_position().row + 1;
        let end_line = node.end_position().row + 1;
        let mut test_fn = TestFn {
            name: full_name,
            line,
            end_line,
            total_asserts: 0,
            strong_asserts: 0,
            tautologies: 0,
            ignored,
            should_panic: false,
            ..Default::default()
        };

        let mut calls = Vec::new();
        if let Some(body) = node.child_by_field_name("body") {
            self.scan_test_body(body, &mut test_fn, BodyMode::Test);
            self.collect_calls(body, scope, &mut calls);
        }

        self.facts.tests.push(test_fn);
        self.test_calls.push(calls);
    }

    /// pytest and unittest collection rules, with their default settings.
    ///
    /// * A module-level function is collected when its name starts with
    ///   `test` (pytest's `python_functions`). Outside a test path only the
    ///   `test_` spelling and bare `test` count, so `testing_mode()` in
    ///   application code is not mistaken for a test.
    /// * A method is collected when its name starts with `test` and its class
    ///   is collected ([`Self::compute_collected_classes`]). In a test path the class
    ///   rule is relaxed: a mixin there (`class ContractTests:` with `test_*`
    ///   methods) runs through a `TestCase` subclass that may live in another
    ///   file, which discipline cannot see, and dropping it would hide erosion
    ///   of the tests it contributes.
    /// * `_private` names, unittest lifecycle hooks, and `@staticmethod` /
    ///   `@classmethod` / `@property` members are never tests.
    ///
    /// No other name is special: `self_test()` is a script entry point that
    /// neither runner collects, unless `[tests].functions` declares it, which wins over
    /// every rule above.
    fn is_collected_test(&self, fn_name: &str, decorators: Option<&[Node]>) -> bool {
        // A name the repository declares (`[tests].functions`) is its test entry point
        // whatever its spelling: `_self_test` is private by convention, not by rule.
        if self.vocab.test_functions.iter().any(|f| f == fn_name) {
            return true;
        }
        if fn_name.starts_with('_')
            || matches!(
                fn_name,
                "setUp"
                    | "tearDown"
                    | "setUpClass"
                    | "tearDownClass"
                    | "setUpModule"
                    | "tearDownModule"
                    | "asyncSetUp"
                    | "asyncTearDown"
            )
        {
            return false;
        }
        if decorators.is_some_and(|decs| decs.iter().any(|d| self.is_non_test_decorator(*d))) {
            return false;
        }
        match self.class_stack.last() {
            Some(&collected) => (collected || self.is_test_path) && fn_name.starts_with("test"),
            None => {
                fn_name.starts_with("test_")
                    || fn_name == "test"
                    || (self.is_test_path && fn_name.starts_with("test"))
            }
        }
    }

    /// Records a non-test function as a helper a test may call.
    fn record_helper(&mut self, node: Node, key: String) {
        let mut facts = TestFn::default();
        let mut calls = Vec::new();
        if let Some(body) = node.child_by_field_name("body") {
            self.scan_test_body(body, &mut facts, BodyMode::Helper);
            // `Class::method` calls `self.other()` in its class's scope.
            let scope: Vec<String> = key
                .rsplit_once("::")
                .map(|(s, _)| s.split("::").map(str::to_string).collect())
                .unwrap_or_default();
            self.collect_calls(body, &scope, &mut calls);
        }
        self.helper_calls.entry(key.clone()).or_insert(calls);
        self.helpers.entry(key).or_insert(HelperFacts {
            total_asserts: facts.total_asserts,
            strong_asserts: facts.strong_asserts,
            tautologies: facts.tautologies,
            fatal_asserts: facts.fatal_asserts,
        });
    }

    /// Collects the same-file callees of a test body: `name(...)` resolves to
    /// a module-level function, `self.name(...)` / `cls.name(...)` to a method
    /// of the enclosing class. Calls inside nested `def` / `lambda` bodies are
    /// not collected: defining them runs nothing.
    fn collect_calls(&self, node: Node, scope: &[String], calls: &mut Vec<String>) {
        match node.kind() {
            "function_definition" | "decorated_definition" | "class_definition" | "lambda" => {
                return;
            }
            "call" => {
                if let Some(func) = node.child_by_field_name("function") {
                    let text = self.text(func);
                    if func.kind() == "identifier" {
                        calls.push(text.to_string());
                    } else if let Some(method) = text
                        .strip_prefix("self.")
                        .or_else(|| text.strip_prefix("cls."))
                    {
                        if !scope.is_empty() && !method.contains('.') {
                            calls.push(format!("{}::{}", scope.join("::"), method));
                        }
                    }
                }
            }
            // A dispatch table: `steps = [("label", check_blocks), ...]` then
            // `for _, fn in steps: fn()`. A function named as an element of a
            // list, tuple or set runs through the loop; an unknown name resolves
            // to nothing.
            "identifier"
                if node
                    .parent()
                    .is_some_and(|p| matches!(p.kind(), "list" | "tuple" | "set")) =>
            {
                calls.push(self.text(node).to_string());
            }
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_calls(child, scope, calls);
        }
    }

    /// Adds the failure paths of each same-file helper a test calls, and of the helpers
    /// those call, up to `super::HELPER_DEPTH` calls deep; a recursive call is not
    /// followed again, and nothing crosses a file boundary.
    fn resolve_same_file_helpers(&mut self) {
        let (helpers, helper_calls) = (&self.helpers, &self.helper_calls);
        for (test, calls) in self.facts.tests.iter_mut().zip(&self.test_calls) {
            for call in calls {
                let mut path = Vec::new();
                let Some(h) = super::transitive_helper(call, helpers, helper_calls, &mut path)
                else {
                    continue;
                };
                // A configured assertion helper was already counted once at
                // the call site; its body now speaks for it.
                let leaf = call.rsplit("::").next().unwrap_or(call);
                if self.vocab.helper_fns.iter().any(|name| name == leaf) {
                    test.total_asserts = test.total_asserts.saturating_sub(1);
                }
                test.total_asserts += h.total_asserts;
                test.strong_asserts += h.strong_asserts;
                test.tautologies += h.tautologies;
                test.fatal_asserts += h.fatal_asserts;
                if h.total_asserts > h.tautologies {
                    test.helper_checks += 1;
                }
            }
        }
    }

    fn is_skip_decorator(&self, dec: Node) -> bool {
        let text = self.text(dec).trim();
        text.contains("pytest.mark.skip")
            || text.contains("pytest.mark.xfail")
            || text.contains("unittest.skip")
            || text.contains("skipIf")
            || text.contains("skipUnless")
            || text.starts_with("@skip")
            || text.starts_with("@unittest.skip")
            || text.starts_with("@pytest.mark.skip")
            || text.starts_with("@pytest.mark.xfail")
    }

    fn is_non_test_decorator(&self, dec: Node) -> bool {
        let text = self.text(dec).trim();
        text.contains("staticmethod") || text.contains("classmethod") || text.contains("property")
    }

    fn scan_test_body(&self, body: Node, test: &mut TestFn, mode: BodyMode) {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            self.visit_body_node(child, test, mode);
        }
    }

    fn visit_body_node(&self, node: Node, test: &mut TestFn, mode: BodyMode) {
        if super::reach::is_dead(&self.dead, node.start_byte()) {
            return;
        }
        match node.kind() {
            "function_definition" | "decorated_definition" | "class_definition" | "lambda"
                if mode == BodyMode::Helper => {}
            "raise_statement" if mode == BodyMode::Helper => {
                test.total_asserts += 1;
                test.strong_asserts += 1;
            }
            "assert_statement" => {
                test.total_asserts += 1;
                let mut cursor = node.walk();
                let children: Vec<_> = node.children(&mut cursor).collect();
                // In Python: assert condition, message
                // Find condition expression
                let condition = children
                    .iter()
                    .copied()
                    .find(|c| c.kind() != "assert" && c.kind() != "," && c.kind() != "comment");

                if let Some(cond) = condition {
                    if self.is_tautological(cond) {
                        test.tautologies += 1;
                    }
                    if self.is_strong_assertion(cond) {
                        test.strong_asserts += 1;
                    }
                }
            }
            "with_statement" => {
                let mut found_items = 0;
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "with_clause" {
                        let mut c2 = child.walk();
                        for item in child.children(&mut c2) {
                            if item.kind() == "with_item" {
                                found_items += 1;
                                if is_assertion_context_manager(self.text(item)) {
                                    test.total_asserts += 1;
                                    test.strong_asserts += 1;
                                }
                            }
                        }
                    } else if child.kind() == "with_item" {
                        found_items += 1;
                        if is_assertion_context_manager(self.text(child)) {
                            test.total_asserts += 1;
                            test.strong_asserts += 1;
                        }
                    } else if child.kind() == "block" {
                        self.scan_test_body(child, test, mode);
                    }
                }
                if found_items == 0 {
                    let header_text = if let Some(block) = node.child_by_field_name("body") {
                        let start = node.start_byte();
                        let end = block.start_byte().min(self.src.len());
                        std::str::from_utf8(&self.src[start..end]).unwrap_or("")
                    } else {
                        self.text(node)
                    };
                    if is_assertion_context_manager(header_text) {
                        test.total_asserts += 1;
                        test.strong_asserts += 1;
                    }
                }
            }
            "call" => {
                let call_text = self.text(node);
                if let Some(func_node) = node.child_by_field_name("function") {
                    let func_name = self.text(func_node);
                    if func_name.starts_with("self.assert") {
                        test.total_asserts += 1;
                        if self.is_strong_unittest_assert(func_name) {
                            test.strong_asserts += 1;
                        }
                        if self.is_tautological_call(node, func_name) {
                            test.tautologies += 1;
                        }
                    } else if func_name == "pytest.skip"
                        || func_name == "pytest.xfail"
                        || func_name == "self.skipTest"
                    {
                        test.ignored = true;
                    } else if self
                        .vocab
                        .helper_fns
                        .iter()
                        .any(|h| func_name == h || func_name.ends_with(&format!(".{h}")))
                    {
                        test.total_asserts += 1;
                    }
                }
                if call_text.contains("pytest.raises") && !call_text.starts_with("with ") {
                    test.total_asserts += 1;
                    test.strong_asserts += 1;
                }
            }
            "block" => {
                self.scan_test_body(node, test, mode);
            }
            _ => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.visit_body_node(child, test, mode);
                }
            }
        }
    }

    fn is_strong_assertion(&self, cond: Node) -> bool {
        match cond.kind() {
            "comparison_operator" => {
                // Check operator: ==, !=, in, not in, is, is not
                let text = self.text(cond);
                text.contains("==")
                    || text.contains("!=")
                    || text.contains(" in ")
                    || text.contains(" not in ")
                    || text.contains(" is ")
                    || text.contains(" is not ")
            }
            "call" => {
                let text = self.text(cond);
                text.contains("isinstance") || text.contains("issubclass")
            }
            _ => false,
        }
    }

    fn is_strong_unittest_assert(&self, func_name: &str) -> bool {
        matches!(
            func_name,
            "self.assertEqual"
                | "self.assertNotEqual"
                | "self.assertIn"
                | "self.assertNotIn"
                | "self.assertIs"
                | "self.assertIsNot"
                | "self.assertIsNone"
                | "self.assertIsNotNone"
                | "self.assertIsInstance"
                | "self.assertNotIsInstance"
                | "self.assertAlmostEqual"
                | "self.assertNotAlmostEqual"
                | "self.assertCountEqual"
                | "self.assertRegex"
                | "self.assertNotRegex"
                | "self.assertRaises"
                | "self.assertRaisesRegex"
                | "self.assertWarns"
                | "self.assertWarnsRegex"
                | "self.assertLogs"
                | "self.assertNoLogs"
        )
    }

    fn is_tautological(&self, cond: Node) -> bool {
        let text = self.text(cond).trim();
        if text == "True" || text == "1" || text == "\"\"" || text == "''" {
            return true;
        }
        if cond.kind() == "comparison_operator" {
            let mut cursor = cond.walk();
            let children: Vec<_> = cond.children(&mut cursor).collect();
            if children.len() >= 3 {
                let left = self.text(children[0]).trim();
                let op = self.text(children[1]).trim();
                let right = self.text(children[2]).trim();
                if (op == "==" || op == "is") && left == right && !left.is_empty() {
                    return true;
                }
            }
        }
        false
    }

    fn is_tautological_call(&self, call: Node, func_name: &str) -> bool {
        if let Some(args) = call.child_by_field_name("arguments") {
            let mut cursor = args.walk();
            let arg_nodes: Vec<_> = args
                .children(&mut cursor)
                .filter(|n| {
                    n.kind() != "(" && n.kind() != ")" && n.kind() != "," && n.kind() != "comment"
                })
                .collect();
            if !arg_nodes.is_empty() {
                let first = self.text(arg_nodes[0]).trim();
                if func_name == "self.assertTrue" && (first == "True" || first == "1") {
                    return true;
                }
                if func_name == "self.assertFalse" && (first == "False" || first == "0") {
                    return true;
                }
            }
            if (func_name == "self.assertEqual" || func_name == "self.assertIs")
                && arg_nodes.len() >= 2
            {
                let a = self.text(arg_nodes[0]).trim();
                let b = self.text(arg_nodes[1]).trim();
                if a == b && !a.is_empty() {
                    return true;
                }
            }
        }
        false
    }
}

/// Abstract methods, overload signatures, Protocol members and `.pyi` stubs are
/// declarations, not bodies to judge.
fn python_fn_skip(node: tree_sitter::Node, src: &str) -> bool {
    let t = |n: tree_sitter::Node| n.utf8_text(src.as_bytes()).unwrap_or("");
    if let Some(parent) = node.parent() {
        if parent.kind() == "decorated_definition" {
            let mut cursor = parent.walk();
            for child in parent.children(&mut cursor) {
                if child.kind() == "decorator" {
                    let d = t(child);
                    if d.contains("abstractmethod")
                        || d.contains("overload")
                        || d.contains("abstractproperty")
                    {
                        return true;
                    }
                }
            }
        }
    }
    let fn_name = node
        .child_by_field_name("name")
        .map(t)
        .unwrap_or("")
        .to_string();
    let mut cur = node.parent();
    while let Some(p) = cur {
        if p.kind() == "class_definition" {
            if let Some(sup) = p.child_by_field_name("superclasses") {
                let s = t(sup);
                if s.contains("Protocol")
                    || s.contains("TypedDict")
                    || s.contains("NamedTuple")
                    || s.contains("ABC")
                {
                    return true;
                }
            }
            // A base class whose method a same-file subclass overrides: the stub is the
            // abstract contract, not an unimplemented function.
            let class_name = p.child_by_field_name("name").map(t).unwrap_or("");
            if !class_name.is_empty() && overridden_in_subclass(p, class_name, &fn_name, src) {
                return true;
            }
        }
        cur = p.parent();
    }
    false
}

fn overridden_in_subclass(
    class: tree_sitter::Node,
    class_name: &str,
    method: &str,
    src: &str,
) -> bool {
    let mut root = class;
    while let Some(p) = root.parent() {
        root = p;
    }
    let t = |n: tree_sitter::Node| n.utf8_text(src.as_bytes()).unwrap_or("");
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if n.kind() == "class_definition" && n != class {
            let derives = n
                .child_by_field_name("superclasses")
                .map(t)
                .is_some_and(|s| {
                    s.split(|c: char| !c.is_alphanumeric() && c != '_')
                        .any(|x| x == class_name)
                });
            if derives {
                if let Some(body) = n.child_by_field_name("body") {
                    let mut c = body.walk();
                    let has = body.children(&mut c).any(|m| {
                        let def = if m.kind() == "decorated_definition" {
                            m.child_by_field_name("definition").unwrap_or(m)
                        } else {
                            m
                        };
                        def.kind() == "function_definition"
                            && def.child_by_field_name("name").map(t) == Some(method)
                    });
                    if has {
                        return true;
                    }
                }
            }
        }
        let mut c = n.walk();
        let kids: Vec<_> = n.children(&mut c).collect();
        stack.extend(kids);
    }
    false
}

fn python_fn_is_test(node: tree_sitter::Node, src: &str, path: &str) -> bool {
    if path.ends_with(".pyi") {
        return true;
    }
    let name = node
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(src.as_bytes()).ok())
        .unwrap_or("");
    functions::test_path(path) || name.starts_with("test_") || is_python_test_path(path)
}

pub const PYTHON_FUNCTIONS: FunctionSpec = FunctionSpec {
    function_kinds: &["function_definition"],
    name_fields: &["name"],
    body_fields: &["body"],
    // A docstring is an expression statement holding a string.
    ignored_kinds: &["comment"],
    skip: python_fn_skip,
    is_test: python_fn_is_test,
    classify: functions::classify_python,
};

pub const PYTHON_MOCKS: super::mocks::MockSpec = super::mocks::MockSpec {
    call_kinds: &["call"],
    callee_fields: &["function"],
};

pub const PYTHON_HANDLERS: super::handlers::HandlerSpec = super::handlers::HandlerSpec {
    handler_kinds: &["except_clause"],
    arm_of: &[],
    body_fields: &["block"],
    ignored_kinds: &["comment"],
    trivial: &["pass", "...", "return", "return None", "continue"],
    discard_kinds: &[],
    discards: super::handlers::no_discard,
    classify_discard: None,
    call_value_kinds: &[],
    silence_kinds: &[],
    silences: super::handlers::no_discard,
};

pub const PYTHON_RETRIES: super::retries::RetrySpec = super::retries::RetrySpec {
    marker_kinds: &["decorator", "call"],
};

pub const PY_BUDGETS: super::budgets::BudgetSpec = super::budgets::BudgetSpec {
    key_values: &[super::budgets::KeyValueShape {
        kind: "keyword_argument",
        key_field: "name",
        value_field: "value",
    }],
    keys: &[
        ("max_examples", "hypothesis max_examples"),
        ("deadline", "hypothesis deadline"),
    ],
    call_kind: "call",
    callee_field: "function",
    arguments_field: "arguments",
    methods: &[],
    integer_kinds: &["integer"],
    token_tree_kinds: &[],
};

pub const PY_REACH: super::reach::ReachSpec = super::reach::ReachSpec {
    if_kinds: &["if_statement"],
    block_kinds: &["block"],
    ignored_kinds: &["comment"],
    terminators: &[
        "return",
        "raise",
        "pytest.fail(",
        "pytest.xfail(",
        "sys.exit(",
        "self.fail(",
        "continue",
        "break",
    ],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pytest_functions_and_assertions() {
        let src = r#"
# type: ignore
def test_addition():
    # noqa: E501
    x = 1
    assert x + 1 == 2
    assert x > 0

def test_vacuous():
    pass

@pytest.mark.skip(reason="unimplemented")
def test_skipped():
    assert 1 == 1
"#;
        let vocab = AssertVocabulary::default();
        let facts = PythonPack
            .extract("tests/test_calc.py", src, &vocab)
            .unwrap();

        assert_eq!(facts.tests.len(), 3);

        let t0 = &facts.tests[0];
        assert_eq!(t0.name, "test_addition");
        assert_eq!(t0.total_asserts, 2);
        assert_eq!(t0.strong_asserts, 1);
        assert!(!t0.is_vacuous());
        assert!(!t0.ignored);

        let t1 = &facts.tests[1];
        assert_eq!(t1.name, "test_vacuous");
        assert_eq!(t1.total_asserts, 0);
        assert!(t1.is_vacuous());

        let t2 = &facts.tests[2];
        assert_eq!(t2.name, "test_skipped");
        assert!(t2.ignored);
        assert_eq!(t2.tautologies, 1);

        assert_eq!(facts.escape_hatches.len(), 2);
        assert!(matches!(
            facts.escape_hatches[0],
            EscapeHatchSite::TypeIgnore { .. }
        ));
        assert!(matches!(
            facts.escape_hatches[1],
            EscapeHatchSite::LinterDisable { .. }
        ));
    }

    #[test]
    fn parses_unittest_classes_and_methods() {
        let src = r#"
import unittest

class TestMath(unittest.TestCase):
    def test_multiplication(self):
        self.assertEqual(2 * 3, 6)
        self.assertTrue(6 > 0)

    @unittest.skip("skip reason")
    def test_skipped_method(self):
        self.assertEqual(1, 1)
"#;
        let vocab = AssertVocabulary::default();
        let facts = PythonPack
            .extract("tests/test_math.py", src, &vocab)
            .unwrap();

        assert_eq!(facts.tests.len(), 2);

        let t0 = &facts.tests[0];
        assert_eq!(t0.name, "TestMath::test_multiplication");
        assert_eq!(t0.total_asserts, 2);
        assert_eq!(t0.strong_asserts, 1);
        assert!(!t0.ignored);

        let t1 = &facts.tests[1];
        assert_eq!(t1.name, "TestMath::test_skipped_method");
        assert!(t1.ignored);
        assert_eq!(t1.tautologies, 1);
    }

    #[test]
    fn parses_pytest_raises_and_helpers() {
        let src = r#"
import pytest

def test_errors():
    with pytest.raises(ValueError):
        int("abc")

def test_helpers():
    custom_assert(123)
"#;
        let vocab = AssertVocabulary {
            helper_fns: vec!["custom_assert".to_string()],
            ..Default::default()
        };
        let facts = PythonPack.extract("test_err.py", src, &vocab).unwrap();

        assert_eq!(facts.tests.len(), 2);
        assert_eq!(facts.tests[0].total_asserts, 1);
        assert_eq!(facts.tests[0].strong_asserts, 1);
        assert!(!facts.tests[0].is_vacuous());

        assert_eq!(facts.tests[1].total_asserts, 1);
        assert!(!facts.tests[1].is_vacuous());
    }

    #[test]
    fn parses_unittest_context_managers_and_pytestmark() {
        let src = r#"
import unittest
import pytest

pytestmark = pytest.mark.skip("module skipped")

def test_module_skipped():
    pass

class TestContextManagers(unittest.TestCase):
    def test_assert_raises(self):
        with self.assertRaises(ValueError):
            int("abc")

    def test_assert_raises_regex(self):
        with self.assertRaisesRegex(ValueError, r"invalid literal"):
            int("xyz")

    def test_assert_warns(self):
        with self.assertWarns(UserWarning):
            do_warn()

    def test_assert_logs(self):
        with self.assertLogs("app", level="INFO"):
            log_info()

@unittest.skip("class skipped")
class TestSkippedClass(unittest.TestCase):
    def test_one(self):
        pass

class TestClassPytestmark(unittest.TestCase):
    pytestmark = pytest.mark.skip("class skip via mark")

    def test_two(self):
        pass
"#;
        let vocab = AssertVocabulary::default();
        let facts = PythonPack.extract("tests/test_f5.py", src, &vocab).unwrap();

        let by_name = |n: &str| facts.tests.iter().find(|t| t.name == n).unwrap();

        // 1. Module pytestmark marks top-level test ignored
        assert!(by_name("test_module_skipped").ignored);

        // 2. with self.assertRaises counted as assertion (not vacuous, strong)
        let t_raises = by_name("TestContextManagers::test_assert_raises");
        assert_eq!(t_raises.total_asserts, 1);
        assert_eq!(t_raises.strong_asserts, 1);
        assert!(!t_raises.is_vacuous());

        // 3. with self.assertRaisesRegex counted
        let t_regex = by_name("TestContextManagers::test_assert_raises_regex");
        assert_eq!(t_regex.total_asserts, 1);
        assert_eq!(t_regex.strong_asserts, 1);
        assert!(!t_regex.is_vacuous());

        // 4. with self.assertWarns counted
        let t_warns = by_name("TestContextManagers::test_assert_warns");
        assert_eq!(t_warns.total_asserts, 1);
        assert_eq!(t_warns.strong_asserts, 1);
        assert!(!t_warns.is_vacuous());

        // 5. with self.assertLogs counted
        let t_logs = by_name("TestContextManagers::test_assert_logs");
        assert_eq!(t_logs.total_asserts, 1);
        assert_eq!(t_logs.strong_asserts, 1);
        assert!(!t_logs.is_vacuous());

        // 6. @unittest.skip on class marks method ignored
        assert!(by_name("TestSkippedClass::test_one").ignored);

        // 7. pytestmark on class marks method ignored
        assert!(by_name("TestClassPytestmark::test_two").ignored);
    }

    #[test]
    fn fixture_negative_control_clean_suite_passes() {
        let src = include_str!("../../tests/fixtures/python/clean_suite.py");
        let vocab = AssertVocabulary::default();
        let facts = PythonPack
            .extract("tests/clean.py", src, &vocab)
            .expect("extract clean");
        assert!(!facts.has_parse_errors);
        assert_eq!(facts.tests.len(), 3);
        for t in &facts.tests {
            assert!(!t.is_vacuous(), "test {} should not be vacuous", t.name);
            assert!(!t.ignored, "test {} should not be ignored", t.name);
            assert!(t.total_asserts >= 1);
            assert!(t.strong_asserts >= 1);
        }
    }

    #[test]
    fn fixture_vacuous_and_tautologies_detected() {
        let src = include_str!("../../tests/fixtures/python/vacuous_suite.py");
        let vocab = AssertVocabulary::default();
        let facts = PythonPack
            .extract("tests/vacuous.py", src, &vocab)
            .expect("extract vacuous");
        assert_eq!(facts.tests.len(), 5);
        for t in &facts.tests {
            assert!(t.is_vacuous(), "test {} should be vacuous", t.name);
        }
        let empty = facts.tests.iter().find(|t| t.name == "test_empty").unwrap();
        assert_eq!(empty.total_asserts, 0);

        let t_bool = facts
            .tests
            .iter()
            .find(|t| t.name == "test_tautology_bool")
            .unwrap();
        assert!(t_bool.tautologies >= 1);

        let t_math = facts
            .tests
            .iter()
            .find(|t| t.name == "test_tautology_math")
            .unwrap();
        assert!(t_math.tautologies >= 1);

        let t_eq = facts
            .tests
            .iter()
            .find(|t| t.name.ends_with("test_tautology_assert_equal"))
            .unwrap();
        assert!(t_eq.tautologies >= 1);

        let t_true = facts
            .tests
            .iter()
            .find(|t| t.name.ends_with("test_tautology_assert_true"))
            .unwrap();
        assert!(t_true.tautologies >= 1);
    }

    #[test]
    fn fixture_implicit_assertions_detected() {
        let src = include_str!("../../tests/fixtures/python/implicit_asserts.py");
        let vocab = AssertVocabulary::default();
        let facts = PythonPack
            .extract("tests/implicit.py", src, &vocab)
            .expect("extract implicit");
        assert_eq!(facts.tests.len(), 5);
        for t in &facts.tests {
            assert!(!t.is_vacuous(), "test {} should not be vacuous", t.name);
            assert!(t.total_asserts >= 1);
            assert!(t.strong_asserts >= 1);
        }
    }

    #[test]
    fn fixture_skips_at_all_levels_detected() {
        let src = include_str!("../../tests/fixtures/python/skips_suite.py");
        let vocab = AssertVocabulary::default();
        let facts = PythonPack
            .extract("tests/skips.py", src, &vocab)
            .expect("extract skips");
        assert_eq!(facts.tests.len(), 4);
        for t in &facts.tests {
            assert!(t.ignored, "test {} should be marked ignored", t.name);
        }
    }

    #[test]
    fn fixture_comments_and_strings_not_counted_as_assertions() {
        let src = include_str!("../../tests/fixtures/python/comments_and_strings.py");
        let vocab = AssertVocabulary::default();
        let facts = PythonPack
            .extract("tests/comments.py", src, &vocab)
            .expect("extract comments");
        assert_eq!(facts.tests.len(), 1);
        assert_eq!(facts.tests[0].total_asserts, 1);
        assert_eq!(facts.tests[0].strong_asserts, 1);
        assert!(!facts.tests[0].is_vacuous());
    }

    #[test]
    fn fixture_same_file_helper_resolves_without_configuration() {
        // `assert_positive` is defined in the same file, so its `assert` is
        // resolved one level from the direct call, as the Rust pack does. The
        // call inside the lambda is not: defining a lambda runs nothing.
        let src = include_str!("../../tests/fixtures/python/helpers_and_lambdas.py");
        let unconfigured_vocab = AssertVocabulary::default();
        let unconfigured = PythonPack
            .extract("tests/helpers.py", src, &unconfigured_vocab)
            .unwrap();
        assert_eq!(unconfigured.tests.len(), 1);
        assert_eq!(unconfigured.tests[0].total_asserts, 1);
        assert!(!unconfigured.tests[0].is_vacuous());

        let configured_vocab = AssertVocabulary {
            helper_fns: vec!["assert_positive".to_string()],
            ..Default::default()
        };
        let configured = PythonPack
            .extract("tests/helpers.py", src, &configured_vocab)
            .unwrap();
        assert_eq!(configured.tests[0].total_asserts, 2);
        assert!(!configured.tests[0].is_vacuous());
    }

    #[test]
    fn fixture_syntax_error_surfaces_parse_error() {
        let src = include_str!("../../tests/fixtures/python/syntax_error.py");
        let vocab = AssertVocabulary::default();
        let facts = PythonPack
            .extract("tests/broken.py", src, &vocab)
            .expect("extract broken");
        assert!(facts.has_parse_errors);
    }

    #[test]
    fn test_class_helpers_and_lifecycle_hooks_are_ignored() {
        let src = r#"
class TestWriterTarget930:
    @staticmethod
    def _cell(arg):
        return arg * 2

    def setUp(self):
        self.x = 1

    def helper_calc(self):
        return 42

    def test_real_case(self):
        assert self.helper_calc() == 42
"#;
        let vocab = AssertVocabulary::default();
        let facts = PythonPack
            .extract("tests/test_writer.py", src, &vocab)
            .expect("extract test_writer");
        assert_eq!(facts.tests.len(), 1);
        assert_eq!(facts.tests[0].name, "TestWriterTarget930::test_real_case");
        assert!(!facts.tests[0].is_vacuous());
    }

    #[test]
    fn raising_same_file_helper_counts_as_assertion() {
        // A consumer refactored inline asserts into module-level helpers that
        // `raise` on mismatch; the helper is the failure path.
        let src = r#"
def check_schedule(got, want):
    if got != want:
        raise AssertionError(f"schedule {got} != {want}")

def check_role(role):
    if role not in ("leader", "follower"):
        raise ValueError(role)

def test_cluster():
    check_schedule(compute(), [1, 2])
    check_role(current_role())
    assert ready()
"#;
        let facts = PythonPack
            .extract("tests/test_cluster.py", src, &AssertVocabulary::default())
            .unwrap();
        assert_eq!(facts.tests.len(), 1);
        let t = &facts.tests[0];
        assert_eq!(t.name, "test_cluster");
        assert_eq!(t.total_asserts, 3, "two raising helpers + one assert");
        assert!(!t.is_vacuous());
    }

    #[test]
    fn helpers_in_a_dispatch_table_are_resolved() {
        // expanse #1028: `self_test()` runs a table of `(label, helper)` pairs.
        let src = "def check_blocks():\n    if blocks() != 3:\n        raise ValueError(\"blocks\")\n\ndef check_rounds():\n    assert rounds() == 8\n\ndef self_test():\n    steps = [\n        (\"blocks\", check_blocks),\n        (\"rounds\", check_rounds),\n    ]\n    for label, fn in steps:\n        fn()\n    return 0\n";
        let vocab = AssertVocabulary {
            test_functions: vec!["self_test".into()],
            ..Default::default()
        };
        let facts = PythonPack.extract("scripts/s.py", src, &vocab).unwrap();
        let t = &facts.tests[0];
        assert_eq!((t.total_asserts, t.helper_checks), (2, 2), "{t:?}");
    }

    #[test]
    fn a_declared_private_function_is_a_test_and_its_handlers_are_test_code() {
        // A `_self_test` the repository declares is its test entry point: its body is test
        // scope, so an expect-to-raise handler there is not error swallowing. An
        // undeclared `_private` function is still not a test.
        let src = "def _self_test():\n    try:\n        outside((3.0, 2.0))\n        check(\"refused\", False)\n    except ValueError:\n        pass\n\ndef _helper():\n    try:\n        g()\n    except ValueError:\n        pass\n";
        let vocab = AssertVocabulary {
            test_functions: vec!["_self_test".into()],
            ..Default::default()
        };
        let facts = PythonPack.extract("scripts/gate.py", src, &vocab).unwrap();
        let names: Vec<&str> = facts.tests.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["_self_test"]);
        let lines: Vec<usize> = facts.swallowed.iter().map(|s| s.line).collect();
        assert_eq!(lines, vec![11]);
    }

    #[test]
    fn helpers_are_followed_three_calls_deep_and_cycle_safe() {
        // `outer` does not raise itself; it calls `inner`, which does: two levels, counted
        // (a validator moved out of the function a self-test drives). `four` is one level
        // past the limit. `loop_a`/`loop_b` call each other and never raise; `walk`
        // raises and calls itself: each counts once, and resolution terminates.
        let src = r#"
def inner(x):
    if not x:
        raise RuntimeError("bad")

def outer(x):
    inner(x)

def two(x):
    outer(x)

def four(x):
    two(x)

def loop_a(n):
    return loop_b(n)

def loop_b(n):
    return loop_a(n)

def walk(n):
    if n < 0:
        raise ValueError(n)
    return walk(n - 1)

def test_nested_helper():
    outer(1)

def test_three_deep():
    two(1)

def test_four_deep():
    four(1)

def test_cycle():
    loop_a(3)

def test_recursive():
    walk(3)

def test_direct():
    inner(1)
"#;
        let facts = PythonPack
            .extract("tests/test_nested.py", src, &AssertVocabulary::default())
            .unwrap();
        let by_name = |n: &str| facts.tests.iter().find(|t| t.name == n).unwrap();
        assert_eq!(by_name("test_nested_helper").total_asserts, 1);
        assert_eq!(by_name("test_three_deep").total_asserts, 1);
        assert_eq!(by_name("test_four_deep").total_asserts, 0);
        assert_eq!(by_name("test_cycle").total_asserts, 0);
        assert_eq!(by_name("test_recursive").total_asserts, 1);
        assert_eq!(by_name("test_direct").total_asserts, 1);
    }

    #[test]
    fn raise_inside_nested_def_in_helper_is_not_a_failure_path() {
        // The `raise` lives in a closure the helper returns but never calls.
        let src = r#"
def make_checker():
    def check(x):
        raise ValueError(x)
    return check

def test_factory():
    make_checker()
"#;
        let facts = PythonPack
            .extract("tests/test_factory.py", src, &AssertVocabulary::default())
            .unwrap();
        assert_eq!(facts.tests[0].total_asserts, 0);
    }

    #[test]
    fn self_method_helper_resolves_within_the_class() {
        let src = r#"
import unittest

class TestOrders(unittest.TestCase):
    def _expect_total(self, order, total):
        if order.total != total:
            raise AssertionError(order.total)

    def test_total(self):
        self._expect_total(make_order(), 3)
"#;
        let facts = PythonPack
            .extract("tests/test_orders.py", src, &AssertVocabulary::default())
            .unwrap();
        assert_eq!(facts.tests.len(), 1);
        assert_eq!(facts.tests[0].total_asserts, 1);
    }

    #[test]
    fn plain_self_test_function_is_not_collected() {
        // Neither pytest nor unittest collects `self_test`: it is a script entry
        // point, not a test.
        let src = r#"
def self_test() -> int:
    assert parse("a") == "a"
    return 0

if __name__ == "__main__":
    raise SystemExit(self_test())
"#;
        let facts = PythonPack
            .extract("scripts/check_thing.py", src, &AssertVocabulary::default())
            .unwrap();
        assert!(
            facts.tests.is_empty(),
            "collected: {:?}",
            facts.tests.iter().map(|t| &t.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn pytest_and_unittest_collection_rules() {
        let src = r#"
import unittest

def test_module_level():
    assert 1 + 1 == 2

class TestPytestStyle:
    def test_method(self):
        assert 2 * 2 == 4

    def testNoUnderscore(self):
        assert 3 == 3 + 0

class OrderTests(unittest.TestCase):
    def testCamel(self):
        self.assertEqual(f(), 1)

    def test_snake(self):
        self.assertEqual(f(), 1)

class Helper:
    def test_looking_method(self):
        assert g() == 1

class Builder(object):
    def test_connection(self):
        return True
"#;
        // Outside a test path, so the class rule applies without the mixin
        // allowance: `Helper` and `Builder` are not test classes.
        let facts = PythonPack
            .extract("pkg/rules.py", src, &AssertVocabulary::default())
            .unwrap();
        let mut names: Vec<&str> = facts.tests.iter().map(|t| t.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec![
                "OrderTests::testCamel",
                "OrderTests::test_snake",
                "TestPytestStyle::testNoUnderscore",
                "TestPytestStyle::test_method",
                "test_module_level",
            ]
        );
    }

    #[test]
    fn indirect_testcase_subclass_in_same_file_is_collected() {
        let src = r#"
import unittest

class BaseCase(unittest.TestCase):
    pass

class Derived(BaseCase):
    def test_derived(self):
        self.assertEqual(h(), 2)
"#;
        let facts = PythonPack
            .extract("tests/test_derived.py", src, &AssertVocabulary::default())
            .unwrap();
        assert_eq!(facts.tests.len(), 1);
        assert_eq!(facts.tests[0].name, "Derived::test_derived");
    }

    #[test]
    fn mixin_test_methods_stay_collected() {
        // Same file: `ContractMixin` is inherited by a TestCase subclass, and
        // its `test_*` methods run through it, even outside a test path.
        let same_file = r#"
import unittest

class ContractMixin:
    def test_status(self):
        self.assertEqual(self.get().status, 200)

class ApiTests(ContractMixin, unittest.TestCase):
    def get(self):
        return fetch()

class Loop1(Loop2):
    def test_a(self):
        assert x() == 1

class Loop2(Loop1):
    pass
"#;
        let facts = PythonPack
            .extract("pkg/api_checks.py", same_file, &AssertVocabulary::default())
            .unwrap();
        let names: Vec<&str> = facts.tests.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["ContractMixin::test_status"]);

        // Test path: the subclass may live in another file, so the mixin's
        // tests are kept rather than silently dropped.
        let cross_file = r#"
class ContractMixin:
    def test_status(self):
        self.assertEqual(self.get().status, 200)
"#;
        let facts = PythonPack
            .extract("tests/mixins.py", cross_file, &AssertVocabulary::default())
            .unwrap();
        assert_eq!(facts.tests.len(), 1);
        assert_eq!(facts.tests[0].name, "ContractMixin::test_status");
    }
}
