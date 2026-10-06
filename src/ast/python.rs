//! Python language pack: tree-sitter AST extraction of tests, assertions, and escape hatches.

use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser};

use std::collections::{HashMap, HashSet};

use super::ci_condition::{CiVerdict, SkipCondition};
use super::functions::{self, FunctionSpec};
use super::{
    AssertVocabulary, EscapeHatchSite, Fact, HelperFacts, LanguagePack, ParsedFileFacts, TestFn,
    TestHelperFacts,
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

    fn is_test_path(&self, path: &str) -> bool {
        super::functions::is_test_file(path, Some(is_python_test_path))
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
            unittest_stack: Vec::new(),
            inherited_cases: Vec::new(),
            inherited_skips: Vec::new(),
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
            // A file matching the shared test-path conventions, this pack's own test-file
            // convention, or one the repository declares as test scope, is test code
            // line for line.
            let whole_file = super::functions::is_test_file(path, Some(is_python_test_path))
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
        super::method_checks::count(root, src, &mut extractor.facts, &PYTHON_RECEIVER_CALLS);
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
        super::expectations::python(root, src, &mut extractor.facts.tests);
        super::caught_assertions::python(root, src, &mut extractor.facts.tests);
        super::expected_exceptions::python(root, src, &mut extractor.facts.tests);
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
    /// For each enclosing class, whether it derives from a `TestCase`: unittest names
    /// the methods of such a class by its own `test` prefix, whatever pytest's
    /// `python_functions` says.
    unittest_stack: Vec<bool>,
    /// The case counts the enclosing module and classes give each of their tests: a
    /// `parametrize` decorator on a class, a `pytestmark` in a class body or the module.
    inherited_cases: Vec<super::test_cases::CaseList>,
    /// Conditional skips of the enclosing module and classes (`pytestmark =
    /// pytest.mark.skipif(..)`, a `skipif` decorator on a class), outermost first.
    inherited_skips: Vec<Vec<(String, CiVerdict)>>,
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

/// The skip marks a decorator list or a `pytestmark` statement carries.
#[derive(Default)]
struct SkipMarks {
    /// An unconditional skip, or one whose condition is always true.
    ignored: bool,
    /// Skips under a condition: the condition as reported and what a CI variable decides.
    conditional: Vec<(String, CiVerdict)>,
}

/// The content of a plain string literal node: no prefix, no interpolation, no escape.
fn python_string_content(n: Node, src: &[u8]) -> Option<String> {
    if n.kind() != "string" {
        return None;
    }
    let mut cursor = n.walk();
    let mut content = String::new();
    for child in n.named_children(&mut cursor) {
        match child.kind() {
            "string_start" => {
                let start = child.utf8_text(src).ok()?;
                if start.chars().any(|c| c.is_ascii_alphabetic()) {
                    return None;
                }
            }
            "string_content" => {
                let mut inner = child.walk();
                if child.named_children(&mut inner).next().is_some() {
                    return None;
                }
                content.push_str(child.utf8_text(src).ok()?);
            }
            "string_end" => {}
            _ => return None,
        }
    }
    Some(content)
}

/// What a pytest string condition (`skipif("sys.platform == 'win32'")`) does, from its
/// own syntax tree. Names it uses are not bound in the test file's scope alone (pytest
/// adds `os`, `sys`, `platform` and `config`), so only what the string itself reads
/// counts.
fn python_string_condition(code: &str) -> SkipCondition {
    use super::ci_condition::{self, Lang};
    let undecided = SkipCondition::When(CiVerdict::NotCi);
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .is_err()
    {
        return undecided;
    }
    let Some(tree) = parser.parse(code, None) else {
        return undecided;
    };
    let root = tree.root_node();
    if root.has_error() || root.named_child_count() != 1 {
        return undecided;
    }
    let expression = root
        .named_child(0)
        .filter(|s| s.kind() == "expression_statement" && s.named_child_count() == 1)
        .and_then(|s| s.named_child(0));
    match expression {
        Some(expression) => {
            ci_condition::skip_condition(Lang::Python, expression, code.as_bytes(), false)
        }
        None => undecided,
    }
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
        // pytest's `python_classes`, when the repository's configuration sets it.
        if self
            .vocab
            .runner_rules
            .pytest
            .class_name_override(name)
            .unwrap_or_else(|| name.starts_with("Test"))
        {
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

    /// Whether class `name` derives from a `TestCase`, through classes defined in this
    /// file or by the name of a base defined elsewhere (as in
    /// [`Self::class_is_test_case`]).
    fn class_inherits_test_case<'n>(
        &'n self,
        name: &'n str,
        visited: &mut HashSet<&'n str>,
    ) -> bool {
        if !visited.insert(name) {
            return false;
        }
        let Some(bases) = self.class_bases.get(name) else {
            return false;
        };
        bases.iter().any(|base| {
            if self.class_bases.contains_key(base.as_str()) {
                self.class_inherits_test_case(base, visited)
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
        let mut marks = SkipMarks::default();
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            self.pytestmark_skips(child, &mut marks);
        }
        let mut scope = Vec::new();
        self.inherited_cases
            .push(super::test_cases::extract_python_pytestmark_cases(
                root, self.src,
            ));
        self.inherited_skips.push(marks.conditional);
        self.visit_node(root, &mut scope, marks.ignored);
        self.inherited_skips.pop();
        self.inherited_cases.pop();
    }

    /// The skip marks of one decorator. A decorator the text rule
    /// ([`Self::is_skip_decorator`]) accepts is an unconditional skip unless its syntax
    /// tree shows a skip that takes a condition.
    fn decorator_skips(&self, dec: Node, marks: &mut SkipMarks) {
        if !self.is_skip_decorator(dec) {
            return;
        }
        let Some(expr) = dec.named_child(0) else {
            marks.ignored = true;
            return;
        };
        if self.condition_skip(expr, marks) {
            return;
        }
        // `@pytest.mark.parametrize(.., [pytest.param(.., marks=pytest.mark.skipif(c))])`:
        // a case skipped under a condition is read as a conditional skip of the test.
        let before = marks.conditional.len();
        let (mut conditional, mut unconditional) = (false, false);
        if expr.kind() == "call" && self.callee_name(expr) == Some("parametrize") {
            let mut stack = vec![expr];
            while let Some(n) = stack.pop() {
                if n.id() != expr.id() && self.condition_skip(n, marks) {
                    conditional = true;
                    continue;
                }
                if matches!(n.kind(), "attribute" | "identifier")
                    && matches!(self.last_name(n), Some("skip" | "xfail"))
                {
                    unconditional = true;
                }
                let mut cursor = n.walk();
                stack.extend(n.named_children(&mut cursor));
            }
        }
        if unconditional || !conditional {
            marks.conditional.truncate(before);
            marks.ignored = true;
        }
    }

    /// The skip marks of a `pytestmark = ..` statement of a module or class body.
    fn pytestmark_skips(&self, statement: Node, marks: &mut SkipMarks) {
        if !statement_has_skip_mark(self.text(statement)) {
            return;
        }
        let assignment = statement
            .named_child(0)
            .filter(|a| statement.kind() == "expression_statement" && a.kind() == "assignment");
        let Some(value) = assignment.and_then(|a| a.child_by_field_name("right")) else {
            marks.ignored = true;
            return;
        };
        let elements: Vec<Node> = if matches!(value.kind(), "list" | "tuple") {
            let mut cursor = value.walk();
            value
                .named_children(&mut cursor)
                .filter(|c| c.kind() != "comment")
                .collect()
        } else {
            vec![value]
        };
        for element in elements {
            let text = self.text(element);
            if !(text.contains("skip") || text.contains("xfail")) {
                continue;
            }
            if !self.condition_skip(element, marks) {
                marks.ignored = true;
            }
        }
    }

    /// The last name of a dotted reference: `skipif` in `pytest.mark.skipif`.
    fn last_name(&self, n: Node) -> Option<&'a str> {
        match n.kind() {
            "identifier" => Some(self.text(n)),
            "attribute" => n.child_by_field_name("attribute").map(|a| self.text(a)),
            _ => None,
        }
    }

    fn callee_name(&self, call: Node) -> Option<&'a str> {
        call.child_by_field_name("function")
            .and_then(|f| self.last_name(f))
    }

    /// Reads `expr` as a skip that takes a condition: `pytest.mark.skipif(<condition>)`,
    /// `unittest.skipIf(<condition>, ..)`, `unittest.skipUnless(<condition>, ..)`. Returns
    /// whether it is one, with what it does added to `marks`.
    fn condition_skip(&self, expr: Node, marks: &mut SkipMarks) -> bool {
        use super::ci_condition::{self, Lang};
        if expr.kind() != "call" {
            return false;
        }
        let Some(name @ ("skipif" | "skipIf" | "skipUnless")) = self.callee_name(expr) else {
            return false;
        };
        let negated = name == "skipUnless";
        let condition = expr.child_by_field_name("arguments").and_then(|args| {
            let mut cursor = args.walk();
            let children: Vec<Node> = args.named_children(&mut cursor).collect();
            let positional = children
                .iter()
                .copied()
                .find(|c| !matches!(c.kind(), "keyword_argument" | "comment"));
            positional.or_else(|| {
                children.iter().find_map(|c| {
                    let is_condition = c.kind() == "keyword_argument"
                        && c.child_by_field_name("name").map(|n| self.text(n)) == Some("condition");
                    is_condition
                        .then(|| c.child_by_field_name("value"))
                        .flatten()
                })
            })
        });
        let Some(condition) = condition else {
            // No condition to read: `pytest.mark.skipif()` skips.
            marks.ignored = true;
            return true;
        };
        let text = self.text(condition).trim();
        let outcome = match (name, python_string_content(condition, self.src)) {
            // pytest evaluates a string condition as an expression when the test is
            // collected. It is parsed the same way here; one that does not parse as a
            // single expression is a condition on no CI variable.
            ("skipif", Some(code)) => python_string_condition(&code),
            // To unittest a string is a value: skipped when it is not empty.
            (_, Some(code)) => {
                if code.is_empty() == negated {
                    SkipCondition::Always
                } else {
                    SkipCondition::Never
                }
            }
            (_, None) => ci_condition::skip_condition(Lang::Python, condition, self.src, negated),
        };
        match outcome {
            SkipCondition::Always => marks.ignored = true,
            SkipCondition::Never => {}
            SkipCondition::When(verdict) => marks.conditional.push((
                if negated {
                    format!("not ({text})")
                } else {
                    text.to_string()
                },
                verdict,
            )),
        }
        true
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

        let mut marks = SkipMarks::default();
        if class_has_skip_mark {
            if let Some(body) = node.child_by_field_name("body") {
                let mut cursor = body.walk();
                for statement in body.children(&mut cursor) {
                    self.pytestmark_skips(statement, &mut marks);
                }
            }
        }
        for dec in class_decorators.unwrap_or_default() {
            self.decorator_skips(*dec, &mut marks);
        }
        let class_is_ignored = parent_ignored || marks.ignored;
        self.inherited_skips.push(marks.conditional);

        let collected = self.collected_classes.contains(&class_name);
        let unittest = self.class_inherits_test_case(&class_name, &mut HashSet::new());
        scope.push(class_name);
        self.class_stack.push(collected);
        self.unittest_stack.push(unittest);
        let body_cases = node
            .child_by_field_name("body")
            .map(|body| super::test_cases::extract_python_pytestmark_cases(body, self.src))
            .unwrap_or_default();
        self.inherited_cases.push(super::test_cases::multiply_cases(
            super::test_cases::extract_python_cases(class_decorators, self.src),
            body_cases,
        ));

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

        self.inherited_skips.pop();
        self.inherited_cases.pop();
        self.class_stack.pop();
        self.unittest_stack.pop();
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
        let mut marks = SkipMarks::default();
        for dec in decorators.unwrap_or_default() {
            self.decorator_skips(*dec, &mut marks);
        }
        let ignored = parent_ignored || marks.ignored;

        let line = node.start_position().row + 1;
        let end_line = node.end_position().row + 1;
        let (cases, non_literal_cases, case_rows) = self
            .inherited_cases
            .iter()
            .fold(
                super::test_cases::extract_python_cases(decorators, self.src),
                |own, outer| super::test_cases::multiply_cases(outer.clone(), own),
            )
            .into_parts();

        let mut test_fn = TestFn {
            name: full_name,
            line,
            end_line,
            total_asserts: 0,
            strong_asserts: 0,
            tautologies: 0,
            ignored,
            should_panic: None,
            cases,
            non_literal_cases,
            case_rows,
            ..Default::default()
        };

        let mut calls = Vec::new();
        if let Some(body) = node.child_by_field_name("body") {
            self.scan_test_body(body, &mut test_fn, BodyMode::Test);
            self.collect_calls(body, scope, &mut calls);
            if !test_fn.ignored {
                self.record_conditional_early_exits(body, &mut test_fn);
            }
        }
        if !test_fn.ignored {
            let inherited = self.inherited_skips.iter().flatten().cloned();
            for (text, verdict) in inherited.chain(marks.conditional) {
                test_fn.record_conditional_skip(text, verdict);
            }
        }

        self.facts.tests.push(test_fn);
        self.test_calls.push(calls);
    }

    /// Early exits under a condition: the first `if` the text rule below accepts, then
    /// every `return` under an `if` (nested and `else` branches included) that a CI
    /// variable is involved in, through a variable, constant or helper of this file.
    fn record_conditional_early_exits(&self, body: Node, test: &mut TestFn) {
        use super::ci_condition::{self, CiVerdict, Lang};
        if let Some((cond, consequence)) = self.detect_python_conditional_early_exit(body) {
            let verdict = ci_condition::site(Lang::Python, consequence, self.src)
                .map_or(CiVerdict::NotCi, |s| s.verdict);
            test.record_conditional_skip(cond, verdict);
        }
        for exit in ci_condition::exits_under_if(body, &|n| n.kind() == "return_statement") {
            if let Some(site) = ci_condition::site(Lang::Python, exit, self.src) {
                if site.related {
                    test.record_conditional_skip(site.text, site.verdict);
                }
            }
        }
    }

    fn detect_python_conditional_early_exit<'t>(
        &self,
        body: Node<'t>,
    ) -> Option<(String, Node<'t>)> {
        let mut cursor = body.walk();
        let mut env_bindings = std::collections::HashSet::new();

        for child in body.children(&mut cursor) {
            if child.kind() == "expression_statement" {
                if let Some(assign) = child.child(0) {
                    if assign.kind() == "assignment" {
                        let text = self.text(assign);
                        if is_python_env_check(text) {
                            if let Some(left) = assign.child_by_field_name("left") {
                                let name = self.text(left).trim();
                                if !name.is_empty() {
                                    env_bindings.insert(name.to_string());
                                }
                            }
                        }
                    }
                }
            }

            if child.kind() == "if_statement" {
                let cond_node = child.child_by_field_name("condition")?;
                let cond_text = self.text(cond_node).trim();

                let is_env_check = is_python_env_check(cond_text)
                    || env_bindings.iter().any(|v| {
                        cond_text == v
                            || cond_text
                                .split(|c: char| !c.is_alphanumeric() && c != '_')
                                .any(|t| t == v)
                    });

                let consequence = child.child_by_field_name("consequence")?;
                let calls_skip = python_block_calls_skip(consequence, self.src);

                if calls_skip {
                    return Some((cond_text.to_string(), consequence));
                }

                if is_env_check && python_block_returns_early(consequence) {
                    return Some((cond_text.to_string(), consequence));
                }
            }
        }
        None
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
        // pytest's `python_functions`, when the repository's configuration sets it,
        // names the test functions and the test methods of a class that is not a
        // `TestCase`. Unset, the defaults below apply.
        let configured = self
            .vocab
            .runner_rules
            .pytest
            .function_name_override(fn_name);
        match self.class_stack.last() {
            Some(&collected) => {
                let unittest = self.unittest_stack.last().copied().unwrap_or(false);
                let named = match configured {
                    Some(named) if !unittest => named,
                    _ => fn_name.starts_with("test"),
                };
                (collected || self.is_test_path) && named
            }
            None => configured.unwrap_or_else(|| {
                fn_name.starts_with("test_")
                    || fn_name == "test"
                    || (self.is_test_path && fn_name.starts_with("test"))
            }),
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
        let wraps = node.child_by_field_name("body").and_then(|b| {
            super::forwarding_wrapper_callee(b, &PY_WRAPPER, &PY_LOCALS, &calls, self.src)
        });
        self.helper_calls
            .entry(key.clone())
            .or_insert_with(|| calls.clone());
        self.helpers.entry(key.clone()).or_insert(HelperFacts {
            total_asserts: facts.total_asserts,
            strong_asserts: facts.strong_asserts,
            tautologies: facts.tautologies,
            fatal_asserts: facts.fatal_asserts,
            wraps,
        });
        let line = node.start_position().row + 1;
        let end_line = node.end_position().row + 1;
        self.facts.push_helper(
            TestHelperFacts {
                name: key,
                line,
                end_line,
                total_asserts: facts.total_asserts,
                strong_asserts: facts.strong_asserts,
                tautologies: facts.tautologies,
                fatal_asserts: facts.fatal_asserts,
                helper_checks: 0,
            },
            calls,
        );
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
            super::resolve_test_same_file_helpers(test, calls, self.vocab, |call| {
                let mut path = Vec::new();
                super::transitive_helper(call, helpers, helper_calls, &mut path)
            });
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

    /// Records where the tautologies counted under `node` are (`TestFn::mark_tautologies`).
    fn visit_body_node(&self, node: Node, test: &mut TestFn, mode: BodyMode) {
        let mark = test.tautology_mark();
        self.visit_body_node_unmarked(node, test, mode);
        test.mark_tautologies(mark, node);
    }

    fn visit_body_node_unmarked(&self, node: Node, test: &mut TestFn, mode: BodyMode) {
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
                        let legacy = enclosing_python_if_condition(node, self.src);
                        let constant = matches!(legacy.as_deref(), Some("True" | "1"));
                        let site = super::ci_condition::site(
                            super::ci_condition::Lang::Python,
                            node,
                            self.src,
                        );
                        match super::ci_condition::conditional(legacy, site) {
                            Some((cond, verdict)) if !constant => {
                                test.record_conditional_skip(cond, verdict);
                            }
                            _ => test.ignored = true,
                        }
                    } else if self
                        .vocab
                        .helper_fns
                        .iter()
                        .any(|h| super::helper_call_matches(func_name, h))
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

fn is_python_env_check(text: &str) -> bool {
    text.contains("os.environ")
        || text.contains("os.getenv")
        || text.contains("environ.get")
        || super::is_ci_condition(text)
}

fn python_block_returns_early(consequence: Node) -> bool {
    let mut cursor = consequence.walk();
    for child in consequence.children(&mut cursor) {
        if child.kind() == "return_statement" {
            return true;
        }
    }
    false
}

fn python_block_calls_skip(consequence: Node, src: &[u8]) -> bool {
    let mut cursor = consequence.walk();
    for child in consequence.children(&mut cursor) {
        if child.kind() == "expression_statement" {
            if let Some(call) = child.child(0) {
                if call.kind() == "call" {
                    if let Some(func) = call.child_by_field_name("function") {
                        if let Ok(name) = func.utf8_text(src) {
                            if name == "pytest.skip"
                                || name == "pytest.xfail"
                                || name == "self.skipTest"
                            {
                                return true;
                            }
                        }
                    }
                }
            }
        }
    }
    false
}

fn enclosing_python_if_condition(node: Node, src: &[u8]) -> Option<String> {
    let mut cur = node;
    while let Some(p) = cur.parent() {
        match p.kind() {
            "function_definition" | "lambda" => return None,
            "if_statement" => {
                let cond = p.child_by_field_name("condition")?;
                let in_body = p
                    .child_by_field_name("consequence")
                    .is_some_and(|c| c.id() == cur.id());
                if in_body {
                    return cond.utf8_text(src).ok().map(|t| t.trim().to_string());
                }
            }
            _ => {}
        }
        cur = p;
    }
    None
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
    functions::is_test_file(path, Some(is_python_test_path)) || name.starts_with("test_")
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

/// A method called on a receiver (`method_checks`).
pub const PYTHON_RECEIVER_CALLS: super::method_checks::ReceiverCalls =
    super::method_checks::ReceiverCalls {
        member: &[("call", "function", "attribute", "attribute")],
        direct: &[],
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
    trivial: &[
        "pass",
        "...",
        "return",
        "return None",
        "return False",
        "return 0",
        "return \"\"",
        "return ''",
        "return []",
        "return {}",
        "return ()",
        "continue",
    ],
    discard_kinds: &[],
    discards: super::handlers::no_discard,
    classify_discard: None,
    call_value_kinds: &[],
    silence_kinds: &[],
    silences: super::handlers::no_discard,
    silence_node: None,
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

/// A local a Python wrapper computes and forwards: `b = loc(d)`.
pub const PY_LOCALS: super::LocalSpec = super::LocalSpec {
    statements: &["expression_statement"],
    binders: &["assignment"],
    pattern: &["left"],
    value: &["right"],
    names: &["identifier"],
    holders: &[],
    refused: &[],
};

/// A Python helper whose body is one call: `return check(x, True)`, `self.check(x)`.
pub const PY_WRAPPER: super::WrapperSpec = super::WrapperSpec {
    through: &["block", "expression_statement", "return_statement", "await"],
    calls: &["call"],
    arguments: &[
        "argument_list",
        "keyword_argument",
        "list",
        "tuple",
        "set",
        "dictionary",
        "pair",
    ],
    references: &[],
    plain: &["true", "false", "none", "integer", "float", "string"],
    skip: &["comment"],
};

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole-file handler rule is the shared-or-own superset: an empty handler is
    /// silent in a file only the shared rule recognises (`benches/`) and in one only
    /// the pack's own `test_` prefix recognises, and fires elsewhere.
    #[test]
    fn whole_file_handler_rule_is_the_shared_or_own_superset() {
        let src = "def load(p):\n    try:\n        return open(p).read()\n    except OSError:\n        pass\n";
        let swallowed = |path: &str| {
            PythonPack
                .extract(path, src, &AssertVocabulary::default())
                .unwrap()
                .swallowed
                .len()
        };
        assert_eq!(swallowed("benches/helper.py"), 0, "shared rule only");
        assert_eq!(swallowed("src/test_helper.py"), 0, "own rule only");
        assert_eq!(swallowed("src/helper.py"), 1, "negative control");
    }

    /// A test calling a thin wrapper gets the credit of one calling the wrapped function
    /// (`crate::ast::thin_wrapper_counts` names the controls).
    #[test]
    fn a_thin_wrapper_resolves_to_the_function_it_wraps() {
        let src = r##"def checked(x, strict):
    if strict and x != 1:
        raise ValueError("x")

def noop(x, n):
    pass

def via(x):
    return checked(x, strict=True)

def hollow(x):
    noop(x, 1)

def busy(x):
    checked(x, True)
    prepare(x)

def ping(x):
    return pong(x, 1)

def pong(x, n):
    return ping(x)

def test_direct():
    checked(1, True)

def test_via_wrapper():
    via(1)

def test_hollow_wrapper():
    hollow(1)

def test_busy_helper():
    busy(1)

def test_wrapper_cycle():
    ping(1)
"##;
        assert_eq!(
            crate::ast::thin_wrapper_counts(&PythonPack, "tests/test_wrap.py", src),
            crate::ast::TRANSITIVE_WRAPPER_COUNTS
        );
        // `outer -> w1 -> w2 -> inner -> checked`: `outer` and `inner` spend two of the
        // three levels and the wrapper hops cost none, so `inner` still reaches the check.
        let deep = PythonPack
            .extract("tests/test_deep.py", "def checked(x, strict):\n    if strict and x != 1:\n        raise ValueError(x)\n\ndef inner(x):\n    setup()\n    checked(x, True)\n\ndef w2(x):\n    inner(x)\n\ndef w1(x):\n    return w2(x)\n\ndef outer(x):\n    setup()\n    w1(x)\n\ndef test_deep():\n    outer(1)\n", &AssertVocabulary::default())
            .unwrap();
        let t = &deep.tests[0];
        assert_eq!((t.total_asserts, t.helper_checks), (1, 1), "{t:?}");
    }

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
        // raises and calls itself: each counts once, and resolution terminates. The chain's
        // helpers also call `log`, so none is a thin wrapper followed for free.
        let src = r#"
def inner(x):
    if not x:
        raise RuntimeError("bad")

def outer(x):
    log(x)
    inner(x)

def two(x):
    log(x)
    outer(x)

def four(x):
    log(x)
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

    #[test]
    fn early_exit_in_python_test_under_env_or_ci_check_detected() {
        let src = r#"
import os
import pytest

def test_ci_return():
    if os.getenv("CI"):
        return
    assert 1 == 1

def test_ci_skip():
    if os.environ.get("GITHUB_ACTIONS"):
        pytest.skip("skipping in github actions")
    assert 1 == 1

def test_generic_env_return():
    if "SKIP_SLOW" in os.environ:
        return None
    assert 1 == 1

def test_guard_without_exit():
    if os.getenv("CI"):
        print("in CI")
    assert 1 == 1

def helper_guard():
    if os.getenv("CI"):
        return
"#;
        let pack = PythonPack;
        let facts = pack
            .extract("tests/test_env_check.py", src, &AssertVocabulary::default())
            .unwrap();

        let ci_return = facts
            .tests
            .iter()
            .find(|t| t.name == "test_ci_return")
            .unwrap();
        assert!(!ci_return.ignored);
        assert_eq!(
            ci_return.conditional_ignore.as_deref(),
            Some("os.getenv(\"CI\")")
        );

        let ci_skip = facts
            .tests
            .iter()
            .find(|t| t.name == "test_ci_skip")
            .unwrap();
        assert!(!ci_skip.ignored);
        assert_eq!(
            ci_skip.conditional_ignore.as_deref(),
            Some("os.environ.get(\"GITHUB_ACTIONS\")")
        );

        let generic_test = facts
            .tests
            .iter()
            .find(|t| t.name == "test_generic_env_return")
            .unwrap();
        assert!(!generic_test.ignored);
        assert_eq!(
            generic_test.conditional_ignore.as_deref(),
            Some("\"SKIP_SLOW\" in os.environ")
        );

        let no_exit_test = facts
            .tests
            .iter()
            .find(|t| t.name == "test_guard_without_exit")
            .unwrap();
        assert!(!no_exit_test.ignored);
        assert_eq!(no_exit_test.conditional_ignore, None);

        assert!(facts.tests.iter().all(|t| t.name != "helper_guard"));
    }

    const PYTEST_NAMED_SOURCE: &str = "\
import unittest


def test_default():
    assert a() == 1


def check_custom():
    assert b() == 2


class TestDefault:
    def test_m(self):
        assert c() == 3

    def check_m(self):
        assert d() == 4


class SuiteCustom:
    def check_n(self):
        assert e() == 5

    def test_n(self):
        assert f() == 6


class LegacyCase(unittest.TestCase):
    def test_u(self):
        self.assertEqual(g(), 7)

    def check_u(self):
        self.assertEqual(h(), 8)
";

    fn named_tests(config: &str) -> Vec<String> {
        let mut vocab = AssertVocabulary::default();
        vocab.runner_rules.pytest =
            crate::ast::runner_collection::PytestCollectionRules::parse_ini(config);
        let facts = PythonPack
            .extract("checks/spec_names.py", PYTEST_NAMED_SOURCE, &vocab)
            .unwrap();
        let mut names: Vec<String> = facts.tests.into_iter().map(|t| t.name).collect();
        names.sort();
        names
    }

    /// pytest `python_functions` / `python_classes` (#593): set, they replace the
    /// default names for functions and for the methods of a class that is not a
    /// `TestCase`; a `TestCase` keeps unittest's `test` prefix.
    #[test]
    fn pytest_python_functions_and_classes_name_the_tests() {
        // Unset: the defaults.
        assert_eq!(
            named_tests("[pytest]\n"),
            ["LegacyCase::test_u", "TestDefault::test_m", "test_default"]
        );
        assert_eq!(
            named_tests("[pytest]\npython_functions = check_*\n"),
            ["LegacyCase::test_u", "TestDefault::check_m", "check_custom"]
        );
        assert_eq!(
            named_tests("[pytest]\npython_classes = Suite*\n"),
            ["LegacyCase::test_u", "SuiteCustom::test_n", "test_default"]
        );
        assert_eq!(
            named_tests(
                "[pytest]\npython_functions = check_* test_*\npython_classes = Suite Test\n"
            ),
            [
                "LegacyCase::test_u",
                "SuiteCustom::check_n",
                "SuiteCustom::test_n",
                "TestDefault::check_m",
                "TestDefault::test_m",
                "check_custom",
                "test_default"
            ]
        );
        // A section that is not a pytest configuration sets nothing.
        assert_eq!(
            named_tests("[flake8]\npython_functions = check_*\n"),
            ["LegacyCase::test_u", "TestDefault::test_m", "test_default"]
        );
    }
}

/// `skipif` / `skipIf` / `skipUnless` on a test, a class, a module and a case (#597).
/// The sources are fixtures, kept out of the test bodies.
#[cfg(test)]
mod conditional_skip_tests {
    use super::*;

    const DECORATED: &str = r#"
import os, sys, unittest, pytest

IN_CI = bool(os.getenv("CI"))

@pytest.mark.skipif(IN_CI, reason="x")
def test_in_ci():
    assert 1 + 1 == 2

@pytest.mark.skipif(sys.platform == "win32", reason="x")
def test_platform():
    assert 1 + 1 == 2

@unittest.skipUnless(IN_CI, "x")
def test_unless_ci():
    assert 1 + 1 == 2

@pytest.mark.skipif(True, reason="x")
def test_always():
    assert 1 + 1 == 2

@pytest.mark.skipif(False, reason="x")
def test_never():
    assert 1 + 1 == 2

@pytest.mark.skipif("os.environ.get('CI')")
def test_string_ci():
    assert 1 + 1 == 2

@pytest.mark.skipif("sys.platform == 'win32'")
def test_string_platform():
    assert 1 + 1 == 2

@unittest.skipIf("always", "x")
def test_unittest_string():
    assert 1 + 1 == 2

@pytest.mark.skipif()
def test_no_condition():
    assert 1 + 1 == 2

@pytest.mark.parametrize("n", [1, pytest.param(2, marks=pytest.mark.skipif(IN_CI, reason="x"))])
def test_case_in_ci(n):
    assert n == n

@pytest.mark.parametrize("n", [1, pytest.param(2, marks=pytest.mark.skip)])
def test_case_skipped(n):
    assert n == n

@pytest.mark.parametrize("n", [pytest.param(1, marks=pytest.mark.skipif(IN_CI, reason="x")), pytest.param(2, marks=pytest.mark.skip)])
def test_case_skipped_beside_a_conditional_one(n):
    assert n == n
"#;

    const INHERITED: &str = r#"
import os, sys, pytest

pytestmark = [pytest.mark.skipif(sys.platform == "win32", reason="x"), pytest.mark.slow]

def test_module():
    assert 1 + 1 == 2

@pytest.mark.skipif(os.environ.get("CI"), reason="x")
class TestDecorated:
    def test_one(self):
        assert 1 + 1 == 2

class TestMarked:
    pytestmark = pytest.mark.skipif(os.environ.get("GITHUB_ACTIONS"), reason="x")

    def test_two(self):
        assert 1 + 1 == 2

class TestAlways:
    pytestmark = [pytest.mark.skipif(sys.platform == "win32", reason="x"), pytest.mark.skip]

    def test_three(self):
        assert 1 + 1 == 2
"#;

    /// `(name, unconditionally skipped, condition, skips in CI)` of each test.
    fn read(src: &str) -> Vec<(String, bool, Option<String>, bool)> {
        PythonPack
            .extract("tests/test_q.py", src, &AssertVocabulary::default())
            .unwrap()
            .tests
            .iter()
            .map(|t| {
                (
                    t.name.clone(),
                    t.ignored,
                    t.conditional_ignore.clone(),
                    t.is_ci_skip(),
                )
            })
            .collect()
    }

    fn row(
        name: &str,
        ignored: bool,
        cond: Option<&str>,
        ci: bool,
    ) -> (String, bool, Option<String>, bool) {
        (name.to_string(), ignored, cond.map(str::to_string), ci)
    }

    #[test]
    fn a_skip_decorator_that_takes_a_condition_is_read_by_its_condition() {
        assert_eq!(
            read(DECORATED),
            vec![
                row("test_in_ci", false, Some("IN_CI"), true),
                row(
                    "test_platform",
                    false,
                    Some("sys.platform == \"win32\""),
                    false
                ),
                row("test_unless_ci", false, Some("not (IN_CI)"), false),
                row("test_always", true, None, false),
                row("test_never", false, None, false),
                row(
                    "test_string_ci",
                    false,
                    Some("\"os.environ.get('CI')\""),
                    true
                ),
                row(
                    "test_string_platform",
                    false,
                    Some("\"sys.platform == 'win32'\""),
                    false
                ),
                row("test_unittest_string", true, None, false),
                row("test_no_condition", true, None, false),
                row("test_case_in_ci", false, Some("IN_CI"), true),
                row("test_case_skipped", true, None, false),
                row(
                    "test_case_skipped_beside_a_conditional_one",
                    true,
                    None,
                    false
                ),
            ]
        );
    }

    #[test]
    fn a_condition_on_a_module_or_class_applies_to_the_tests_under_it() {
        let platform = "sys.platform == \"win32\"";
        assert_eq!(
            read(INHERITED),
            vec![
                row("test_module", false, Some(platform), false),
                // The condition a CI variable decides replaces the module's.
                row(
                    "TestDecorated::test_one",
                    false,
                    Some("os.environ.get(\"CI\")"),
                    true
                ),
                row(
                    "TestMarked::test_two",
                    false,
                    Some("os.environ.get(\"GITHUB_ACTIONS\")"),
                    true
                ),
                row("TestAlways::test_three", true, None, false),
            ]
        );
    }

    fn decorated(module: &str, condition: &str) -> TestFn {
        let src = format!(
            "import os\nimport sys\nimport pytest\n\n{module}\n@pytest.mark.skipif({condition}, reason=\"x\")\ndef test_q():\n    assert 1 + 1 == 2\n"
        );
        let facts = PythonPack
            .extract("tests/test_q.py", &src, &AssertVocabulary::default())
            .unwrap();
        facts.tests[0].clone()
    }

    /// `(unconditional skip, conditional skip, a CI variable decides it)`
    fn verdict(test: &TestFn) -> (bool, bool, bool) {
        (
            test.ignored,
            test.conditional_ignore.is_some(),
            test.is_ci_skip(),
        )
    }

    const ALWAYS: (bool, bool, bool) = (true, false, false);
    const NEVER: (bool, bool, bool) = (false, false, false);
    const CI_SKIP: (bool, bool, bool) = (false, true, true);
    const OTHER_SKIP: (bool, bool, bool) = (false, true, false);

    const ALWAYS_TRUE: &[&str] = &[
        "True",
        "1",
        "not False",
        "(True)",
        "1 == 1",
        "\"a\" == \"a\"",
        "1 != 2",
        "True or sys.platform == \"win32\"",
        "True and 1",
        "ALWAYS",
    ];

    const ALWAYS_FALSE: &[&str] = &[
        "False",
        "0",
        "not True",
        "1 == 2",
        "\"a\" != \"a\"",
        "False and sys.platform == \"win32\"",
        "False or 0",
    ];

    /// Conditions that are not constants, although they may hold on every machine.
    const NOT_CONSTANT: &[&str] = &[
        "sys.platform == \"win32\"",
        "True and sys.platform == \"win32\"",
        "False or sys.platform == \"win32\"",
        "1 == \"1\"",
        "len(\"a\") == 1",
        "\"non-empty\"",
        "REBOUND",
        "not sys.flags.debug or sys.flags.debug",
    ];

    const CONSTANTS_MODULE: &str =
        "ALWAYS = True\nREBOUND = True\nREBOUND = sys.platform == \"win32\"\n";

    #[test]
    fn a_constant_condition_is_an_unconditional_skip_or_no_skip() {
        for condition in ALWAYS_TRUE {
            assert_eq!(
                verdict(&decorated(CONSTANTS_MODULE, condition)),
                ALWAYS,
                "{condition}"
            );
        }
        for condition in ALWAYS_FALSE {
            assert_eq!(
                verdict(&decorated(CONSTANTS_MODULE, condition)),
                NEVER,
                "{condition}"
            );
        }
        for condition in NOT_CONSTANT {
            assert_eq!(
                verdict(&decorated(CONSTANTS_MODULE, condition)),
                OTHER_SKIP,
                "{condition}"
            );
        }
    }

    const CI_READ: &str = "os.environ.get(\"CI\")";
    const NOT_CI_READ: &str = "not os.environ.get(\"CI\")";
    const TRUE_AND_CI_READ: &str = "True and os.environ.get(\"CI\")";

    #[test]
    fn a_condition_that_reads_a_ci_variable_is_read_both_ways() {
        assert_eq!(verdict(&decorated("", CI_READ)), CI_SKIP);
        assert_eq!(verdict(&decorated("", NOT_CI_READ)), OTHER_SKIP);
        // A constant beside a CI read is not a constant condition.
        assert_eq!(verdict(&decorated("", TRUE_AND_CI_READ)), CI_SKIP);
    }

    /// A name written as a CI variable is, bound to something this file does not define.
    const BOUND_ELSEWHERE: &[&str] = &[
        "CI = helpers.in_ci()\n",
        "CI = in_ci()\n",
        "CI = settings.ci_mode\n",
        "CI = bool(helpers.in_ci())\n",
        "CI = not helpers.local()\n",
    ];

    /// Controls: a literal, a read of another variable, a helper of the file that reads
    /// no CI variable, a comparison, and a name that is not written as the variable is.
    const BOUND_TO_SOMETHING_ELSE: &[(&str, &str)] = &[
        ("CI = \"no\"\n", "CI"),
        ("CI = os.environ.get(\"FLAVOR\")\n", "CI"),
        (
            "def slow():\n    return sys.platform == \"win32\"\n\nCI = slow()\n",
            "CI",
        ),
        ("CI = sys.platform == \"win32\"\n", "CI"),
        ("ci = helpers.make_client()\n", "ci"),
        ("in_ci = helpers.in_ci()\n", "in_ci"),
    ];

    #[test]
    fn a_ci_spelled_name_bound_to_a_call_the_file_does_not_define_is_undecided() {
        for binding in BOUND_ELSEWHERE {
            let test = decorated(binding, "CI");
            assert_eq!(verdict(&test), CI_SKIP, "{binding}");
            assert_eq!(test.ci_skip_vars(), vec!["CI".to_string()], "{binding}");
            // Undecided: the opposite condition is a skip in CI too.
            assert_eq!(verdict(&decorated(binding, "not CI")), CI_SKIP, "{binding}");
        }
        for (binding, name) in BOUND_TO_SOMETHING_ELSE {
            assert_eq!(verdict(&decorated(binding, name)), OTHER_SKIP, "{binding}");
        }
        // A name bound once to a constant is that constant: no skip.
        assert_eq!(verdict(&decorated("CI = False\n", "CI")), NEVER);
        // Control: a read of the variable is decided, not undecided.
        assert_eq!(
            verdict(&decorated("CI = os.environ.get(\"CI\")\n", "not CI")),
            OTHER_SKIP
        );
    }
}
