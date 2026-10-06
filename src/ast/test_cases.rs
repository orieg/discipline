//! Parametrized and table-driven test case counting.
//!
//! Tracks literal test case tables across supported ecosystems:
//! - Python: `@pytest.mark.parametrize` list / tuple literals, on a test, its class, or
//!   a `pytestmark` of its class or module
//! - JS / TS: `test.each([...])`, `it.each`, `describe.each`
//! - Go: `[]struct{...}{...}` composite literals in the test body and in the
//!   package-level variables it names
//! - Java / Kotlin: `@ValueSource` array lengths, `@CsvSource` rows, null and empty
//!   sources, `@EnumSource` names
//! - C#: `[InlineData]` / `[TestCase]` / `[DataRow]` attribute counts
//! - Rust: `#[case]` and `#[test_case]` attribute counts, `#[values]` combinations
//!
//! A case turned into a comment is a comment node to every grammar and is never counted.
//!
//! Non-literal sources (fixtures, generators, dynamic function calls) yield `cases = None`
//! and set `non_literal = true`, so gate notes report that they were not compared.

use tree_sitter::Node;

fn text<'a>(node: Node, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

/// The named children of `node` that are not comments: the elements of a list, the
/// arguments of a call. A row turned into a comment is a comment node in every bundled
/// grammar, and is not an element.
fn elements(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|c| !c.kind().ends_with("comment"))
        .collect()
}

/// Two case counts that multiply: a source on a class and one on its method, a suite's
/// table and a test's. A side that cannot be counted makes the whole uncountable; a side
/// with no case source leaves the other as it is.
pub fn multiply_cases(
    outer: (Option<usize>, bool),
    inner: (Option<usize>, bool),
) -> (Option<usize>, bool) {
    match (outer, inner) {
        ((_, true), _) | (_, (_, true)) => (None, true),
        ((Some(a), false), (Some(b), false)) => (Some(a.saturating_mul(b)), false),
        ((Some(n), false), (None, false)) | ((None, false), (Some(n), false)) => (Some(n), false),
        ((None, false), (None, false)) => (None, false),
    }
}

// ============================================================================
// Python
// ============================================================================

/// Reads one call as `pytest.mark.parametrize(names, values)`: `None` when it is another
/// call, `Some(Ok(n))` for a list or tuple of `n` values, `Some(Err(()))` when the values
/// are not a literal list.
fn python_parametrize_call(call: Node, src: &[u8]) -> Option<Result<usize, ()>> {
    if call.kind() != "call" {
        return None;
    }
    let func = call.child_by_field_name("function")?;
    if !text(func, src).ends_with("parametrize") {
        return None;
    }
    let args = call.child_by_field_name("arguments")?;
    let mut values_node = None;
    let mut positional_idx = 0;
    for arg in elements(args) {
        if arg.kind() == "keyword_argument" {
            if let Some(name) = arg.child_by_field_name("name") {
                if text(name, src) == "argvalues" {
                    values_node = arg.child_by_field_name("value");
                    break;
                }
            }
        } else {
            if positional_idx == 1 {
                values_node = Some(arg);
                break;
            }
            positional_idx += 1;
        }
    }
    Some(match values_node {
        Some(values) if matches!(values.kind(), "list" | "tuple") => Ok(elements(values).len()),
        _ => Err(()),
    })
}

/// Folds `parametrize` calls into one count: stacked parametrizations multiply.
fn python_product(calls: impl Iterator<Item = Result<usize, ()>>) -> (Option<usize>, bool) {
    let mut total_cases: usize = 1;
    let mut found = false;
    let mut has_non_literal = false;
    for call in calls {
        found = true;
        match call {
            Ok(count) => total_cases = total_cases.saturating_mul(count),
            Err(()) => has_non_literal = true,
        }
    }
    if has_non_literal {
        (None, true)
    } else if found {
        (Some(total_cases), false)
    } else {
        (None, false)
    }
}

/// Extracts test case count from Python decorators.
///
/// Recognizes `@pytest.mark.parametrize(names, values)`.
/// If values is a list `[...]` or tuple `(...)`, counts elements; a comment between them
/// is not an element.
/// When multiple `@pytest.mark.parametrize` decorators are present,
/// computes the Cartesian product (total test executions).
/// Non-literal values (function calls, identifiers) set `non_literal = true`.
pub fn extract_python_cases(decorators: Option<&[Node]>, src: &[u8]) -> (Option<usize>, bool) {
    let Some(decs) = decorators else {
        return (None, false);
    };
    python_product(decs.iter().flat_map(|dec| {
        // Decorator has an expression, usually a call: `@pytest.mark.parametrize(...)`
        elements(*dec)
            .into_iter()
            .filter_map(|child| python_parametrize_call(child, src))
    }))
}

/// Extracts the case count a `pytestmark = ...` assignment gives every test of its
/// module or class: one `pytest.mark.parametrize(...)` call, or a list or tuple of marks.
/// `block` is the module or the class body; only its own statements are read.
pub fn extract_python_pytestmark_cases(block: Node, src: &[u8]) -> (Option<usize>, bool) {
    let mut calls = Vec::new();
    for statement in elements(block) {
        if statement.kind() != "expression_statement" {
            continue;
        }
        for assignment in elements(statement) {
            if assignment.kind() != "assignment" {
                continue;
            }
            let (Some(left), Some(right)) = (
                assignment.child_by_field_name("left"),
                assignment.child_by_field_name("right"),
            ) else {
                continue;
            };
            if left.kind() != "identifier" || text(left, src) != "pytestmark" {
                continue;
            }
            match right.kind() {
                "list" | "tuple" => calls.extend(
                    elements(right)
                        .into_iter()
                        .filter_map(|mark| python_parametrize_call(mark, src)),
                ),
                _ => calls.extend(python_parametrize_call(right, src)),
            }
        }
    }
    python_product(calls.into_iter())
}

// ============================================================================
// JavaScript / TypeScript
// ============================================================================

