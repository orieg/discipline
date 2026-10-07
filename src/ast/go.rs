//! Go language pack: tree-sitter AST extraction of tests, assertions, and escape hatches.

use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser};

use super::functions::{self, FunctionSpec};
use super::{AssertVocabulary, EscapeHatchSite, Fact, LanguagePack, ParsedFileFacts, TestFn};

/// Go language pack implementing [`LanguagePack`].
pub struct GoPack;

impl LanguagePack for GoPack {
    fn id(&self) -> &'static str {
        "go"
    }

    fn supplies(&self, fact: Fact) -> bool {
        matches!(
            fact,
            Fact::Tests | Fact::EscapeHatches | Fact::Functions | Fact::Handlers | Fact::Prose
        )
    }

    fn name(&self) -> &'static str {
        "Go"
    }

    fn matches(&self, path: &str) -> bool {
        matches!(super::extension(path), Some("go"))
    }

    fn is_test_path(&self, path: &str) -> bool {
        super::functions::is_test_file(path, Some(is_go_test_path))
    }

    fn extract(&self, path: &str, src: &str, vocab: &AssertVocabulary) -> Result<ParsedFileFacts> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_go::LANGUAGE.into())
            .map_err(|e| anyhow!("failed to load the Go grammar: {e}"))?;
        let tree = crate::ast::source_text::parse_file(&mut parser, path, src)?;
        let root = tree.root_node();

        let mut extractor = GoExtractor {
            dead: super::reach::dead_ranges(root, src, &GO_REACH),
            src: src.as_bytes(),
            vocab,
            is_test_path: is_go_test_path(path),
            facts: ParsedFileFacts {
                has_parse_errors: root.has_error(),
                ..Default::default()
            },
            helpers: std::collections::HashMap::new(),
            test_calls: Vec::new(),
            suites: suite_types(root, src),
            suite_receiver: None,
            suite_package: suite_package(root, src),
            assertion_locals: Vec::new(),
            gocheck: Vec::new(),
            imports: super::expected_exceptions::go_imports(root, src),
        };

        extractor.collect_comments_and_escape_hatches(root);
        extractor.visit_root(root);
        extractor.resolve_same_file_helpers();
        extractor.facts.functions = functions::extract(root, src, path, &GO_FUNCTIONS);
        super::mocks::count(
            root,
            src,
            &mut extractor.facts.tests,
            &GO_MOCKS,
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
            let whole_file = super::functions::is_test_file(path, Some(is_go_test_path))
                || super::functions::declared_test_path(path, &vocab.test_paths);
            let is_test_line =
                |l: usize| whole_file || spans.iter().any(|(a, b)| *a <= l && l <= *b);
            extractor.facts.swallowed =
                super::handlers::extract(root, src, &GO_HANDLERS, &is_test_line);
        }
        super::retries::mark(root, src, &mut extractor.facts.tests, &GO_RETRIES);
        if super::functions::declared_test_path(path, &vocab.test_paths) {
            for f in &mut extractor.facts.functions {
                f.is_test = true;
            }
        }
        super::method_checks::count(root, src, &mut extractor.facts, &GO_RECEIVER_CALLS);
        super::helper_loops::count(root, src, &mut extractor.facts, &super::helper_loops::GO);
        super::calls::count(
            root,
            src,
            &mut extractor.facts.tests,
            &GO_MOCKS,
            super::calls::SLEEP_VOCAB,
            super::calls::sleeps,
        );
        super::calls::count(
            root,
            src,
            &mut extractor.facts.tests,
            &GO_MOCKS,
            super::calls::TRIVIAL_ASSERT_VOCAB,
            super::calls::trivial_asserts,
        );
        super::bounds::go(root, src, &mut extractor.facts.tests);
        super::expectations::go(root, src, &mut extractor.facts.tests);
        super::expected_exceptions::go(root, src, &mut extractor.facts.tests, &extractor.imports);
        super::caught_assertions::go(root, src, &mut extractor.facts.tests, vocab);
        extractor.facts.prose = super::prose::extract(
            root,
            src,
            &[
                "comment",
                "interpreted_string_literal",
                "raw_string_literal",
            ],
        );
        Ok(extractor.facts)
    }
}

/// Reads the case tables of the tests of `facts`, the facts of the Go file `src`, again
/// with the other files of its package in hand (`siblings`, as `(path, source)`): a
/// table or row type declared in another file of the package directory counts as one
/// declared in the test's own file does
/// ([`super::test_cases::extract_go_cases_in_package`]). A sibling that declares another
/// package (`calc_test` beside `calc`) or that the grammar cannot read is left out.
pub fn resolve_package_cases(
    facts: &mut ParsedFileFacts,
    path: &str,
    src: &str,
    siblings: &[(String, String)],
) -> Result<()> {
    use super::test_cases::{extract_go_cases_in_package, go_package_name, GoSibling};
    if facts.tests.is_empty() || siblings.is_empty() {
        return Ok(());
    }
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_go::LANGUAGE.into())
        .map_err(|e| anyhow!("failed to load the Go grammar: {e}"))?;
    let tree = crate::ast::source_text::parse_file(&mut parser, path, src)?;
    let root = tree.root_node();
    let Some(package) = go_package_name(root, src.as_bytes()) else {
        return Ok(());
    };
    let mut trees = Vec::new();
    for (sibling_path, sibling_src) in siblings {
        let Ok(tree) = crate::ast::source_text::parse_file(&mut parser, sibling_path, sibling_src)
        else {
            continue;
        };
        if go_package_name(tree.root_node(), sibling_src.as_bytes()) == Some(package) {
            trees.push((tree, sibling_src.as_bytes()));
        }
    }
    if trees.is_empty() {
        return Ok(());
    }
    let siblings: Vec<GoSibling> = trees
        .iter()
        .map(|(tree, src)| GoSibling {
            src,
            root: tree.root_node(),
        })
        .collect();
    let mut cursor = root.walk();
    for decl in root.children(&mut cursor) {
        if !matches!(decl.kind(), "function_declaration" | "method_declaration") {
            continue;
        }
        let Some(body) = decl.child_by_field_name("body") else {
            continue;
        };
        let line = decl.start_position().row + 1;
        let end_line = decl.end_position().row + 1;
        // The test recorded for this declaration; a subtest shares neither line.
        let test = facts
            .tests
            .iter_mut()
            .find(|t| t.line == line && t.end_line == end_line && !t.name.contains('/'));
        if let Some(test) = test {
            let (cases, non_literal_cases, case_rows) =
                extract_go_cases_in_package(body, src.as_bytes(), &siblings).into_parts();
            test.cases = cases;
            test.non_literal_cases = non_literal_cases;
            test.case_rows = case_rows;
        }
    }
    Ok(())
}

/// Determines whether a function name matches the Go test runner's convention (TestXxx or FuzzXxx).
pub fn is_go_test_function_name(name: &str) -> bool {
    for prefix in &["Test", "Fuzz"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            if rest.is_empty() {
                return true;
            }
            if let Some(first) = rest.chars().next() {
                if !first.is_lowercase() {
                    return true;
                }
            }
        }
    }
    false
}

/// Determines whether a path is conventionally a Go test file.
pub fn is_go_test_path(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    let filename = p.rsplit('/').next().unwrap_or(&p);
    filename.ends_with("_test.go")
        || super::functions::names_file(filename, "testutil.go")
        || super::functions::names_file(filename, "testutils.go")
        || p.starts_with("test/")
        || p.contains("/test/")
        || p.starts_with("tests/")
        || p.contains("/tests/")
        || p.starts_with("testutil/")
        || p.contains("/testutil/")
        || p.starts_with("testutils/")
        || p.contains("/testutils/")
}

struct GoExtractor<'a> {
    /// Byte ranges no execution reaches (`super::reach`).
    dead: super::reach::DeadRanges,
    src: &'a [u8],
    vocab: &'a AssertVocabulary,
    is_test_path: bool,
    facts: ParsedFileFacts,
    helpers: std::collections::HashMap<String, super::HelperFacts>,
    test_calls: Vec<Vec<String>>,
    /// The types of this file read as testify suites ([`suite_types`]).
    suites: std::collections::HashSet<String>,
    /// The receiver of the suite method being read (`s` of `func (s *Suite) TestAdd()`):
    /// an assertion called on it is counted.
    suite_receiver: Option<String>,
    /// The name the file calls testify's `suite` package by ([`suite_package`]).
    suite_package: String,
    /// Locals of the function being read that hold a testify assertion object, and
    /// whether its assertions stop the test (`read_assertion_locals`).
    assertion_locals: Vec<(String, bool)>,
    /// The gocheck `*C` parameters of the function being read ([`gocheck_parameters`]).
    gocheck: Vec<String>,
    /// The package each name of the file is bound to by an `import`.
    imports: super::expected_exceptions::GoImports,
}

/// The parameters of a function or method declared as gocheck's `*C` (`c *C`,
/// `c *check.C`, `c *gc.C`): assertions are made on them (`c.Assert(a, Equals, b)`).
fn gocheck_parameters(node: Node, src: &str) -> Vec<String> {
    let text = |n: Node| n.utf8_text(src.as_bytes()).unwrap_or("").to_string();
    let Some(params) = node.child_by_field_name("parameters") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = params.walk();
    for param in params
        .named_children(&mut cursor)
        .filter(|c| c.kind() == "parameter_declaration")
    {
        let Some(ty) = param.child_by_field_name("type") else {
            continue;
        };
        if ty.kind() != "pointer_type" {
            continue;
        }
        let named = ty.named_child(0).is_some_and(|inner| match inner.kind() {
            "type_identifier" => text(inner) == "C",
            "qualified_type" => inner
                .child_by_field_name("name")
                .is_some_and(|n| text(n) == "C"),
            _ => false,
        });
        if named {
            let mut names = param.walk();
            out.extend(
                param
                    .children_by_field_name("name", &mut names)
                    .map(text)
                    .filter(|n| n != "_"),
            );
        }
    }
    out
}

/// Whether a method has the shape gocheck runs as a test: named as a Go test, with one
/// parameter, a gocheck `*C`, and no result (`func (s *S) TestX(c *C)`).
fn is_gocheck_test(node: Node, src: &str) -> bool {
    let name = node
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(src.as_bytes()).ok())
        .unwrap_or("");
    is_go_test_function_name(name)
        && node.child_by_field_name("result").is_none()
        && node
            .child_by_field_name("parameters")
            .is_some_and(|p| p.named_child_count() == 1)
        && gocheck_parameters(node, src).len() == 1
}

/// gocheck checkers that compare the value with an expected one.
const GOCHECK_WEAK_CHECKERS: &[&str] = &["IsNil", "NotNil"];

/// The assertion methods of testify's `assert` and `require` packages, which a suite
/// carries as methods of its own (`s.Equal(..)`). Each also exists with a trailing `f`
/// (`Equalf`). `Fail`, `FailNow` and `Error` are read with the `testing.T` calls of the
/// same names.
const TESTIFY_ASSERTIONS: &[&str] = &[
    "Condition",
    "Contains",
    "DirExists",
    "ElementsMatch",
    "Empty",
    "Equal",
    "EqualError",
    "EqualExportedValues",
    "EqualValues",
    "ErrorAs",
    "ErrorContains",
    "ErrorIs",
    "Eventually",
    "EventuallyWithT",
    "Exactly",
    "False",
    "FileExists",
    "Greater",
    "GreaterOrEqual",
    "HTTPBodyContains",
    "HTTPBodyNotContains",
    "HTTPError",
    "HTTPRedirect",
    "HTTPStatusCode",
    "HTTPSuccess",
    "Implements",
    "InDelta",
    "InDeltaMapValues",
    "InDeltaSlice",
    "InEpsilon",
    "InEpsilonSlice",
    "IsDecreasing",
    "IsIncreasing",
    "IsNonDecreasing",
    "IsNonIncreasing",
    "IsNotType",
    "IsType",
    "JSONEq",
    "Len",
    "Less",
    "LessOrEqual",
    "Negative",
    "Never",
    "Nil",
    "NoDirExists",
    "NoError",
    "NoFileExists",
    "NotContains",
    "NotElementsMatch",
    "NotEmpty",
    "NotEqual",
    "NotEqualValues",
    "NotErrorAs",
    "NotErrorIs",
    "NotImplements",
    "NotNil",
    "NotPanics",
    "NotRegexp",
    "NotSame",
    "NotSubset",
    "NotZero",
    "Panics",
    "PanicsWithError",
    "PanicsWithValue",
    "Positive",
    "Regexp",
    "Same",
    "Subset",
    "True",
    "WithinDuration",
    "WithinRange",
    "YAMLEq",
    "Zero",
];

fn is_testify_assertion(method: &str) -> bool {
    TESTIFY_ASSERTIONS.contains(&method)
        || method
            .strip_suffix('f')
            .is_some_and(|plain| TESTIFY_ASSERTIONS.contains(&plain))
}

/// The receiver type of a method declaration (`Suite` of `func (s *Suite) m()`), the
/// receiver's name when it has one, and whether the method has the shape testify runs
/// as a test: named as a Go test, with no parameter and no result.
fn method_head<'t>(node: Node<'t>, src: &'t str) -> Option<(&'t str, Option<&'t str>, bool)> {
    let text = |n: Node<'t>| n.utf8_text(src.as_bytes()).unwrap_or("");
    let receiver = node.child_by_field_name("receiver")?;
    let mut cursor = receiver.walk();
    let declared = receiver
        .named_children(&mut cursor)
        .find(|c| c.kind() == "parameter_declaration")?;
    let mut ty = declared.child_by_field_name("type")?;
    if ty.kind() == "pointer_type" {
        ty = ty.named_child(0)?;
    }
    if ty.kind() == "generic_type" {
        ty = ty.child_by_field_name("type")?;
    }
    if ty.kind() != "type_identifier" {
        return None;
    }
    let name = declared.child_by_field_name("name").map(text);
    let method = node.child_by_field_name("name").map(text).unwrap_or("");
    let no_parameter = node
        .child_by_field_name("parameters")
        .is_some_and(|p| p.named_child_count() == 0);
    let shaped = is_go_test_function_name(method)
        && no_parameter
        && node.child_by_field_name("result").is_none();
    Some((text(ty), name, shaped))
}

/// The name a file calls testify's `suite` package by: the name its import gives it
/// (`import ts "github.com/stretchr/testify/suite"`), else `suite`.
fn suite_package(root: Node, src: &str) -> String {
    let mut name = None;
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "import_spec" {
            let path = node
                .child_by_field_name("path")
                .and_then(|p| p.utf8_text(src.as_bytes()).ok())
                .unwrap_or("");
            if path.trim_matches(['"', '`']).ends_with("testify/suite") {
                name = node
                    .child_by_field_name("name")
                    .and_then(|n| n.utf8_text(src.as_bytes()).ok())
                    .map(str::to_string);
            }
            continue;
        }
        let mut cursor = node.walk();
        stack.extend(node.children(&mut cursor));
    }
    name.unwrap_or_else(|| "suite".to_string())
}

/// The types of a file that are testify suites: tests are their `Test*` methods, run by
/// `suite.Run`, and assertions are methods of the suite itself.
///
/// A struct declared in the file is a suite when it embeds `suite.Suite` (or
/// `*suite.Suite`), directly or through embedded structs declared in the file. A type
/// the file cannot settle (declared in another file of the package, or embedding a type
/// declared elsewhere) is read as a suite when the file gives it a method of the shape
/// testify runs (`Test*`, no parameter, no result): nothing else calls such a method.
/// A struct of the file that embeds nothing from outside it and no suite is not one.
fn suite_types(root: Node, src: &str) -> std::collections::HashSet<String> {
    use std::collections::{HashMap, HashSet};
    let text = |n: Node| n.utf8_text(src.as_bytes()).unwrap_or("").to_string();
    // Every type the file declares, with the types a struct embeds.
    let mut declared: HashMap<String, Vec<String>> = HashMap::new();
    let mut shaped: HashSet<String> = HashSet::new();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() == "method_declaration" {
            if let Some((ty, _, true)) = method_head(child, src) {
                shaped.insert(ty.to_string());
            }
            continue;
        }
        if child.kind() != "type_declaration" {
            continue;
        }
        let mut specs = child.walk();
        for spec in child.named_children(&mut specs) {
            let Some(name) = spec.child_by_field_name("name") else {
                continue;
            };
            let mut embedded = Vec::new();
            let fields = spec
                .child_by_field_name("type")
                .filter(|t| t.kind() == "struct_type")
                .and_then(|t| t.named_child(0));
            if let Some(fields) = fields {
                let mut each = fields.walk();
                for field in fields.named_children(&mut each) {
                    // An embedded field has a type and no name.
                    if field.kind() != "field_declaration"
                        || field.child_by_field_name("name").is_some()
                    {
                        continue;
                    }
                    let Some(mut ty) = field.child_by_field_name("type") else {
                        continue;
                    };
                    if ty.kind() == "pointer_type" {
                        ty = ty.named_child(0).unwrap_or(ty);
                    }
                    if ty.kind() == "generic_type" {
                        ty = ty.child_by_field_name("type").unwrap_or(ty);
                    }
                    embedded.push(text(ty));
                }
            }
            declared.insert(text(name), embedded);
        }
    }

    /// `Some(true)`: a suite. `Some(false)`: not one. `None`: not settled in this file.
    fn settle(
        ty: &str,
        declared: &HashMap<String, Vec<String>>,
        path: &mut Vec<String>,
    ) -> Option<bool> {
        if ty == "suite.Suite" {
            return Some(true);
        }
        let embedded = declared.get(ty)?;
        if path.iter().any(|seen| seen == ty) {
            return Some(false);
        }
        path.push(ty.to_string());
        let mut verdict = Some(false);
        for inner in embedded {
            match settle(inner, declared, path) {
                Some(true) => {
                    verdict = Some(true);
                    break;
                }
                None => verdict = None,
                Some(false) => {}
            }
        }
        path.pop();
        verdict
    }

    let mut suites = HashSet::new();
    for ty in declared.keys().chain(&shaped) {
        let is_suite = match settle(ty, &declared, &mut Vec::new()) {
            Some(settled) => settled,
            None => shaped.contains(ty),
        };
        if is_suite {
            suites.insert(ty.clone());
        }
    }
    suites
}