/// The rows of a `test.each` tagged template: its table lines after the header row. A
/// template holding only its header runs no case.
fn javascript_template_rows(template: Node, src: &[u8]) -> Option<usize> {
    let lines = text(template, src)
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && l.contains('|'))
        .count();
    // The first table line is the header row (a | b | expected), the rest are cases.
    lines.checked_sub(1)
}

/// Extracts test cases from JS/TS `test.each(...)`, `it.each(...)`, `describe.each(...)`.
///
/// In JS AST: `test.each([...])("title", fn)` has an outer call whose `function` child
/// is an inner call `test.each([...])` or a tagged template `test.each`\`...\`.
/// A comment between the rows of the array is not a row.
pub fn extract_javascript_cases(func_node: Node, src: &[u8]) -> (Option<usize>, bool) {
    if func_node.kind() == "call_expression" {
        let mut cursor = func_node.walk();
        let children: Vec<Node> = func_node.children(&mut cursor).collect();
        if let Some(first) = children.first() {
            let fn_text = text(*first, src);
            if fn_text.ends_with(".each") || fn_text == "each" {
                if let Some(template) = children.iter().find(|c| c.kind() == "template_string") {
                    if let Some(rows) = javascript_template_rows(*template, src) {
                        return (Some(rows), false);
                    }
                }
            }
        }

        if let Some(inner_fn) = func_node.child_by_field_name("function") {
            let fn_text = text(inner_fn, src);
            if fn_text.ends_with(".each") || fn_text == "each" {
                if let Some(args) = func_node.child_by_field_name("arguments") {
                    if let Some(arg) = elements(args).first() {
                        if arg.kind() == "array" {
                            return (Some(elements(*arg).len()), false);
                        } else {
                            return (None, true);
                        }
                    }
                }
            }
        }
    } else if func_node.kind() == "tagged_template_expression" {
        let tag = func_node
            .child_by_field_name("tag")
            .or_else(|| func_node.child(0));
        if let Some(tag) = tag {
            let tag_text = text(tag, src);
            if tag_text.ends_with(".each") || tag_text == "each" {
                let mut cursor = func_node.walk();
                let template = func_node
                    .children(&mut cursor)
                    .find(|c| c.kind() == "template_string");
                if let Some(rows) = template.and_then(|t| javascript_template_rows(t, src)) {
                    return (Some(rows), false);
                }
            }
        }
    }
    (None, false)
}

// ============================================================================
// Go
// ============================================================================

/// How a Go composite literal's element type reads as the row type of a case table.
#[derive(Clone, Copy, PartialEq)]
enum GoRows {
    /// Not rows: a scalar, an unknown type, or the empty struct of a set.
    No,
    /// A struct written in place, or a type named for cases: a table wherever it stands.
    Table,
    /// A struct type declared in this file: a table when the test ranges over it, and
    /// an ordinary value (an expected result, a fixture) when it does not.
    NamedStruct,
}

/// Whether a `struct_type` declares at least one field. `struct{}` carries no data: a
/// `map[string]struct{}` is a set and `[]struct{}` a list of nothing, not rows of cases.
fn go_struct_has_fields(struct_type: Node) -> bool {
    let mut cursor = struct_type.walk();
    let has_fields = struct_type
        .named_children(&mut cursor)
        .filter(|c| c.kind() == "field_declaration_list")
        .any(|list| !elements(list).is_empty());
    has_fields
}

/// What the file a test stands in says about the names its body uses.
struct GoFile<'a, 'tree> {
    src: &'a [u8],
    /// Types declared at package level as a struct with fields.
    struct_types: Vec<&'a str>,
    /// Identifiers the test body ranges over (`for _, c := range cases`).
    ranged: Vec<&'a str>,
    /// Every identifier in the test body.
    used: Vec<&'a str>,
    /// Package-level `var` specifications.
    package_vars: Vec<Node<'tree>>,
}