impl<'a> GoExtractor<'a> {
    fn text(&self, node: Node) -> &'a str {
        node.utf8_text(self.src).unwrap_or("")
    }

    fn collect_comments_and_escape_hatches(&mut self, node: Node) {
        let kind = node.kind();
        if kind == "comment" {
            let text = self.text(node);
            let line = node.start_position().row + 1;
            let trimmed = text
                .trim_start_matches("//")
                .trim_start_matches("/*")
                .trim_end_matches("*/")
                .trim();

            if trimmed.starts_with("nolint")
                || trimmed.starts_with("lint:ignore")
                || trimmed.starts_with("revive:disable")
            {
                self.facts
                    .escape_hatches
                    .push(EscapeHatchSite::LinterDisable {
                        line,
                        rule: trimmed.to_string(),
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
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            if child.kind() == "function_declaration" {
                self.visit_function(child);
            } else if child.kind() == "method_declaration" && self.is_test_path {
                self.visit_method(child);
            }
        }
    }

    fn visit_function(&mut self, node: Node) {
        let name_node = node.child_by_field_name("name");
        let func_name = name_node.map(|n| self.text(n)).unwrap_or("");

        // Benchmarks (BenchmarkXxx taking *testing.B) are performance harnesses, not assertion-bearing unit tests.
        if func_name.starts_with("Benchmark") || self.is_benchmark_signature(node) {
            return;
        }

        let is_test = is_go_test_function_name(func_name) && self.is_unit_test_signature(node);

        if is_test {
            self.record_test(node, func_name);
        } else if self.is_test_path {
            self.record_helper(node, func_name.to_string(), true);
        }
    }

    /// A method of a type in a test file (`func (s Suite) check(t *testing.T)`): a helper
    /// tracked under `Type.method`. A call through a receiver is not resolved to it, since
    /// the receiver's type is not known where the call is read.
    ///
    /// A method of a testify suite ([`suite_types`]) counts the assertions made on its
    /// receiver. One of the shape testify runs is a test, named `Type.Method`: the name
    /// it had as a helper, and what tells `Suite.TestAdd` from another suite's `TestAdd`.
    /// The suite's other methods (`SetupTest`, `TearDownSuite`, `BeforeTest`, its own
    /// checks) stay helpers.
    fn visit_method(&mut self, node: Node) {
        let method = node
            .child_by_field_name("name")
            .map(|n| self.text(n))
            .unwrap_or("");
        let receiver = node
            .child_by_field_name("receiver")
            .and_then(|r| Self::first_of_kind(r, "type_identifier"))
            .map(|n| self.text(n))
            .unwrap_or("");
        if method.is_empty() || receiver.is_empty() {
            return;
        }
        let name = format!("{receiver}.{method}");
        let src = std::str::from_utf8(self.src).unwrap_or("");
        // A gocheck suite method: `func (s *S) TestX(c *C)`. The type need not be a
        // testify suite, and gocheck runs every method of that shape.
        if is_gocheck_test(node, src) {
            self.record_test(node, &name);
            return;
        }
        let suite = method_head(node, src).filter(|(ty, _, _)| self.suites.contains(*ty));
        let Some((_, on, shaped)) = suite else {
            self.record_helper(node, name, false);
            return;
        };
        self.suite_receiver = on.map(str::to_string);
        if shaped {
            self.record_test(node, &name);
        } else {
            self.record_helper(node, name, false);
        }
        self.suite_receiver = None;
    }

    /// Records the function or method `node` as the test `name`.
    fn record_test(&mut self, node: Node, name: &str) {
        let mut test_fn = TestFn {
            name: name.to_string(),
            line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
            ..Default::default()
        };
        let mut direct_calls = Vec::new();
        self.enter_function(node);
        if let Some(body) = node.child_by_field_name("body") {
            let (cases, non_literal_cases, case_rows) =
                super::test_cases::extract_go_cases(body, self.src).into_parts();
            test_fn.cases = cases;
            test_fn.non_literal_cases = non_literal_cases;
            test_fn.case_rows = case_rows;
            self.scan_block(body, &mut test_fn, name, &mut direct_calls);
            super::dispatch_calls(body, self.src, &GO_DISPATCH, &mut direct_calls);
            if !test_fn.ignored {
                self.record_conditional_early_exits(body, &mut test_fn);
            }
        }
        self.facts.tests.push(test_fn);
        self.test_calls.push(direct_calls);
    }

    /// Reads what the function or method `node` makes assertions on beside `t` and the
    /// suite: its gocheck `*C` parameters, and its locals bound to a testify assertion
    /// object.
    fn enter_function(&mut self, node: Node) {
        let src = std::str::from_utf8(self.src).unwrap_or("");
        self.gocheck = if self.is_test_path {
            gocheck_parameters(node, src)
        } else {
            Vec::new()
        };
        self.assertion_locals.clear();
        if let Some(body) = node.child_by_field_name("body") {
            self.read_assertion_locals(body);
        }
    }

    /// Records the locals under `node` bound to a testify assertion object:
    /// `r := s.Require()` and `a := s.Assert()` on the suite being read,
    /// `r := require.New(t)` and `a := assert.New(t)`. `r.NoError(err)` then counts as
    /// `require.NoError(t, err)` does. A name bound to anything else, or assigned twice
    /// to objects that differ, is not recorded.
    fn read_assertion_locals<'t>(&mut self, node: Node<'t>) {
        if matches!(node.kind(), "short_var_declaration" | "var_spec") {
            let left = node
                .child_by_field_name("left")
                .or_else(|| node.child_by_field_name("name"));
            let right = node
                .child_by_field_name("right")
                .or_else(|| node.child_by_field_name("value"));
            let single = |n: Node<'t>| -> Option<Node<'t>> {
                match n.kind() {
                    "expression_list" => (n.named_child_count() == 1)
                        .then(|| n.named_child(0))
                        .flatten(),
                    _ => Some(n),
                }
            };
            let bound = left.and_then(single).zip(right.and_then(single));
            if let Some((name, value)) = bound {
                if name.kind() == "identifier" {
                    if let Some(fatal) = self.assertion_object(value) {
                        let name = self.text(name).to_string();
                        match self.assertion_locals.iter().position(|(n, _)| *n == name) {
                            Some(at) if self.assertion_locals[at].1 != fatal => {
                                self.assertion_locals.remove(at);
                            }
                            Some(_) => {}
                            None => self.assertion_locals.push((name, fatal)),
                        }
                    }
                }
            }
        }
        let mut cursor = node.walk();
        let children: Vec<Node<'t>> = node.children(&mut cursor).collect();
        for child in children {
            self.read_assertion_locals(child);
        }
    }

    /// Whether `value` builds a testify assertion object, and whether its assertions
    /// stop the test: `s.Require()` / `require.New(t)` do, `s.Assert()` / `assert.New(t)`
    /// do not.
    fn assertion_object(&self, value: Node) -> Option<bool> {
        if value.kind() != "call_expression" {
            return None;
        }
        let callee = value.child_by_field_name("function")?;
        if callee.kind() != "selector_expression" {
            return None;
        }
        let on = self.text(callee.child_by_field_name("operand")?);
        let method = self.text(callee.child_by_field_name("field")?);
        let args = Self::collect_arguments(value.child_by_field_name("arguments"));
        match (on, method, args.len()) {
            ("require", "New", 1) => Some(true),
            ("assert", "New", 1) => Some(false),
            (_, "Require", 0) if self.suite_receiver.as_deref() == Some(on) => Some(true),
            (_, "Assert", 0) if self.suite_receiver.as_deref() == Some(on) => Some(false),
            _ => None,
        }
    }

    /// The assertion a call makes on a local bound to a testify assertion object
    /// ([`Self::read_assertion_locals`]), and whether it stops the test.
    fn local_assertion(&self, callee: Node) -> Option<(&'a str, bool)> {
        if callee.kind() != "selector_expression" {
            return None;
        }
        let on = callee.child_by_field_name("operand")?;
        let method = self.text(callee.child_by_field_name("field")?);
        if on.kind() != "identifier" || !is_testify_assertion(method) {
            return None;
        }
        let on = self.text(on);
        self.assertion_locals
            .iter()
            .find(|(name, _)| name == on)
            .map(|(_, fatal)| (method, *fatal))
    }

    /// Counts a gocheck assertion, `c.Assert(obtained, Checker, expected..)` (stops the
    /// test) or `c.Check(..)` (does not), made on a `*C` parameter of the function being
    /// read. A checker that takes an expected value (`Equals`, `DeepEquals`, `Matches`,
    /// `HasLen`, ..) is an equality check; `IsNil` / `NotNil` are not; the same text on
    /// both sides of one is a tautology.
    fn count_gocheck(&self, callee: Node, args: &[Node], test_fn: &mut TestFn) -> bool {
        if callee.kind() != "selector_expression" {
            return false;
        }
        let (Some(on), Some(method)) = (
            callee.child_by_field_name("operand"),
            callee.child_by_field_name("field"),
        ) else {
            return false;
        };
        let fatal = match self.text(method) {
            "Assert" => true,
            "Check" => false,
            _ => return false,
        };
        if on.kind() != "identifier"
            || !self.gocheck.iter().any(|c| c == self.text(on))
            || args.len() < 2
        {
            return false;
        }
        let checker = self.text(args[1]);
        let checker = checker.rsplit('.').next().unwrap_or(checker);
        if let Some(call) = callee.parent() {
            let src = std::str::from_utf8(self.src).unwrap_or("");
            test_fn
                .expected_exceptions
                .extend(super::expected_exceptions::go_gocheck(
                    args,
                    call,
                    src,
                    &self.imports,
                ));
        }
        test_fn.total_asserts += 1;
        if fatal {
            test_fn.fatal_asserts += 1;
        }
        let same = args.len() == 3 && self.text(args[0]).trim() == self.text(args[2]).trim();
        if same {
            test_fn.tautologies += 1;
        } else if !GOCHECK_WEAK_CHECKERS.contains(&checker) {
            test_fn.strong_asserts += 1;
        }
        true
    }

    /// The testify method a call invokes, whatever it is called on, and the position of
    /// its first value argument: 1 on the `assert` or `require` package the file imports
    /// (`require.ErrorIs(t, err, target)`), 0 on the suite being read, on its
    /// `Require()` / `Assert()`, and on a local bound to an assertion object.
    fn testify_call(&self, callee: Node) -> Option<(&'a str, usize)> {
        if callee.kind() != "selector_expression" {
            return None;
        }
        let on = callee.child_by_field_name("operand")?;
        let method = self.text(callee.child_by_field_name("field")?);
        if on.kind() == "identifier" {
            let name = self.text(on);
            let object = self.suite_receiver.as_deref() == Some(name)
                || self.assertion_locals.iter().any(|(local, _)| local == name);
            return if object {
                Some((method, 0))
            } else {
                self.imports.testify(name).then_some((method, 1))
            };
        }
        self.suite_assertion(callee).map(|(method, _)| (method, 0))
    }

    /// Whether a call is `suite.Run(t, <suite value>)`: `Run` of testify's `suite`
    /// package, under the name the file imports it by, with a second argument that is
    /// not a function written in place.
    fn is_suite_run(&self, callee: Node, args: &[Node]) -> bool {
        callee.kind() == "selector_expression"
            && callee
                .child_by_field_name("field")
                .is_some_and(|f| self.text(f) == "Run")
            && callee
                .child_by_field_name("operand")
                .is_some_and(|on| on.kind() == "identifier" && self.text(on) == self.suite_package)
            && args.len() == 2
            && args[1].kind() != "func_literal"
    }

    /// The assertion a call makes on the suite being read, and whether it stops the
    /// test: `s.Equal(..)` and `s.Assert().Equal(..)` do not, `s.Require().Equal(..)`
    /// does, as `assert.Equal` and `require.Equal` do.
    fn suite_assertion(&self, callee: Node) -> Option<(&'a str, bool)> {
        let suite = self.suite_receiver.as_deref()?;
        if callee.kind() != "selector_expression" {
            return None;
        }
        let on = callee.child_by_field_name("operand")?;
        let method = self.text(callee.child_by_field_name("field")?);
        if on.kind() == "identifier" {
            return (self.text(on) == suite && is_testify_assertion(method))
                .then_some((method, false));
        }
        // `s.Require().X(..)` / `s.Assert().X(..)`: every method of those is an assertion.
        if on.kind() != "call_expression" {
            return None;
        }
        let through = on.child_by_field_name("function")?;
        if through.kind() != "selector_expression"
            || self.text(through.child_by_field_name("operand")?) != suite
        {
            return None;
        }
        match self.text(through.child_by_field_name("field")?) {
            "Require" => Some((method, true)),
            "Assert" => Some((method, false)),
            _ => None,
        }
    }

    fn first_of_kind<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
        if node.kind() == kind {
            return Some(node);
        }
        let mut cursor = node.walk();
        let children: Vec<Node<'t>> = node.children(&mut cursor).collect();
        children
            .into_iter()
            .find_map(|c| Self::first_of_kind(c, kind))
    }

    /// Records a non-test function or method as a helper; `resolvable` when a test's
    /// `name(...)` call in this file runs it.
    fn record_helper(&mut self, node: Node, name: String, resolvable: bool) {
        let Some(body) = node.child_by_field_name("body") else {
            return;
        };
        let mut helper_fn = TestFn::default();
        let mut dummy_calls = Vec::new();
        self.enter_function(node);
        self.scan_block(body, &mut helper_fn, &name, &mut dummy_calls);
        helper_fn.total_asserts += super::count_failure_exits(
            body,
            self.src,
            &["call_expression"],
            &["panic("],
            &["func_literal"],
        );
        if resolvable {
            let facts = super::HelperFacts {
                total_asserts: helper_fn.total_asserts,
                strong_asserts: helper_fn.strong_asserts,
                tautologies: helper_fn.tautologies,
                fatal_asserts: helper_fn.fatal_asserts,
                wraps: super::forwarding_wrapper_callee(
                    body,
                    &GO_WRAPPER,
                    &GO_LOCALS,
                    &dummy_calls,
                    self.src,
                ),
            };
            self.helpers.insert(name.clone(), facts);
        }
        let line = node.start_position().row + 1;
        let end_line = node.end_position().row + 1;
        self.facts.push_helper(
            super::TestHelperFacts {
                name,
                line,
                end_line,
                total_asserts: helper_fn.total_asserts,
                strong_asserts: helper_fn.strong_asserts,
                tautologies: helper_fn.tautologies,
                fatal_asserts: helper_fn.fatal_asserts,
                helper_checks: 0,
                equality_exits: 0,
            },
            dummy_calls,
        );
    }

    fn is_benchmark_signature(&self, func_node: Node) -> bool {
        let Some(params) = func_node.child_by_field_name("parameters") else {
            return false;
        };
        let mut cursor = params.walk();
        for child in params.children(&mut cursor) {
            if child.kind() == "parameter_declaration" {
                let type_text = child
                    .child_by_field_name("type")
                    .map(|n| self.text(n).trim())
                    .unwrap_or("");
                if type_text.contains("testing.B") || type_text.ends_with("*B") {
                    return true;
                }
            }
        }
        false
    }

    fn is_unit_test_signature(&self, func_node: Node) -> bool {
        let Some(params) = func_node.child_by_field_name("parameters") else {
            return false;
        };
        let mut cursor = params.walk();
        for child in params.children(&mut cursor) {
            if child.kind() == "parameter_declaration" {
                let type_text = child
                    .child_by_field_name("type")
                    .map(|n| self.text(n).trim())
                    .unwrap_or("");
                if type_text.ends_with("testing.TB")
                    || type_text.ends_with("*testing.TB")
                    || type_text == "TB"
                    || type_text == "*TB"
                {
                    return false;
                }
                if type_text.ends_with("testing.T")
                    || type_text.ends_with("*testing.T")
                    || type_text.ends_with("testing.F")
                    || type_text.ends_with("*testing.F")
                    || type_text == "*T"
                    || type_text == "*F"
                    || type_text == "T"
                    || type_text == "F"
                {
                    return true;
                }
            }
        }
        false
    }

    fn scan_block(
        &mut self,
        block: Node,
        test_fn: &mut TestFn,
        parent_name: &str,
        direct_calls: &mut Vec<String>,
    ) {
        let mut cursor = block.walk();
        for child in block.children(&mut cursor) {
            self.scan_node(child, test_fn, parent_name, direct_calls);
        }
    }

    /// Records where the tautologies counted under `node` are (`TestFn::mark_tautologies`).
    fn scan_node(
        &mut self,
        node: Node,
        test_fn: &mut TestFn,
        parent_name: &str,
        direct_calls: &mut Vec<String>,
    ) {
        let mark = test_fn.tautology_mark();
        self.scan_node_unmarked(node, test_fn, parent_name, direct_calls);
        test_fn.mark_tautologies(mark, node);
    }

    fn scan_node_unmarked(
        &mut self,
        node: Node,
        test_fn: &mut TestFn,
        parent_name: &str,
        direct_calls: &mut Vec<String>,
    ) {
        if super::reach::is_dead(&self.dead, node.start_byte()) {
            return;
        }
        match node.kind() {
            "call_expression" => {
                self.inspect_call(node, test_fn, parent_name, direct_calls);
            }
            _ => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.scan_node(child, test_fn, parent_name, direct_calls);
                }
            }
        }
    }

    fn inspect_call(
        &mut self,
        node: Node,
        test_fn: &mut TestFn,
        parent_name: &str,
        direct_calls: &mut Vec<String>,
    ) {
        let Some(func_node) = node.child_by_field_name("function") else {
            return;
        };

        let call_text = self.text(func_node);
        let args_node = node.child_by_field_name("arguments");
        let args = Self::collect_arguments(args_node);

        // The entry point of a testify suite: `suite.Run(t, new(CalcSuite))` runs the
        // suite's `Test*` methods, each of which is read as a test of its own where it
        // is declared. The call is not a subtest with a body to read; it is what the
        // entry function does, whether the suite is declared in this file or elsewhere.
        if self.is_suite_run(func_node, &args) {
            test_fn.total_asserts += 1;
            test_fn.strong_asserts += 1;
            return;
        }

        // Subtests: t.Run("subtest", func(t *testing.T) { ... })
        if (call_text.ends_with(".Run") || call_text == "Run") && args.len() >= 2 {
            let sub_title = self.extract_string_literal(args[0]);
            let sub_name = if sub_title.is_empty() {
                format!("{}/subtest", parent_name)
            } else {
                format!("{}/{}", parent_name, sub_title)
            };

            let line = node.start_position().row + 1;
            let end_line = node.end_position().row + 1;
            let mut sub_test = TestFn {
                name: sub_name.clone(),
                line,
                end_line,
                total_asserts: 0,
                strong_asserts: 0,
                tautologies: 0,
                ignored: test_fn.ignored,
                should_panic: None,
                ..Default::default()
            };

            let mut sub_calls = Vec::new();
            // Recursively scan callback body
            if let Some(func_lit) = args.iter().find(|a| a.kind() == "func_literal") {
                if let Some(sub_body) = func_lit.child_by_field_name("body") {
                    self.scan_block(sub_body, &mut sub_test, &sub_name, &mut sub_calls);
                    super::dispatch_calls(sub_body, self.src, &GO_DISPATCH, &mut sub_calls);
                    if !sub_test.ignored {
                        self.record_conditional_early_exits(sub_body, &mut sub_test);
                    }
                }
            }

            self.facts.tests.push(sub_test);
            self.test_calls.push(sub_calls);
            test_fn.total_asserts += 1;
            test_fn.strong_asserts += 1;
            return;
        }

        // Check for test skip: t.Skip(...), t.Skipf(...), t.SkipNow()
        if call_text.ends_with(".Skip")
            || call_text.ends_with(".Skipf")
            || call_text.ends_with(".SkipNow")
            || call_text == "Skip"
            || call_text == "Skipf"
        {
            // `if testing.Short() { t.Skip(...) }` runs in a full run: a conditional skip,
            // reported as a note. A constant condition skips every run.
            // A skip in the `else` branch of an `if` on a CI variable is conditional too.
            // A test keeps its first condition unless a later one makes it skip in CI.
            let legacy = enclosing_if_condition(node, self.src);
            let constant = legacy.as_deref() == Some("true");
            let site = super::ci_condition::site(super::ci_condition::Lang::Go, node, self.src);
            match super::ci_condition::conditional(legacy, site) {
                Some((cond, verdict)) if !constant => {
                    test_fn.record_conditional_skip(cond, verdict);
                }
                _ if test_fn.conditional_ignore.is_none() => test_fn.ignored = true,
                _ => {}
            }
            return;
        }

        // The expected failure a testify assertion states (`require.ErrorIs(t, err, ErrGone)`).
        if let Some((method, first)) = self.testify_call(func_node) {
            let src = std::str::from_utf8(self.src).unwrap_or("");
            let values = args.get(first..).unwrap_or(&[]);
            test_fn
                .expected_exceptions
                .extend(super::expected_exceptions::go_testify(
                    method,
                    values,
                    node,
                    src,
                    &self.imports,
                ));
        }

        // An assertion made on a testify suite: `s.Equal(..)`, `s.Require().NoError(..)`.
        if let Some((method, fatal)) = self.suite_assertion(func_node) {
            self.count_testify(test_fn, method, &args, 0, fatal);
            return;
        }

        // An assertion made on a local bound to one: `r := s.Require(); r.NoError(err)`.
        if let Some((method, fatal)) = self.local_assertion(func_node) {
            self.count_testify(test_fn, method, &args, 0, fatal);
            return;
        }

        // A gocheck assertion: `c.Assert(got, Equals, want)`, `c.Check(err, IsNil)`.
        if self.count_gocheck(func_node, &args, test_fn) {
            return;
        }

        // Standard testing methods: t.Fatalf, t.Fatal, t.Errorf, t.Error, t.FailNow
        if call_text.ends_with(".Fatalf")
            || call_text.ends_with(".Fatal")
            || call_text.ends_with(".FailNow")
        {
            test_fn.total_asserts += 1;
            test_fn.strong_asserts += 1;
            test_fn.fatal_asserts += 1;
            return;
        }
        if call_text.ends_with(".Errorf") || call_text.ends_with(".Error") {
            test_fn.total_asserts += 1;
            test_fn.strong_asserts += 1;
            return;
        }

        if call_text.ends_with(".Fail") {
            test_fn.total_asserts += 1;
            // Weak assertion
            return;
        }

        // Testify assert / require
        let method_name = call_text.rsplit('.').next().unwrap_or(call_text);
        if matches!(call_text, "assert.New" | "require.New") {
            // Builds an assertion object (`read_assertion_locals`); it checks nothing.
            return;
        }
        if call_text.starts_with("assert.") || call_text.starts_with("require.") {
            // The first argument is `t`.
            let fatal = call_text.starts_with("require.");
            self.count_testify(test_fn, method_name, &args, 1, fatal);
            return;
        }

        // Track potential call to helper
        if func_node.kind() == "identifier" {
            let id_text = self.text(func_node);
            direct_calls.push(id_text.to_string());
        }

        // Configured helper functions
        if self
            .vocab
            .helper_fns
            .iter()
            .any(|h| super::helper_call_matches(method_name, h))
        {
            test_fn.total_asserts += 1;
            return;
        }

        // Recursively inspect child nodes
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child != func_node {
                self.scan_node(child, test_fn, parent_name, direct_calls);
            }
        }
    }

    /// Counts the testify assertion `method` into `test_fn`. `first` is the position of
    /// its first value argument: 1 for `assert.Equal(t, a, b)`, 0 for `s.Equal(a, b)`.
    fn count_testify(
        &self,
        test_fn: &mut TestFn,
        method: &str,
        args: &[Node],
        first: usize,
        fatal: bool,
    ) {
        if fatal {
            test_fn.fatal_asserts += 1;
        }
        let value = |at: usize| args.get(first + at).map(|a| self.text(*a).trim());
        match method {
            "True" => {
                test_fn.total_asserts += 1;
                if value(0) == Some("true") {
                    test_fn.tautologies += 1;
                }
            }
            "False" => {
                test_fn.total_asserts += 1;
                if value(0) == Some("false") {
                    test_fn.tautologies += 1;
                }
            }
            "Equal" | "Same" => {
                test_fn.total_asserts += 1;
                match (value(0), value(1)) {
                    (Some(a), Some(b)) if a == b => test_fn.tautologies += 1,
                    _ => test_fn.strong_asserts += 1,
                }
            }
            "Nil" | "NotNil" => {
                test_fn.total_asserts += 1;
            }
            _ => {
                test_fn.total_asserts += 1;
                test_fn.strong_asserts += 1;
            }
        }
    }

    fn resolve_same_file_helpers(&mut self) {
        for (i, test) in self.facts.tests.iter_mut().enumerate() {
            if let Some(calls) = self.test_calls.get(i) {
                super::resolve_test_same_file_helpers(test, calls, self.vocab, |call| {
                    super::helper_through_wrappers(call, &self.helpers)
                });
            }
        }
    }

    fn collect_arguments(args_node: Option<Node>) -> Vec<Node> {
        let mut result = Vec::new();
        let Some(args) = args_node else {
            return result;
        };
        for i in 0..args.child_count() {
            if let Some(c) = args.child(i) {
                if c.is_named() {
                    result.push(c);
                }
            }
        }
        result
    }

    fn extract_string_literal(&self, node: Node) -> String {
        let t = self.text(node).trim();
        if (t.starts_with('"') && t.ends_with('"')) || (t.starts_with('`') && t.ends_with('`')) {
            t[1..t.len() - 1].to_string()
        } else {
            t.to_string()
        }
    }

    /// Early exits under a condition: the first `if` the text rule below accepts, then
    /// every `return` under an `if` (nested and `else` branches included) that a CI
    /// variable is involved in, through a variable, constant or helper of this file.
    fn record_conditional_early_exits(&self, body: Node, test: &mut TestFn) {
        use super::ci_condition::{self, CiVerdict, Lang};
        if let Some((cond, consequence)) = self.detect_go_conditional_early_exit(body) {
            let verdict = ci_condition::site(Lang::Go, consequence, self.src)
                .map_or(CiVerdict::NotCi, |s| s.verdict);
            test.record_conditional_skip(cond, verdict);
        }
        for exit in ci_condition::exits_under_if(body, &|n| n.kind() == "return_statement") {
            if let Some(site) = ci_condition::site(Lang::Go, exit, self.src) {
                if site.related {
                    test.record_conditional_skip(site.text, site.verdict);
                }
            }
        }
    }

    fn detect_go_conditional_early_exit<'t>(&self, body: Node<'t>) -> Option<(String, Node<'t>)> {
        let mut cursor = body.walk();
        let mut env_bindings = std::collections::HashSet::new();

        let mut statements = Vec::new();
        for child in body.children(&mut cursor) {
            if child.kind() == "statement_list" {
                let mut s_cursor = child.walk();
                for stmt in child.children(&mut s_cursor) {
                    statements.push(stmt);
                }
            } else {
                statements.push(child);
            }
        }

        for stmt in statements {
            if stmt.kind() == "short_var_declaration" && is_go_env_check(stmt, self.src) {
                if let Some(left) = stmt.child_by_field_name("left") {
                    let name = self.text(left).trim();
                    if !name.is_empty() {
                        env_bindings.insert(name.to_string());
                    }
                }
            }

            if stmt.kind() == "if_statement" {
                let init = stmt.child_by_field_name("initializer");
                let cond = stmt.child_by_field_name("condition");
                let init_str = init
                    .and_then(|n| n.utf8_text(self.src).ok())
                    .unwrap_or("")
                    .trim();
                let cond_str = cond
                    .and_then(|n| n.utf8_text(self.src).ok())
                    .unwrap_or("")
                    .trim();

                let is_env = init.is_some_and(|n| is_go_env_check(n, self.src))
                    || cond.is_some_and(|n| {
                        is_go_env_check(n, self.src)
                            || super::code_names_one_of(n, self.src, &GO_NOT_CODE, &env_bindings)
                    });

                if is_env {
                    if let Some(consequence) = stmt.child_by_field_name("consequence") {
                        if go_consequence_returns_early(consequence, self.src) {
                            let cond_desc = if !init_str.is_empty() {
                                format!("{init_str}; {cond_str}")
                            } else {
                                cond_str.to_string()
                            };
                            return Some((cond_desc, consequence));
                        }
                    }
                }
            }
        }
        None
    }
}