impl<'a, 'tree> GoFile<'a, 'tree> {
    fn new(body: Node<'tree>, src: &'a [u8]) -> Self {
        let mut file = Self {
            src,
            struct_types: Vec::new(),
            ranged: Vec::new(),
            used: Vec::new(),
            package_vars: Vec::new(),
        };
        let mut root = body;
        while let Some(parent) = root.parent() {
            root = parent;
        }
        // The body itself is the root when a caller hands in a detached block.
        if root.id() != body.id() {
            for decl in elements(root) {
                match decl.kind() {
                    "type_declaration" => {
                        for spec in elements(decl) {
                            let (Some(name), Some(ty)) = (
                                spec.child_by_field_name("name"),
                                spec.child_by_field_name("type"),
                            ) else {
                                continue;
                            };
                            if spec.kind() == "type_spec"
                                && ty.kind() == "struct_type"
                                && go_struct_has_fields(ty)
                            {
                                file.struct_types.push(text(name, src));
                            }
                        }
                    }
                    "var_declaration" => {
                        for spec in elements(decl) {
                            match spec.kind() {
                                "var_spec" => file.package_vars.push(spec),
                                "var_spec_list" => file.package_vars.extend(
                                    elements(spec)
                                        .into_iter()
                                        .filter(|s| s.kind() == "var_spec"),
                                ),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        file.collect_names(body);
        file
    }

    fn collect_names(&mut self, node: Node<'tree>) {
        match node.kind() {
            "identifier" => self.used.push(text(node, self.src)),
            "range_clause" => {
                if let Some(right) = node.child_by_field_name("right") {
                    if right.kind() == "identifier" {
                        self.ranged.push(text(right, self.src));
                    }
                }
            }
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_names(child);
        }
    }

    /// How the element type of a slice, array or map literal reads as a row type.
    fn rows(&self, type_node: Node) -> GoRows {
        let src = self.src;
        let (row, by_name) = match type_node.kind() {
            "slice_type" | "array_type" => {
                let Some(elem) = type_node.child_by_field_name("element") else {
                    return GoRows::No;
                };
                let name = text(elem, src);
                let lower = name.to_lowercase();
                (
                    elem,
                    name.contains("struct") || lower.contains("case") || lower.contains("test"),
                )
            }
            "map_type" => {
                let Some(value) = type_node.child_by_field_name("value") else {
                    return GoRows::No;
                };
                let name = text(value, src);
                (
                    value,
                    name.contains("struct") || name.to_lowercase().contains("case"),
                )
            }
            _ => return GoRows::No,
        };
        if row.kind() == "struct_type" {
            return if go_struct_has_fields(row) {
                GoRows::Table
            } else {
                GoRows::No
            };
        }
        if by_name {
            GoRows::Table
        } else if row.kind() == "type_identifier" && self.struct_types.contains(&text(row, src)) {
            GoRows::NamedStruct
        } else {
            GoRows::No
        }
    }

    /// The variable a composite literal is bound to: `cases` in `cases := []T{..}`,
    /// `var cases = []T{..}` and `cases = []T{..}`.
    fn bound_name(&self, literal: Node) -> Option<&'a str> {
        let list = literal.parent().filter(|p| p.kind() == "expression_list")?;
        let index = elements(list).iter().position(|e| e.id() == literal.id())?;
        let statement = list.parent()?;
        let name = match statement.kind() {
            "short_var_declaration" | "assignment_statement" => {
                let left = statement.child_by_field_name("left")?;
                elements(left).get(index).copied()?
            }
            "var_spec" => {
                let mut cursor = statement.walk();
                let name = statement
                    .children_by_field_name("name", &mut cursor)
                    .nth(index)?;
                name
            }
            _ => return None,
        };
        (name.kind() == "identifier").then(|| text(name, self.src))
    }

    /// Adds up the rows of every case table under `node`.
    fn count_tables(&self, node: Node, total: &mut Option<usize>, non_literal: &mut bool) {
        if node.kind() == "composite_literal" {
            if let (Some(type_node), Some(body)) = (
                node.child_by_field_name("type"),
                node.child_by_field_name("body"),
            ) {
                let is_table = match self.rows(type_node) {
                    GoRows::Table => true,
                    GoRows::NamedStruct => {
                        node.parent().is_some_and(|p| p.kind() == "range_clause")
                            || self
                                .bound_name(node)
                                .is_some_and(|name| self.ranged.contains(&name))
                    }
                    GoRows::No => false,
                };
                if is_table {
                    // The body is a `literal_value`; a row turned into a comment is not
                    // one of its elements.
                    *total = Some(total.unwrap_or(0).saturating_add(elements(body).len()));
                }
            }
        } else if node.kind() == "range_clause" {
            // Check what is being ranged over: `for _, tc := range ...`
            if let Some(right) = node.child_by_field_name("right") {
                if right.kind() == "call_expression" {
                    *non_literal = true;
                }
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.count_tables(child, total, non_literal);
        }
    }
}

/// Extracts test cases from a Go test function body.
///
/// Counts the rows of every table composite literal in the body: `[]struct{...}{...}`,
/// `[...]struct{...}{...}`, or `map[...]struct{...}{...}`, and a slice, array or map of a
/// struct type declared in the same file when the test ranges over it. A package-level
/// `var` holding such a table counts toward every test whose body names it, so a table
/// moved out of the test function keeps its rows. A row turned into a comment is not
/// counted, and neither is a set (`map[string]struct{}`).
/// If a `range` loop in the test iterates over a non-literal (function call or external slice),
/// sets `non_literal = true`.
pub fn extract_go_cases(body_node: Node, src: &[u8]) -> (Option<usize>, bool) {
    let file = GoFile::new(body_node, src);
    let mut table_cases: Option<usize> = None;
    let mut has_non_literal = false;

    file.count_tables(body_node, &mut table_cases, &mut has_non_literal);
    for spec in &file.package_vars {
        let mut cursor = spec.walk();
        let named_in_body = spec
            .children_by_field_name("name", &mut cursor)
            .any(|name| file.used.contains(&text(name, src)));
        if named_in_body {
            // A call ranged over inside a package-level initialiser is not this test's.
            let mut ignored = false;
            file.count_tables(*spec, &mut table_cases, &mut ignored);
        }
    }

    if has_non_literal && table_cases.is_none() {
        (None, true)
    } else {
        (table_cases, false)
    }
}

// ============================================================================
// Java
// ============================================================================

/// The rows of a `textBlock` string, as written in the source with its delimiters: its
/// lines that are neither blank nor a comment. JUnit reads a line starting with `#` as a
/// comment, so a row behind one no longer runs. A string that is not a text block is
/// one row.
fn text_block_rows(literal: &str) -> usize {
    let block = literal
        .strip_prefix("\"\"\"")
        .and_then(|rest| rest.strip_suffix("\"\"\""));
    match block {
        Some(block) => block
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .count(),
        None => 1,
    }
}

/// The number of values a Java annotation element holds: the elements of an array
/// initialiser, or one for a single value written without braces. A name (`ROWS`,
/// `Fixtures.ROWS`) stands for a value the annotation does not show.
fn java_element_len(value: Node) -> Result<usize, ()> {
    match value.kind() {
        "element_value_array_initializer" | "array_initializer" => Ok(elements(value).len()),
        "identifier" | "field_access" | "scoped_identifier" => Err(()),
        _ => Ok(1),
    }
}

/// Reads one Java annotation: `Ok(Some(n))` for a literal case source of `n` cases,
/// `Ok(None)` for an annotation that is not a case source, `Err(())` for a case source
/// whose cases cannot be counted from the source.
fn java_annotation_cases(annotation: Node, src: &[u8]) -> Result<Option<usize>, ()> {
    let Some(name_node) = annotation.child_by_field_name("name") else {
        return Ok(None);
    };
    let name = text(name_node, src);
    let name = name.rsplit('.').next().unwrap_or(name);
    // `(key, value)` of each argument; a positional argument has no key.
    let args: Vec<(Option<&str>, Node)> = annotation
        .child_by_field_name("arguments")
        .map(|list| {
            elements(list)
                .into_iter()
                .filter_map(|arg| {
                    if arg.kind() == "element_value_pair" {
                        let key = arg.child_by_field_name("key").map(|k| text(k, src));
                        arg.child_by_field_name("value").map(|v| (key, v))
                    } else {
                        Some((None, arg))
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    match name {
        "MethodSource" | "CsvFileSource" | "ArgumentsSource" => Err(()),
        "NullSource" | "EmptySource" => Ok(Some(1)),
        "NullAndEmptySource" => Ok(Some(2)),
        // `@ValueSource(ints = {1, 2})`: one typed array, whichever its name.
        "ValueSource" => match args.as_slice() {
            [(_, value)] => java_element_len(*value).map(Some),
            _ => Err(()),
        },
        "CsvSource" => {
            let mut rows = 0usize;
            let mut found = false;
            for (key, value) in &args {
                match key {
                    // `@CsvSource({"a,1", "b,2"})` and `@CsvSource(value = {..})`.
                    None | Some("value") => {
                        rows += match value.kind() {
                            "string_literal" => text_block_rows(text(*value, src)),
                            _ => java_element_len(*value)?,
                        };
                        found = true;
                    }
                    Some("textBlock") => match value.kind() {
                        "string_literal" => {
                            rows += text_block_rows(text(*value, src));
                            found = true;
                        }
                        _ => return Err(()),
                    },
                    // `delimiter`, `nullValues` and the like are not rows.
                    Some(_) => {}
                }
            }
            if found {
                Ok(Some(rows))
            } else {
                Err(())
            }
        }
        // `names` lists the constants to run. With a `mode` it may list the ones to
        // leave out, and without `names` the cases are the enum's own constants:
        // neither can be counted here.
        "EnumSource" => {
            let has_mode = args.iter().any(|(key, _)| *key == Some("mode"));
            match args.iter().find(|(key, _)| *key == Some("names")) {
                Some((_, names)) if !has_mode => java_element_len(*names).map(Some),
                _ => Err(()),
            }
        }
        _ => Ok(None),
    }
}

/// Extracts test cases from Java JUnit 5 annotations on a method (`modifiers` node).
///
/// Recognizes `@ValueSource(strings = {...})`, `@CsvSource({...})` with its `value` and
/// `textBlock` forms, `@EnumSource(names = {...})`, `@NullSource`, `@EmptySource` and
/// `@NullAndEmptySource`. Several literal sources on one method add up, and a value
/// turned into a comment is not counted.
/// `@MethodSource`, `@CsvFileSource`, `@ArgumentsSource`, an `@EnumSource` that does not
/// list its constants, and a source whose value is a name rather than a literal, mark
/// `non_literal = true`.
pub fn extract_java_cases(modifiers_node: Node, src: &[u8]) -> (Option<usize>, bool) {
    let mut total_cases: Option<usize> = None;
    let mut has_non_literal = false;

    for child in elements(modifiers_node) {
        if child.kind() == "annotation" || child.kind() == "marker_annotation" {
            match java_annotation_cases(child, src) {
                Ok(Some(n)) => total_cases = Some(total_cases.unwrap_or(0).saturating_add(n)),
                Ok(None) => {}
                Err(()) => has_non_literal = true,
            }
        }
    }

    if has_non_literal {
        (None, true)
    } else {
        (total_cases, false)
    }
}

// ============================================================================
// Kotlin
// ============================================================================

/// The named children of `node` that are not comments.
fn kotlin_elements(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|c| !c.kind().ends_with("comment"))
        .collect()
}

/// The name a Kotlin `value_argument` is passed under (`ints` in `ints = [1, 2]`), and
/// its value expression. A positional argument has no name; a spread has no value.
fn kotlin_argument<'a, 'tree>(
    arg: Node<'tree>,
    src: &'a [u8],
) -> (Option<&'a str>, Option<Node<'tree>>) {
    let mut cursor = arg.walk();
    let tokens: Vec<&str> = arg.children(&mut cursor).map(|c| c.kind()).collect();
    let parts = kotlin_elements(arg);
    let name = if tokens.contains(&"=") {
        parts.first().map(|n| text(*n, src))
    } else {
        None
    };
    let value = parts
        .get(usize::from(name.is_some())..)
        .and_then(|v| v.last());
    // A spread (`*rows`) stands for a number of values the source does not show. The
    // grammar reads it as a `*` token of the argument or as a `spread_expression`.
    let value = value
        .copied()
        .filter(|v| !tokens.contains(&"*") && v.kind() != "spread_expression");
    (name, value)
}

/// The number of elements of a literal Kotlin array: `[a, b]`, or a call of `arrayOf` or
/// one of its typed forms (`intArrayOf(1, 2)`). Any other expression (a constant, another
/// call) has no count that can be read from the source.
fn kotlin_literal_array_len(expr: Node, src: &[u8]) -> Option<usize> {
    match expr.kind() {
        "collection_literal" => Some(kotlin_elements(expr).len()),
        "call_expression" => {
            let parts = kotlin_elements(expr);
            let callee = parts.first()?;
            if callee.kind() != "identifier" {
                return None;
            }
            let name = text(*callee, src);
            if name != "arrayOf" && !name.ends_with("ArrayOf") {
                return None;
            }
            let args = parts.iter().find(|c| c.kind() == "value_arguments")?;
            let elements = kotlin_elements(*args);
            let all_plain = elements
                .iter()
                .all(|a| matches!(kotlin_argument(*a, src), (None, Some(_))));
            all_plain.then_some(elements.len())
        }
        _ => None,
    }
}

/// The rows of a `textBlock` string: its lines that are neither blank nor a comment
/// (JUnit reads a line starting with `#` as one).
fn kotlin_text_block_rows(literal: Node, src: &[u8]) -> usize {
    text(literal, src)
        .trim_matches('"')
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .count()
}

/// Reads one Kotlin annotation: `Ok(Some(n))` for a literal case source of `n` cases,
/// `Ok(None)` for an annotation that is not a case source, `Err(())` for a case source
/// whose cases cannot be counted from the source.
///
/// In the bundled grammar an annotation with arguments is
/// `annotation > constructor_invocation > (user_type, value_arguments > value_argument*)`,
/// and one without is `annotation > user_type`.
fn kotlin_annotation_cases(annotation: Node, src: &[u8]) -> Result<Option<usize>, ()> {
    let parts = kotlin_elements(annotation);
    let Some(invocation) = parts.iter().find(|c| c.kind() == "constructor_invocation") else {
        // No arguments: a marker such as `@Test`, or a source naming nothing to count.
        let name = parts
            .iter()
            .find(|c| c.kind() == "user_type")
            .and_then(|t| {
                kotlin_elements(*t)
                    .into_iter()
                    .rfind(|c| c.kind() == "identifier")
            })
            .map(|n| text(n, src));
        return match name {
            Some("MethodSource" | "CsvFileSource" | "ArgumentsSource") => Err(()),
            Some("NullSource" | "EmptySource") => Ok(Some(1)),
            Some("NullAndEmptySource") => Ok(Some(2)),
            _ => Ok(None),
        };
    };
    let invocation_parts = kotlin_elements(*invocation);
    let name = invocation_parts
        .iter()
        .find(|c| c.kind() == "user_type")
        .and_then(|t| {
            kotlin_elements(*t)
                .into_iter()
                .rfind(|c| c.kind() == "identifier")
        })
        .map(|n| text(n, src))
        .unwrap_or("");
    let args: Vec<(Option<&str>, Option<Node>)> = invocation_parts
        .iter()
        .find(|c| c.kind() == "value_arguments")
        .map(|a| {
            kotlin_elements(*a)
                .into_iter()
                .filter(|c| c.kind() == "value_argument")
                .map(|c| kotlin_argument(c, src))
                .collect()
        })
        .unwrap_or_default();
    match name {
        "MethodSource" | "CsvFileSource" | "ArgumentsSource" => Err(()),
        // `@ValueSource(ints = [1, 2])`: one typed array, whichever its name.
        "ValueSource" => match args.as_slice() {
            [(Some(_), Some(value))] => kotlin_literal_array_len(*value, src).map(Some).ok_or(()),
            _ => Err(()),
        },
        "CsvSource" => {
            let mut rows = 0usize;
            let mut found = false;
            for (arg_name, value) in &args {
                let Some(value) = value else { return Err(()) };
                match arg_name {
                    // `@CsvSource("a,1", "b,2")`: each positional string is one row.
                    None => match value.kind() {
                        "string_literal" | "multiline_string_literal" => {
                            rows += 1;
                            found = true;
                        }
                        // One positional array is the whole `value`.
                        _ => {
                            rows += kotlin_literal_array_len(*value, src).ok_or(())?;
                            found = true;
                        }
                    },
                    Some("value") => {
                        rows += kotlin_literal_array_len(*value, src).ok_or(())?;
                        found = true;
                    }
                    Some("textBlock") => match value.kind() {
                        "string_literal" | "multiline_string_literal" => {
                            rows += kotlin_text_block_rows(*value, src);
                            found = true;
                        }
                        _ => return Err(()),
                    },
                    // `delimiter`, `nullValues` and the like are not rows.
                    Some(_) => {}
                }
            }
            if found {
                Ok(Some(rows))
            } else {
                Err(())
            }
        }
        _ => Ok(None),
    }
}

/// Extracts test cases from Kotlin annotations on a function.
///
/// Recognizes `@ValueSource(ints = [..])` and its `intArrayOf(..)` / `arrayOf(..)` form,
/// `@CsvSource("..", "..")`, `@CsvSource(value = [..])` and `@CsvSource(textBlock = "..")`.
/// `@NullSource` and `@EmptySource` are one case each and `@NullAndEmptySource` two.
/// Several literal sources on one function add up. `@MethodSource`, `@CsvFileSource`,
/// `@ArgumentsSource`, and a `@ValueSource` / `@CsvSource` whose value is not a literal
/// list, mark `non_literal = true`.
pub fn extract_kotlin_cases(node: Node, src: &[u8]) -> (Option<usize>, bool) {
    let mut total_cases: Option<usize> = None;
    let mut has_non_literal = false;

    let mut annotations = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "annotation" => annotations.push(child),
            "modifiers" => {
                let mut sub_cursor = child.walk();
                annotations.extend(
                    child
                        .children(&mut sub_cursor)
                        .filter(|c| c.kind() == "annotation"),
                );
            }
            _ => {}
        }
    }
    for annotation in annotations {
        match kotlin_annotation_cases(annotation, src) {
            Ok(Some(n)) => total_cases = Some(total_cases.unwrap_or(0).saturating_add(n)),
            Ok(None) => {}
            Err(()) => has_non_literal = true,
        }
    }

    if has_non_literal {
        (None, true)
    } else {
        (total_cases, false)
    }
}

// ============================================================================
// C#
// ============================================================================

/// Whether a C# data-row attribute carries a named argument that stops its row from
/// running: `Skip = ".."` (xUnit), `Ignore = ".."` / `IgnoreReason = ".."` (NUnit),
/// `IgnoreMessage = ".."` (MSTest).
fn csharp_row_is_skipped(attr: Node, src: &[u8]) -> bool {
    elements(attr)
        .into_iter()
        .filter(|c| c.kind() == "attribute_argument_list")
        .flat_map(elements)
        .filter_map(|arg| arg.child_by_field_name("name"))
        .any(|name| {
            matches!(
                text(name, src),
                "Skip" | "Ignore" | "IgnoreReason" | "IgnoreMessage"
            )
        })
}

/// Extracts test cases from C# attribute lists on a method declaration.
///
/// Recognizes `[InlineData(...)]` (xUnit), `[TestCase(...)]` (NUnit) and `[DataRow(...)]`
/// (MSTest). A row whose attribute marks it skipped is not counted.
/// Non-literal attributes `[MemberData]`, `[ClassData]`, `[TestCaseSource]`,
/// `[DynamicData]` set `non_literal = true`.
pub fn extract_csharp_cases(method_node: Node, src: &[u8]) -> (Option<usize>, bool) {
    let mut count = 0;
    let mut found = false;
    let mut has_non_literal = false;

    let mut cursor = method_node.walk();
    for child in method_node.children(&mut cursor) {
        if child.kind() == "attribute_list" {
            let mut attr_cursor = child.walk();
            for attr in child.children(&mut attr_cursor) {
                if attr.kind() == "attribute" {
                    if let Some(name_node) = attr.child_by_field_name("name") {
                        let name = text(name_node, src);
                        let clean = name.strip_suffix("Attribute").unwrap_or(name);
                        match clean {
                            "InlineData" | "TestCase" | "DataRow" => {
                                found = true;
                                if !csharp_row_is_skipped(attr, src) {
                                    count += 1;
                                }
                            }
                            "MemberData" | "ClassData" | "TestCaseSource" | "DynamicData" => {
                                has_non_literal = true;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    if has_non_literal {
        (None, true)
    } else if found {
        (Some(count), false)
    } else {
        (None, false)
    }
}

// ============================================================================
// Rust
// ============================================================================

/// Whether a Rust `attribute` is one case of its function: `#[case(..)]` of `rstest`
/// (also `#[case::name(..)]` and `#[rstest::case(..)]`), or `#[test_case(..)]` of the
/// `test-case` crate, which needs its arguments to be a case.
fn rust_attribute_is_case(attr: Node, src: &[u8]) -> bool {
    let attr_text = text(attr, src);
    if attr_text.starts_with("case") || attr_text.starts_with("rstest::case") {
        return true;
    }
    let mut cursor = attr.walk();
    let mut path = None;
    let mut has_arguments = false;
    for child in attr.children(&mut cursor) {
        match child.kind() {
            "identifier" | "scoped_identifier" if path.is_none() => path = Some(child),
            "token_tree" => has_arguments = true,
            _ => {}
        }
    }
    let Some(path) = path else { return false };
    let name = match path.kind() {
        "scoped_identifier" => path.child_by_field_name("name"),
        _ => Some(path),
    };
    has_arguments && name.is_some_and(|n| text(n, src) == "test_case")
}

/// The number of values in the token tree of `#[values(..)]`: its comma-separated
/// arguments. A value is one argument however many tokens it takes (`-1`, `f(2)`); a
/// comment is not a token of any, and a trailing comma adds none.
fn rust_values_len(token_tree: Node) -> usize {
    let mut count = 0;
    let mut in_value = false;
    let mut cursor = token_tree.walk();
    for token in token_tree.children(&mut cursor) {
        match token.kind() {
            "(" | ")" | "[" | "]" | "{" | "}" | "line_comment" | "block_comment" => {}
            "," => in_value = false,
            _ => {
                if !in_value {
                    count += 1;
                }
                in_value = true;
            }
        }
    }
    count
}

/// Extracts test cases from Rust outer attributes and arguments (`rstest`, `test-case`).
///
/// Recognizes `#[case(...)]` and `#[test_case(...)]` attribute count on function.
/// Also handles combinations of `#[values(...)]` in argument attributes, which multiply
/// each other and the cases they sit beside.
pub fn extract_rust_cases(fn_node: Node, src: &[u8]) -> (Option<usize>, bool) {
    let mut case_count: usize = 0;
    let mut values_product: usize = 1;
    let mut has_values = false;

    // Check preceding attribute_item siblings
    let mut prev = fn_node.prev_sibling();
    while let Some(p) = prev {
        match p.kind() {
            "attribute_item" => {
                let mut sub_cursor = p.walk();
                for attr in p.children(&mut sub_cursor) {
                    if attr.kind() == "attribute" && rust_attribute_is_case(attr, src) {
                        case_count += 1;
                    }
                }
                prev = p.prev_sibling();
            }
            "line_comment" | "block_comment" => {
                prev = p.prev_sibling();
            }
            _ => break,
        }
    }

    let mut cursor = fn_node.walk();
    for child in fn_node.children(&mut cursor) {
        if child.kind() == "attribute_item" {
            let mut sub_cursor = child.walk();
            for attr in child.children(&mut sub_cursor) {
                if attr.kind() == "attribute" && rust_attribute_is_case(attr, src) {
                    case_count += 1;
                }
            }
        } else if child.kind() == "parameters" {
            let mut param_cursor = child.walk();
            for p_child in child.children(&mut param_cursor) {
                if p_child.kind() == "attribute_item" {
                    let mut a_cursor = p_child.walk();
                    for attr in p_child.children(&mut a_cursor) {
                        if attr.kind() == "attribute" {
                            let attr_text = text(attr, src);
                            if attr_text.starts_with("values")
                                || attr_text.starts_with("rstest::values")
                            {
                                let mut arg_cursor = attr.walk();
                                let args = attr
                                    .children(&mut arg_cursor)
                                    .find(|c| c.kind() == "arguments" || c.kind() == "token_tree");
                                if let Some(args) = args {
                                    let count = rust_values_len(args);
                                    if count > 0 {
                                        values_product = values_product.saturating_mul(count);
                                        has_values = true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    match (case_count > 0, has_values) {
        (true, true) => (Some(case_count.saturating_mul(values_product)), false),
        (true, false) => (Some(case_count), false),
        (false, true) => (Some(values_product), false),
        (false, false) => (None, false),
    }
}

// ============================================================================
// Unit Tests (positive, negative, non-literal controls for all languages)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_python_cases_list_literal() {
        let code = r#"
@pytest.mark.parametrize("a,b,s", [(1, 2, 3), (2, 2, 4), (-1, 1, 0)])
def test_add(a, b, s):
    assert a + b == s
"#;
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(code, None).unwrap();
        let root = tree.root_node();
        let fn_node = root.child(0).unwrap();
        let mut decorators = Vec::new();
        let mut cursor = fn_node.walk();
        for child in fn_node.children(&mut cursor) {
            if child.kind() == "decorator" {
                decorators.push(child);
            }
        }
        let (cases, non_literal) = extract_python_cases(Some(&decorators), code.as_bytes());
        assert_eq!(cases, Some(3));
        assert!(!non_literal);
    }

    #[test]
    fn test_python_cases_tuple_literal() {
        let code = r#"
@pytest.mark.parametrize("x", (1, 2, 3, 4))
def test_x(x):
    assert x > 0
"#;
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(code, None).unwrap();
        let root = tree.root_node();
        let fn_node = root.child(0).unwrap();
        let mut decorators = Vec::new();
        let mut cursor = fn_node.walk();
        for child in fn_node.children(&mut cursor) {
            if child.kind() == "decorator" {
                decorators.push(child);
            }
        }
        let (cases, non_literal) = extract_python_cases(Some(&decorators), code.as_bytes());
        assert_eq!(cases, Some(4));
        assert!(!non_literal);
    }

    #[test]
    fn test_python_cases_multiple_stacked_cartesian() {
        let code = r#"
@pytest.mark.parametrize("x", [1, 2])
@pytest.mark.parametrize("y", [10, 20, 30])
def test_xy(x, y):
    assert x * y > 0
"#;
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(code, None).unwrap();
        let root = tree.root_node();
        let fn_node = root.child(0).unwrap();
        let mut decorators = Vec::new();
        let mut cursor = fn_node.walk();
        for child in fn_node.children(&mut cursor) {
            if child.kind() == "decorator" {
                decorators.push(child);
            }
        }
        let (cases, non_literal) = extract_python_cases(Some(&decorators), code.as_bytes());
        assert_eq!(cases, Some(6));
        assert!(!non_literal);
    }

    #[test]
    fn test_python_cases_non_literal() {
        let code = r#"
@pytest.mark.parametrize("x", get_dynamic_cases())
def test_dynamic(x):
    assert x
"#;
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(code, None).unwrap();
        let root = tree.root_node();
        let fn_node = root.child(0).unwrap();
        let mut decorators = Vec::new();
        let mut cursor = fn_node.walk();
        for child in fn_node.children(&mut cursor) {
            if child.kind() == "decorator" {
                decorators.push(child);
            }
        }
        let (cases, non_literal) = extract_python_cases(Some(&decorators), code.as_bytes());
        assert_eq!(cases, None);
        assert!(non_literal);
    }

    #[test]
    fn test_javascript_cases_array_literal() {
        let code = r#"
test.each([[1, 2, 3], [2, 2, 4], [-1, 1, 0]])("add %i %i", (a, b, s) => {
    expect(a + b).toBe(s);
});
"#;
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_javascript::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(code, None).unwrap();
        let root = tree.root_node();
        let expr_stmt = root.child(0).unwrap();
        let outer_call = expr_stmt.child(0).unwrap();
        let func_node = outer_call.child_by_field_name("function").unwrap();
        let (cases, non_literal) = extract_javascript_cases(func_node, code.as_bytes());
        assert_eq!(cases, Some(3));
        assert!(!non_literal);
    }

    #[test]
    fn test_javascript_cases_non_literal() {
        let code = r#"
test.each(getCases())("add %i %i", (a, b, s) => {
    expect(a + b).toBe(s);
});
"#;
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_javascript::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(code, None).unwrap();
        let root = tree.root_node();
        let expr_stmt = root.child(0).unwrap();
        let outer_call = expr_stmt.child(0).unwrap();
        let func_node = outer_call.child_by_field_name("function").unwrap();
        let (cases, non_literal) = extract_javascript_cases(func_node, code.as_bytes());
        assert_eq!(cases, None);
        assert!(non_literal);
    }

    #[test]
    fn test_javascript_cases_tagged_template() {
        let code = r#"
test.each`
  a | b | s
  1 | 2 | 3
  2 | 2 | 4
`("add $a $b", ({ a, b, s }) => {
  expect(a + b).toBe(s);
});
"#;
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_javascript::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(code, None).unwrap();
        let root = tree.root_node();
        let expr_stmt = root.child(0).unwrap();
        let outer_call = expr_stmt.child(0).unwrap();
        let func_node = outer_call.child_by_field_name("function").unwrap();
        eprintln!(
            "JS func_node kind={}, text={}",
            func_node.kind(),
            text(func_node, code.as_bytes())
        );
        let mut c = func_node.walk();
        for child in func_node.children(&mut c) {
            eprintln!(
                "  JS child kind={}, text={}",
                child.kind(),
                text(child, code.as_bytes())
            );
        }
        let (cases, non_literal) = extract_javascript_cases(func_node, code.as_bytes());
        assert_eq!(cases, Some(2));
        assert!(!non_literal);
    }

    #[test]
    fn test_go_cases_slice_of_structs() {
        let code = r#"package p

func TestAdd(t *testing.T) {
	cases := []struct{ a, b, s int }{{1, 2, 3}, {2, 2, 4}, {-1, 1, 0}}
	for _, c := range cases {
		if c.a+c.b != c.s {
			t.Fatalf("bad %v", c)
		}
	}
}
"#;
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_go::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(code, None).unwrap();
        let root = tree.root_node();
        let fn_node = root.child(1).unwrap();
        let body = fn_node.child_by_field_name("body").unwrap();
        let (cases, non_literal) = extract_go_cases(body, code.as_bytes());
        assert_eq!(cases, Some(3));
        assert!(!non_literal);
    }

    #[test]
    fn test_go_cases_non_literal_range() {
        let code = r#"package p

func TestAdd(t *testing.T) {
	for _, c := range getCases() {
		if c.a+c.b != c.s {
			t.Fatalf("bad %v", c)
		}
	}
}
"#;
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_go::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(code, None).unwrap();
        let root = tree.root_node();
        let fn_node = root.child(1).unwrap();
        let body = fn_node.child_by_field_name("body").unwrap();
        let (cases, non_literal) = extract_go_cases(body, code.as_bytes());
        assert_eq!(cases, None);
        assert!(non_literal);
    }

    #[test]
    fn test_java_cases_value_source() {
        let code = r#"
class TestExample {
    @ParameterizedTest
    @ValueSource(strings = {"apple", "banana", "cherry"})
    void testFruits(String fruit) {
        assertNotNull(fruit);
    }
}
"#;
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_java::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(code, None).unwrap();
        let root = tree.root_node();
        let class_decl = root.child(0).unwrap();
        let class_body = class_decl.child_by_field_name("body").unwrap();
        let mut cursor = class_body.walk();
        let method_decl = class_body
            .children(&mut cursor)
            .find(|c| c.kind() == "method_declaration")
            .unwrap();
        let mut m_cursor = method_decl.walk();
        let modifiers = method_decl
            .children(&mut m_cursor)
            .find(|c| c.kind() == "modifiers")
            .unwrap();
        let (cases, non_literal) = extract_java_cases(modifiers, code.as_bytes());
        assert_eq!(cases, Some(3));
        assert!(!non_literal);
    }

    #[test]
    fn test_java_cases_csv_source() {
        let code = r#"
class TestExample {
    @ParameterizedTest
    @CsvSource({"1, 2, 3", "2, 2, 4"})
    void testAdd(int a, int b, int s) {
        assertEquals(s, a + b);
    }
}
"#;
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_java::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(code, None).unwrap();
        let root = tree.root_node();
        let class_decl = root.child(0).unwrap();
        let class_body = class_decl.child_by_field_name("body").unwrap();
        let mut cursor = class_body.walk();
        let method_decl = class_body
            .children(&mut cursor)
            .find(|c| c.kind() == "method_declaration")
            .unwrap();
        let mut m_cursor = method_decl.walk();
        let modifiers = method_decl
            .children(&mut m_cursor)
            .find(|c| c.kind() == "modifiers")
            .unwrap();
        let (cases, non_literal) = extract_java_cases(modifiers, code.as_bytes());
        assert_eq!(cases, Some(2));
        assert!(!non_literal);
    }

    #[test]
    fn test_java_cases_method_source_non_literal() {
        let code = r#"
class TestExample {
    @ParameterizedTest
    @MethodSource("provideStringsForIsBlank")
    void testBlank(String input) {
        assertTrue(input.isBlank());
    }
}
"#;
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_java::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(code, None).unwrap();
        let root = tree.root_node();
        let class_decl = root.child(0).unwrap();
        let class_body = class_decl.child_by_field_name("body").unwrap();
        let mut cursor = class_body.walk();
        let method_decl = class_body
            .children(&mut cursor)
            .find(|c| c.kind() == "method_declaration")
            .unwrap();
        let mut m_cursor = method_decl.walk();
        let modifiers = method_decl
            .children(&mut m_cursor)
            .find(|c| c.kind() == "modifiers")
            .unwrap();
        let (cases, non_literal) = extract_java_cases(modifiers, code.as_bytes());
        assert_eq!(cases, None);
        assert!(non_literal);
    }

    #[test]
    fn test_csharp_cases_inline_data() {
        let code = r#"
public class TestClass {
    [Theory]
    [InlineData(1, 2, 3)]
    [InlineData(2, 2, 4)]
    [InlineData(-1, 1, 0)]
    public void TestAdd(int a, int b, int s) {
        Assert.Equal(s, a + b);
    }
}
"#;
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_c_sharp::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(code, None).unwrap();
        let root = tree.root_node();
        let class_decl = root.child(0).unwrap();
        let class_body = class_decl.child_by_field_name("body").unwrap();
        let mut cursor = class_body.walk();
        let method_decl = class_body
            .children(&mut cursor)
            .find(|c| c.kind() == "method_declaration")
            .unwrap();
        let (cases, non_literal) = extract_csharp_cases(method_decl, code.as_bytes());
        assert_eq!(cases, Some(3));
        assert!(!non_literal);
    }

    #[test]
    fn test_csharp_cases_member_data_non_literal() {
        let code = r#"
public class TestClass {
    [Theory]
    [MemberData(nameof(Data))]
    public void TestAdd(int a, int b, int s) {
        Assert.Equal(s, a + b);
    }
}
"#;
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_c_sharp::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(code, None).unwrap();
        let root = tree.root_node();
        let class_decl = root.child(0).unwrap();
        let class_body = class_decl.child_by_field_name("body").unwrap();
        let mut cursor = class_body.walk();
        let method_decl = class_body
            .children(&mut cursor)
            .find(|c| c.kind() == "method_declaration")
            .unwrap();
        let (cases, non_literal) = extract_csharp_cases(method_decl, code.as_bytes());
        assert_eq!(cases, None);
        assert!(non_literal);
    }

    #[test]
    fn test_rust_cases_case_attributes() {
        let code = r#"
#[rstest]
#[case(1, 2, 3)]
#[case(2, 2, 4)]
#[case(-1, 1, 0)]
fn test_add(#[case] a: i32, #[case] b: i32, #[case] s: i32) {
    assert_eq!(a + b, s);
}
"#;
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(code, None).unwrap();
        let root = tree.root_node();
        let mut cursor = root.walk();
        let fn_node = root
            .children(&mut cursor)
            .find(|c| c.kind() == "function_item")
            .unwrap();
        let (cases, non_literal) = extract_rust_cases(fn_node, code.as_bytes());
        assert_eq!(cases, Some(3));
        assert!(!non_literal);
    }

    #[test]
    fn test_rust_cases_values_attributes() {
        let code = r#"
#[rstest]
fn test_matrix(#[values(1, 2)] a: i32, #[values(10, 20, 30)] b: i32) {
    assert!(a < b);
}
"#;
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(code, None).unwrap();
        let root = tree.root_node();
        let mut cursor = root.walk();
        let fn_node = root
            .children(&mut cursor)
            .find(|c| c.kind() == "function_item")
            .unwrap();
        let (cases, non_literal) = extract_rust_cases(fn_node, code.as_bytes());
        assert_eq!(cases, Some(6));
        assert!(!non_literal);
    }
}