fn go_fn_is_test(node: tree_sitter::Node, src: &str, path: &str) -> bool {
    let name = node
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(src.as_bytes()).ok())
        .unwrap_or("");
    functions::is_test_file(path, Some(is_go_test_path)) || is_go_test_function_name(name)
}

const GO_NOT_CODE: super::NotCode = super::NotCode {
    strings: &[
        "interpreted_string_literal",
        "raw_string_literal",
        "rune_literal",
    ],
    comments: &["comment"],
    interpolations: &[],
};

/// Whether `node` is a candidate for a condition on the environment: its code, outside
/// string literals and comments, spells an environment read or names a CI variable. A CI
/// variable named in a string (`os.Getenv("CI")`, `lookup("CI")`) is read by
/// `ci_condition::site`, from the tree.
fn is_go_env_check(node: Node, src: &[u8]) -> bool {
    let code = super::code_text(node, src, &GO_NOT_CODE);
    code.contains("Getenv") || code.contains("LookupEnv") || super::is_ci_condition(&code)
}

fn go_consequence_returns_early(consequence: Node, src: &[u8]) -> bool {
    let mut statements = Vec::new();
    let mut cursor = consequence.walk();
    for child in consequence.children(&mut cursor) {
        if child.kind() == "statement_list" {
            let mut s_cursor = child.walk();
            for stmt in child.children(&mut s_cursor) {
                statements.push(stmt);
            }
        } else {
            statements.push(child);
        }
    }

    for child in statements {
        if child.kind() == "return_statement" {
            return true;
        }
        if child.kind() == "expression_statement" {
            if let Some(call) = child.child(0) {
                if call.kind() == "call_expression" {
                    if let Some(func) = call.child_by_field_name("function") {
                        if let Ok(name) = func.utf8_text(src) {
                            if name.ends_with(".Skip")
                                || name.ends_with(".Skipf")
                                || name.ends_with(".SkipNow")
                                || name == "Skip"
                                || name == "Skipf"
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

/// The condition of the nearest `if` whose body holds `node`, stopping at the function
/// the call is in; `None` when the call runs unconditionally.
fn enclosing_if_condition(node: Node, src: &[u8]) -> Option<String> {
    let mut cur = node;
    while let Some(p) = cur.parent() {
        match p.kind() {
            "function_declaration" | "method_declaration" | "func_literal" => return None,
            "if_statement" => {
                let cond = p.child_by_field_name("condition")?;
                let in_body = p
                    .child_by_field_name("consequence")
                    .is_some_and(|c| c.id() == cur.id());
                if in_body {
                    let cond_str = cond.utf8_text(src).ok()?.trim().to_string();
                    if let Some(init) = p.child_by_field_name("initializer") {
                        if let Ok(init_str) = init.utf8_text(src) {
                            return Some(format!("{}; {}", init_str.trim(), cond_str));
                        }
                    }
                    return Some(cond_str);
                }
            }
            _ => {}
        }
        cur = p;
    }
    None
}

pub const GO_FUNCTIONS: FunctionSpec = FunctionSpec {
    function_kinds: &["function_declaration", "method_declaration"],
    name_fields: &["name"],
    body_fields: &["body"],
    ignored_kinds: &["comment"],
    skip: functions::skip_none,
    is_test: go_fn_is_test,
    classify: functions::classify_go,
};

/// A method called on a receiver (`method_checks`).
pub const GO_RECEIVER_CALLS: super::method_checks::ReceiverCalls =
    super::method_checks::ReceiverCalls {
        member: &[(
            "call_expression",
            "function",
            "selector_expression",
            "field",
        )],
        direct: &[],
        bare: &[],
        tokens: &[],
    };

pub const GO_MOCKS: super::mocks::MockSpec = super::mocks::MockSpec {
    call_kinds: &["call_expression"],
    callee_fields: &["function"],
};

pub const GO_HANDLERS: super::handlers::HandlerSpec = super::handlers::HandlerSpec {
    handler_kinds: &[],
    arm_of: &[],
    body_fields: &[],
    ignored_kinds: &["comment"],
    trivial: &[],
    discard_kinds: &["assignment_statement", "short_var_declaration"],
    discards: super::handlers::go_discards,
    classify_discard: Some(super::handlers::go_discard_class),
    call_value_kinds: &[],
    silence_kinds: &[],
    silences: super::handlers::no_discard,
    silence_node: None,
};

pub const GO_RETRIES: super::retries::RetrySpec = super::retries::RetrySpec {
    marker_kinds: &["call_expression"],
};

pub const GO_REACH: super::reach::ReachSpec = super::reach::ReachSpec {
    if_kinds: &["if_statement"],
    // A Go block holds its statements in a `statement_list`.
    block_kinds: &["statement_list"],
    ignored_kinds: &["comment"],
    terminators: &["return", "panic(", "t.FailNow()", "os.Exit("],
};

/// Functions a test body runs through a dispatch table (`super::dispatch_calls`).
pub const GO_DISPATCH: super::DispatchSpec = super::DispatchSpec {
    containers: &["literal_value"],
    names: &["identifier"],
    references: &[],
};

/// A local a Go wrapper computes and forwards: `b := loc(d)`, `var b = loc(d)`.
pub const GO_LOCALS: super::LocalSpec = super::LocalSpec {
    statements: &["var_declaration"],
    binders: &["short_var_declaration", "var_spec"],
    pattern: &["left", "name"],
    value: &["right", "value"],
    names: &["identifier"],
    holders: &["expression_list"],
    refused: &[],
};

/// A Go helper whose body is one call: `{ return check(t, true) }`.
pub const GO_WRAPPER: super::WrapperSpec = super::WrapperSpec {
    through: &[
        "block",
        "statement_list",
        "expression_statement",
        "return_statement",
        "expression_list",
    ],
    calls: &["call_expression"],
    arguments: &["argument_list"],
    references: &[("unary_expression", "&")],
    plain: &["true", "false", "nil"],
    skip: &["comment"],
};

#[cfg(test)]
mod tests {
    use super::*;

    /// The function rule is the shared-or-own superset: a plain function counts as a
    /// test function in a file only the shared rule recognises (`benches/`, which the
    /// Go convention does not name), as before in a `_test.go` file, and not elsewhere.
    #[test]
    fn fn_rule_is_the_shared_or_own_superset() {
        let src = "package a\nfunc Helper() int { return 1 }\n";
        let is_test = |path: &str| {
            GoPack
                .extract(path, src, &AssertVocabulary::default())
                .unwrap()
                .functions
                .iter()
                .map(|f| (f.name.clone(), f.is_test))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            is_test("benches/a.go"),
            vec![("Helper".to_string(), true)],
            "shared rule only"
        );
        assert_eq!(
            is_test("pkg/a_test.go"),
            vec![("Helper".to_string(), true)],
            "both rules"
        );
        assert_eq!(
            is_test("pkg/a.go"),
            vec![("Helper".to_string(), false)],
            "negative control"
        );
    }

    /// A test calling a thin wrapper gets the credit of one calling the wrapped function
    /// (`crate::ast::thin_wrapper_counts` names the controls).
    #[test]
    fn a_thin_wrapper_resolves_to_the_function_it_wraps() {
        let src = r##"package a

import "testing"

func checked(x int, strict bool) {
	if strict && x != 1 {
		panic("x")
	}
}
func noop(x int, n int) {}
func via(x int) { checked(x, true) }
func hollow(x int) { noop(x, 1) }
func busy(x int) {
	checked(x, true)
	prepare(x)
}
func ping(x int) { pong(x, 1) }
func pong(x int, n int) { ping(x) }
func TestDirect(t *testing.T) { checked(1, true) }
func TestViaWrapper(t *testing.T) { via(1) }
func TestHollowWrapper(t *testing.T) { hollow(1) }
func TestBusyHelper(t *testing.T) { busy(1) }
func TestWrapperCycle(t *testing.T) { ping(1) }
"##;
        assert_eq!(
            crate::ast::thin_wrapper_counts(&GoPack, "pkg/wrap_test.go", src),
            crate::ast::ONE_LEVEL_WRAPPER_COUNTS
        );
        // A closure passed along is work the helper does: not a wrapper, although its
        // `func_literal` ends in `literal` and makes no call.
        let closure = format!(
            "{src}func deferred(x int) {{ checked(x, func() {{}}) }}\nfunc TestClosureArg(t *testing.T) {{ deferred(1) }}\n"
        );
        let facts = GoPack
            .extract("pkg/wrap_test.go", &closure, &AssertVocabulary::default())
            .unwrap();
        let t = facts
            .tests
            .iter()
            .find(|t| t.name.contains("Closure"))
            .unwrap();
        assert_eq!((t.total_asserts, t.helper_checks), (0, 0), "{t:?}");
    }

    #[test]
    fn test_go_std_test_extraction_and_assertions() {
        let src = r#"
package main

import "testing"

func TestCalculator(t *testing.T) {
	got := 1 + 1
	if got != 2 {
		t.Fatalf("expected 2, got %d", got)
	}
	if 2 + 2 != 4 {
		t.Errorf("expected 4")
	}
}
"#;
        let pack = GoPack;
        let facts = pack
            .extract("calc_test.go", src, &AssertVocabulary::default())
            .expect("extraction must succeed");

        assert_eq!(facts.tests.len(), 1);
        let t = &facts.tests[0];
        assert_eq!(t.name, "TestCalculator");
        assert_eq!(t.total_asserts, 2);
        assert_eq!(t.strong_asserts, 2);
        assert!(!t.is_vacuous());
        assert!(!t.ignored);
    }

    #[test]
    fn a_skip_under_an_if_is_conditional_and_an_unconditional_one_is_ignored() {
        let src = "package p\n\nimport \"testing\"\n\nfunc TestShort(t *testing.T) {\n\tif testing.Short() {\n\t\tt.Skip(\"short mode\")\n\t}\n\tif 1+1 != 2 {\n\t\tt.Fatal(\"math\")\n\t}\n}\n\nfunc TestAlways(t *testing.T) {\n\tt.Skip(\"later\")\n}\n\nfunc TestConstant(t *testing.T) {\n\tif true {\n\t\tt.Skip(\"later\")\n\t}\n}\n";
        let facts = GoPack
            .extract("p_test.go", src, &AssertVocabulary::default())
            .unwrap();
        let by = |n: &str| facts.tests.iter().find(|t| t.name == n).unwrap();
        let short = by("TestShort");
        assert!(!short.ignored);
        assert_eq!(short.conditional_ignore.as_deref(), Some("testing.Short()"));
        assert!(by("TestAlways").ignored && by("TestAlways").conditional_ignore.is_none());
        assert!(by("TestConstant").ignored && by("TestConstant").conditional_ignore.is_none());
    }

    #[test]
    fn early_exit_in_go_test_under_env_or_ci_check_detected() {
        let src = r#"
package p

import (
	"os"
	"testing"
)

func TestCIReturn(t *testing.T) {
	if os.Getenv("CI") != "" {
		return
	}
	if 1+1 != 2 {
		t.Fatal("math")
	}
}

func TestCILookupEnvSkip(t *testing.T) {
	if _, ok := os.LookupEnv("GITHUB_ACTIONS"); ok {
		t.Skip("skipping in github actions")
	}
	if 1+1 != 2 {
		t.Fatal("math")
	}
}

func TestGenericEnvReturn(t *testing.T) {
	if os.Getenv("SKIP_SLOW") != "" {
		return
	}
	if 1+1 != 2 {
		t.Fatal("math")
	}
}

func TestGuardWithoutExit(t *testing.T) {
	if os.Getenv("CI") != "" {
		println("in CI")
	}
	if 1+1 != 2 {
		t.Fatal("math")
	}
}

func helperGuard(t *testing.T) {
	if os.Getenv("CI") != "" {
		return
	}
}
"#;
        let facts = GoPack
            .extract("p_test.go", src, &AssertVocabulary::default())
            .unwrap();
        let by = |n: &str| facts.tests.iter().find(|t| t.name == n).unwrap();

        let ci_return = by("TestCIReturn");
        assert!(!ci_return.ignored);
        assert_eq!(
            ci_return.conditional_ignore.as_deref(),
            Some("os.Getenv(\"CI\") != \"\"")
        );

        let ci_lookup = by("TestCILookupEnvSkip");
        assert!(!ci_lookup.ignored);
        assert_eq!(
            ci_lookup.conditional_ignore.as_deref(),
            Some("_, ok := os.LookupEnv(\"GITHUB_ACTIONS\"); ok")
        );

        let generic = by("TestGenericEnvReturn");
        assert!(!generic.ignored);
        assert_eq!(
            generic.conditional_ignore.as_deref(),
            Some("os.Getenv(\"SKIP_SLOW\") != \"\"")
        );

        let no_exit = by("TestGuardWithoutExit");
        assert!(!no_exit.ignored);
        assert_eq!(no_exit.conditional_ignore, None);

        assert!(facts.tests.iter().all(|t| t.name != "helperGuard"));
    }

    #[test]
    fn test_go_vacuous_test_detected() {
        let src = r#"
package main

import "testing"

func TestEmpty(t *testing.T) {
	// does nothing
}
"#;
        let pack = GoPack;
        let facts = pack
            .extract("empty_test.go", src, &AssertVocabulary::default())
            .expect("extraction must succeed");

        assert_eq!(facts.tests.len(), 1);
        assert!(facts.tests[0].is_vacuous());
    }

    #[test]
    fn test_go_test_skip_detected() {
        let src = r#"
package main

import "testing"

func TestSkipped(t *testing.T) {
	t.Skip("skipping for now")
	if 1 != 2 {
		t.Fatal("fail")
	}
}
"#;
        let pack = GoPack;
        let facts = pack
            .extract("skip_test.go", src, &AssertVocabulary::default())
            .expect("extraction must succeed");

        assert_eq!(facts.tests.len(), 1);
        assert!(facts.tests[0].ignored);
    }

    #[test]
    fn test_go_testify_and_tautologies() {
        let src = r#"
package main

import (
	"testing"
	"github.com/stretchr/testify/assert"
)

func TestTestify(t *testing.T) {
	assert.Equal(t, 2, 1+1)
	assert.True(t, true) // tautology
	assert.Equal(t, 42, 42) // tautology
}
"#;
        let pack = GoPack;
        let facts = pack
            .extract("testify_test.go", src, &AssertVocabulary::default())
            .expect("extraction must succeed");

        assert_eq!(facts.tests.len(), 1);
        let t = &facts.tests[0];
        assert_eq!(t.total_asserts, 3);
        assert_eq!(t.strong_asserts, 1);
        assert_eq!(t.tautologies, 2);
        assert_eq!(t.effective_asserts(), 1);
        assert!(!t.is_vacuous());
    }

    #[test]
    fn test_go_subtests_with_t_run() {
        let src = r#"
package main

import "testing"

func TestSuite(t *testing.T) {
	t.Run("subA", func(t *testing.T) {
		t.Fatal("fail")
	})
	t.Run("subB", func(t *testing.T) {
		t.Skip("skip B")
	})
}
"#;
        let pack = GoPack;
        let facts = pack
            .extract("sub_test.go", src, &AssertVocabulary::default())
            .expect("extraction must succeed");

        assert_eq!(facts.tests.len(), 3);
        assert_eq!(facts.tests[0].name, "TestSuite/subA");
        assert_eq!(facts.tests[0].total_asserts, 1);
        assert!(!facts.tests[0].ignored);

        assert_eq!(facts.tests[1].name, "TestSuite/subB");
        assert!(facts.tests[1].ignored);

        assert_eq!(facts.tests[2].name, "TestSuite");
    }

    #[test]
    fn test_go_benchmarks_distinguished_from_unit_tests() {
        let src = r#"
package main

import "testing"

func TestReal(t *testing.T) {
	t.Fatal("fail")
}

func BenchmarkSearch(b *testing.B) {
	for i := 0; i < b.N; i++ {
		// Benchmark timing loop without assertions
	}
}
"#;
        let pack = GoPack;
        let facts = pack
            .extract("bench_test.go", src, &AssertVocabulary::default())
            .expect("extraction must succeed");

        assert_eq!(
            facts.tests.len(),
            1,
            "only TestReal must be extracted as a test"
        );
        assert_eq!(facts.tests[0].name, "TestReal");
    }

    #[test]
    fn test_go_testify_require_fatal_and_nil_checks() {
        let src = r#"
package main

import (
	"testing"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestFatalAndNil(t *testing.T) {
	x := 1
	y := 2
	assert.NotNil(t, t)       // weak assert (not strong), not fatal
	require.NotNil(t, t)      // weak assert (not strong), fatal
	assert.Equal(t, 1, x)     // strong assert, not fatal
	require.Equal(t, 2, y)    // strong assert, fatal
	t.Fatalf("fatal abort")   // strong assert, fatal
}
"#;
        let pack = GoPack;
        let facts = pack
            .extract("require_test.go", src, &AssertVocabulary::default())
            .expect("extraction must succeed");

        assert_eq!(facts.tests.len(), 1);
        let t = &facts.tests[0];
        assert_eq!(t.total_asserts, 5);
        // NotNil are 2 weak asserts, 2 Equal + 1 Fatalf are 3 strong
        assert_eq!(t.strong_asserts, 3);
        // require.NotNil + require.Equal + t.Fatalf are 3 fatal
        assert_eq!(t.fatal_asserts, 3);
    }

    #[test]
    fn test_go_helper_functions_and_same_file_resolution() {
        let src = r#"
package main

import "testing"

func assertValue(t *testing.T, got, want int) {
	if got != want {
		t.Fatalf("expected %d, got %d", want, got)
	}
}

func testHelperUnused(_ testing.TB) {
	// helper not starting with Test
}

func TestCaller(t *testing.T) {
	assertValue(t, 1+1, 2)
}
"#;
        let pack = GoPack;
        let facts = pack
            .extract("caller_test.go", src, &AssertVocabulary::default())
            .expect("extraction must succeed");

        assert_eq!(
            facts.tests.len(),
            1,
            "only TestCaller must be extracted as a test; assertValue and testHelperUnused are helpers"
        );
        let t = &facts.tests[0];
        assert_eq!(t.name, "TestCaller");
        assert_eq!(t.total_asserts, 1, "must resolve helper assertion");
        assert_eq!(t.strong_asserts, 1);
        assert_eq!(t.fatal_asserts, 1);
        assert!(!t.is_vacuous());
    }

    fn go_facts(src: &str) -> ParsedFileFacts {
        GoPack
            .extract("calc_test.go", src, &AssertVocabulary::default())
            .unwrap()
    }

    const SUITE: &str = "package calc\n\ntype Suite struct {\n\tsuite.Suite\n}\n\n";

    /// #595: a `Test*` method of a testify suite is a test named `Type.Method`; the
    /// suite's other methods are helpers; every way a suite spells an assertion counts,
    /// with `Require` forms fatal.
    #[test]
    fn testify_suite_methods_are_tests_and_their_assertions_count() {
        let src = format!("{SUITE}func (s *Suite) SetupTest() {{\n\ts.Require().NoError(open())\n}}\n\nfunc (s *Suite) TestAdd() {{\n\ts.Equal(2, Add(1, 1))\n\ts.Equalf(3, Add(1, 2), \"case %d\", 3)\n\ts.Len(items(), 2)\n\ts.Nil(err)\n\ts.Assert().True(ok())\n\ts.Require().NoError(run())\n\ts.Require().Error(fail())\n\tassert.Equal(s.T(), 4, Add(2, 2))\n\trequire.Equal(s.T(), 5, Add(2, 3))\n\ts.Equal(7, 7)\n\ts.True(true)\n\ts.helper(1)\n\ts.T().Log(\"x\")\n}}\n\nfunc (s *Suite) helper(n int) {{\n\ts.Equal(n, 1)\n}}\n\nfunc (s *Suite) TestWith(n int) {{\n\ts.Equal(n, 1)\n}}\n\nfunc TestSuite(t *testing.T) {{\n\tsuite.Run(t, new(Suite))\n}}\n");
        let facts = go_facts(&src);
        let t = facts
            .tests
            .iter()
            .find(|t| t.name == "Suite.TestAdd")
            .unwrap_or_else(|| panic!("{:?}", facts.tests));
        // Eleven assertions, two of them tautologies; three made through `Require`.
        assert_eq!(
            (t.total_asserts, t.tautologies, t.fatal_asserts),
            (11, 2, 3),
            "{t:?}"
        );
        // `Nil` and `True` are not strong; the tautologies are not either.
        assert_eq!(t.strong_asserts, 7, "{t:?}");
        let helpers: Vec<(&str, usize, usize)> = facts
            .test_helpers
            .iter()
            .map(|h| (h.name.as_str(), h.total_asserts, h.fatal_asserts))
            .collect();
        assert_eq!(
            helpers,
            vec![
                ("Suite.SetupTest", 1, 1),
                ("Suite.helper", 1, 0),
                ("Suite.TestWith", 1, 0)
            ]
        );
        assert!(!facts.tests.iter().any(|t| t.name.contains("Setup")));
    }

    /// Which types are suites: one that embeds `suite.Suite`, directly, by pointer or
    /// through a struct of the file; and one the file cannot settle that has a method of
    /// the shape testify runs. A struct of the file that embeds no suite is not one,
    /// whatever its methods are called.
    #[test]
    fn suite_types_are_settled_in_the_file_or_read_by_the_shape_of_their_methods() {
        let names = |src: &str| -> Vec<String> {
            let mut names: Vec<String> = go_facts(src).tests.into_iter().map(|t| t.name).collect();
            names.sort();
            names
        };
        let method =
            |ty: &str, head: &str| format!("func (s *{ty}) {head} {{\n\ts.Equal(1, 1+0)\n}}\n\n");
        // Embedded by pointer, and through a struct of the file.
        let src = format!("package x\n\ntype Base struct {{\n\t*suite.Suite\n\tdb DB\n}}\n\ntype Api struct {{\n\tBase\n}}\n\n{}{}", method("Api", "TestA()"), method("Base", "TestB()"));
        assert_eq!(names(&src), ["Api.TestA", "Base.TestB"]);
        // Declared in another file, or embedding a type declared elsewhere: read by the
        // shape. A value receiver and a generic one are receivers too.
        let src = format!("package x\n\ntype Remote struct {{\n\thelp.Harness\n}}\n\n{}{}func (s Elsewhere) TestV() {{\n\ts.Equal(1, 1+0)\n}}\n\nfunc (s *Gen[T]) TestG() {{\n\ts.Equal(1, 1+0)\n}}\n", method("Remote", "TestR()"), method("Elsewhere", "TestE()"));
        assert_eq!(
            names(&src),
            [
                "Elsewhere.TestE",
                "Elsewhere.TestV",
                "Gen.TestG",
                "Remote.TestR"
            ]
        );
        // Not the shape: a parameter, a result, a lowercase letter after `Test`.
        let src = format!(
            "package x\n\n{}{}{}",
            method("Elsewhere", "TestP(n int)"),
            method("Elsewhere", "TestR() error"),
            method("Elsewhere", "Testify()")
        );
        assert_eq!(names(&src), Vec::<String>::new());
        // With no method of the shape, a type the file cannot settle is not read as a
        // suite: a call on its receiver is no assertion.
        let src = format!(
            "{src}type Remote struct {{\n\thelp.Harness\n}}\n\n{}",
            method("Remote", "check()")
        );
        let unshaped = go_facts(&src);
        assert_eq!(
            unshaped.test_helpers.len(),
            4,
            "{:?}",
            unshaped.test_helpers
        );
        assert!(
            unshaped.test_helpers.iter().all(|h| h.total_asserts == 0),
            "{:?}",
            unshaped.test_helpers
        );
        // Settled as no suite: a plain struct, a named non-struct type, and two structs
        // that embed each other.
        let src = format!("package x\n\ntype Plain struct {{\n\tn int\n}}\n\ntype Count int\n\ntype A struct {{\n\tB\n}}\n\ntype B struct {{\n\tA\n}}\n\n{}{}{}", method("Plain", "TestA()"), method("Count", "TestB()"), method("A", "TestC()"));
        assert_eq!(names(&src), Vec::<String>::new());
        // There, a call on the receiver is no assertion either.
        let facts = go_facts(&src);
        assert!(
            facts.test_helpers.iter().all(|h| h.total_asserts == 0),
            "{:?}",
            facts.test_helpers
        );
    }

    /// Control: an assertion method called on something other than the suite's receiver
    /// is not an assertion, and a plain test function is read as before.
    #[test]
    fn only_the_suite_receiver_carries_assertions() {
        let src = format!("{SUITE}func (s *Suite) TestAdd() {{\n\tother.Equal(1, 2)\n\tv.Require().NoError(err)\n\ts.db.Len(3)\n\ts.Other().Equal(1, 2)\n}}\n\nfunc TestPlain(t *testing.T) {{\n\ts := newThing()\n\ts.Equal(1, 2)\n\tassert.Equal(t, 1, one())\n}}\n");
        let facts = go_facts(&src);
        let counts: Vec<(&str, usize)> = facts
            .tests
            .iter()
            .map(|t| (t.name.as_str(), t.total_asserts))
            .collect();
        assert_eq!(counts, vec![("Suite.TestAdd", 0), ("TestPlain", 1)]);
    }
}
