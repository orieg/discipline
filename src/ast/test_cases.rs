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
//! Non-literal sources (fixtures, generators, dynamic function calls, a list holding a
//! spread or splat element) yield no count and set `non_literal`, so gate notes report
//! that they were not compared.
//!
//! A literal source also yields its rows as text (`row_text`), so a case that leaves one
//! test can be looked for in another by what it is, not by how many there are.

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

/// What a test's case sources say: how many literal cases it runs, and what they are.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CaseList {
    /// The number of literal cases; `None` with no case source, or with one that cannot
    /// be counted.
    pub count: Option<usize>,
    /// A case source is there and its cases cannot be counted from the source.
    pub non_literal: bool,
    /// Each literal case as text (`row_text`), `count` of them. `None` when there is no
    /// count, and for a product of sources larger than `ROW_PRODUCT_LIMIT`.
    pub rows: Option<Vec<String>>,
}

/// The largest product of case sources whose combinations are written out as rows.
const ROW_PRODUCT_LIMIT: usize = 4096;

impl CaseList {
    /// No case source.
    pub fn none() -> Self {
        Self::default()
    }

    /// A case source whose cases cannot be counted.
    pub fn non_literal() -> Self {
        Self {
            count: None,
            non_literal: true,
            rows: None,
        }
    }

    /// A literal case source with these rows.
    pub fn literal(rows: Vec<String>) -> Self {
        Self {
            count: Some(rows.len()),
            non_literal: false,
            rows: Some(rows),
        }
    }

    /// The three facts a test keeps: `TestFn::cases`, `non_literal_cases`, `case_rows`.
    pub fn into_parts(self) -> (Option<usize>, bool, Option<Vec<String>>) {
        (self.count, self.non_literal, self.rows)
    }
}

/// The tokens of `node` in source order, comments left out. Text the grammar keeps
/// between two children without a node of its own is a token too.
fn row_tokens<'a>(node: Node, src: &'a [u8], out: &mut Vec<&'a str>) {
    if node.kind().ends_with("comment") {
        return;
    }
    if node.child_count() == 0 {
        let token = text(node, src).trim();
        if !token.is_empty() {
            out.push(token);
        }
        return;
    }
    let between = |from: usize, to: usize| -> &'a str {
        src.get(from..to)
            .and_then(|gap| std::str::from_utf8(gap).ok())
            .map_or("", str::trim)
    };
    let mut at = node.start_byte();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let gap = between(at, child.start_byte());
        if !gap.is_empty() {
            out.push(gap);
        }
        row_tokens(child, src, out);
        at = child.end_byte();
    }
    let gap = between(at, node.end_byte());
    if !gap.is_empty() {
        out.push(gap);
    }
}

/// Joins tokens into the text of one case: one space between two tokens, and no comma
/// before a closing bracket or at the end.
fn join_row(tokens: &[&str]) -> String {
    let mut out = String::new();
    for (i, token) in tokens.iter().enumerate() {
        let closes = tokens
            .get(i + 1)
            .is_none_or(|next| matches!(*next, ")" | "]" | "}"));
        if *token == "," && closes {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(token);
    }
    out
}

/// One case as text, from its syntax node: its tokens joined by one space, so that
/// layout, comments and a trailing comma are not part of it. Two rows with the same text
/// are the same case.
pub(super) fn row_text(node: Node, src: &[u8]) -> String {
    let mut tokens = Vec::new();
    row_tokens(node, src, &mut tokens);
    join_row(&tokens)
}

/// Two case lists that multiply: a source on a class and one on its method, a suite's
/// table and a test's. A side that cannot be counted makes the whole uncountable; a side
/// with no case source leaves the other as it is. Each row of the product is a row of
/// one side beside a row of the other.
pub fn multiply_cases(outer: CaseList, inner: CaseList) -> CaseList {
    if outer.non_literal || inner.non_literal {
        return CaseList::non_literal();
    }
    match (outer.count, inner.count) {
        (Some(a), Some(b)) => {
            let count = a.saturating_mul(b);
            let rows = match (&outer.rows, &inner.rows) {
                (Some(left), Some(right)) if count <= ROW_PRODUCT_LIMIT => Some(
                    left.iter()
                        .flat_map(|l| right.iter().map(move |r| format!("{l} \u{d7} {r}")))
                        .collect(),
                ),
                _ => None,
            };
            CaseList {
                count: Some(count),
                non_literal: false,
                rows,
            }
        }
        (Some(_), None) => outer,
        _ => inner,
    }
}

// ============================================================================
// Python
// ============================================================================

/// Reads one call as `pytest.mark.parametrize(names, values)`: `None` when it is another
/// call, a literal list for a list or tuple of values, and a non-literal one when the
/// values are not a literal list. A list holding a splat (`[1, *more]`) stands for a
/// number of values the source does not show, and is not literal.
fn python_parametrize_call(call: Node, src: &[u8]) -> Option<CaseList> {
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
        Some(values) if matches!(values.kind(), "list" | "tuple") => {
            let values = elements(values);
            if values.iter().any(|v| v.kind() == "list_splat") {
                CaseList::non_literal()
            } else {
                CaseList::literal(values.into_iter().map(|v| row_text(v, src)).collect())
            }
        }
        _ => CaseList::non_literal(),
    })
}

/// Folds `parametrize` calls into one count: stacked parametrizations multiply.
fn python_product(calls: impl Iterator<Item = CaseList>) -> CaseList {
    calls.fold(CaseList::none(), multiply_cases)
}

/// Extracts test case count from Python decorators.
///
/// Recognizes `@pytest.mark.parametrize(names, values)`.
/// If values is a list `[...]` or tuple `(...)`, counts elements; a comment between them
/// is not an element.
/// When multiple `@pytest.mark.parametrize` decorators are present,
/// computes the Cartesian product (total test executions).
/// Non-literal values (function calls, identifiers) set `non_literal = true`.
pub fn extract_python_cases(decorators: Option<&[Node]>, src: &[u8]) -> CaseList {
    let Some(decs) = decorators else {
        return CaseList::none();
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
pub fn extract_python_pytestmark_cases(block: Node, src: &[u8]) -> CaseList {
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

/// The rows of a `test.each` tagged template: the lines of its text after the header
/// row, one case each, whatever the number of columns. A line is a line of the template's
/// own text: a line break inside a `${..}` value is part of that value, not a row. A
/// template holding only its header runs no case, and one with no text has no table.
fn javascript_template_rows(template: Node, src: &[u8]) -> Option<Vec<String>> {
    fn add_text(fragment: &str, lines: &mut Vec<String>) {
        for (i, part) in fragment.split('\n').enumerate() {
            if i > 0 {
                lines.push(String::new());
            }
            if let Some(line) = lines.last_mut() {
                line.push_str(part);
            }
        }
    }
    let mut lines = vec![String::new()];
    let between = |from: usize, to: usize| -> &str {
        src.get(from..to)
            .and_then(|gap| std::str::from_utf8(gap).ok())
            .unwrap_or("")
    };
    let mut at = template.start_byte();
    let mut cursor = template.walk();
    for child in template.children(&mut cursor) {
        add_text(between(at, child.start_byte()), &mut lines);
        at = child.end_byte();
        match child.kind() {
            "`" => {}
            "template_substitution" => {
                if let Some(line) = lines.last_mut() {
                    line.push(' ');
                    line.push_str(&row_text(child, src));
                    line.push(' ');
                }
            }
            _ => add_text(text(child, src), &mut lines),
        }
    }
    add_text(between(at, template.end_byte()), &mut lines);
    let mut rows = lines
        .iter()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty());
    // The first line is the header row (`a | b | expected`), the rest are cases.
    rows.next()?;
    Some(rows.collect())
}

/// The rows of a `test.each([..])` array. An array holding a spread (`[row, ...more]`)
/// stands for a number of rows the source does not show, and is not literal.
fn javascript_array_rows(array: Node, src: &[u8]) -> CaseList {
    let rows = elements(array);
    if rows.iter().any(|row| row.kind() == "spread_element") {
        CaseList::non_literal()
    } else {
        CaseList::literal(rows.into_iter().map(|row| row_text(row, src)).collect())
    }
}

/// Extracts test cases from JS/TS `test.each(...)`, `it.each(...)`, `describe.each(...)`.
///
/// In JS AST: `test.each([...])("title", fn)` has an outer call whose `function` child
/// is an inner call `test.each([...])` or a tagged template `test.each`\`...\`.
/// A comment between the rows of the array is not a row.
pub fn extract_javascript_cases(func_node: Node, src: &[u8]) -> CaseList {
    if func_node.kind() == "call_expression" {
        let mut cursor = func_node.walk();
        let children: Vec<Node> = func_node.children(&mut cursor).collect();
        if let Some(first) = children.first() {
            let fn_text = text(*first, src);
            if fn_text.ends_with(".each") || fn_text == "each" {
                if let Some(template) = children.iter().find(|c| c.kind() == "template_string") {
                    if let Some(rows) = javascript_template_rows(*template, src) {
                        return CaseList::literal(rows);
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
                            return javascript_array_rows(*arg, src);
                        } else {
                            return CaseList::non_literal();
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
                    return CaseList::literal(rows);
                }
            }
        }
    }
    CaseList::none()
}

// ============================================================================
// Go
// ============================================================================

/// How a Go composite literal's element type reads as the row type of a case table.
#[derive(Clone, Copy, PartialEq)]
enum GoRows {
    /// Not rows: a scalar, an unknown type, or the empty struct of a set.
    No,
    /// A struct written in place, or a type whose name says it holds cases: a table
    /// wherever it stands.
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

/// The words of a Go identifier: `testCase` is `test`, `case`; `HTTPCases` is `http`,
/// `cases`; `Showcase` is the one word `showcase`.
fn go_name_words(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut words = vec![String::new()];
    for (i, c) in chars.iter().enumerate() {
        if !c.is_alphabetic() {
            words.push(String::new());
            continue;
        }
        let after_lower = i > 0 && chars[i - 1].is_lowercase();
        let ends_acronym = i > 0
            && chars[i - 1].is_uppercase()
            && chars.get(i + 1).is_some_and(|next| next.is_lowercase());
        if c.is_uppercase() && (after_lower || ends_acronym) {
            words.push(String::new());
        }
        if let Some(word) = words.last_mut() {
            word.extend(c.to_lowercase());
        }
    }
    words.retain(|word| !word.is_empty());
    words
}

/// The identifier that names an element type: `T` of `T`, `*T`, `pkg.T` and `T[K]`.
fn go_type_name(node: Node) -> Option<Node> {
    match node.kind() {
        "type_identifier" => Some(node),
        "pointer_type" | "parenthesized_type" => elements(node).into_iter().find_map(go_type_name),
        "qualified_type" => node.child_by_field_name("name"),
        "generic_type" => node.child_by_field_name("type").and_then(go_type_name),
        _ => None,
    }
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
        // The words that name a row of cases: a slice of `testCase` or `tests`, a map of
        // `caseRow`. A word is a whole word of the type's name, so `Showcase` and
        // `Contestant` are not rows.
        let (row, words): (Node, &[&str]) = match type_node.kind() {
            "slice_type" | "array_type" => {
                let Some(elem) = type_node.child_by_field_name("element") else {
                    return GoRows::No;
                };
                (
                    elem,
                    &["case", "cases", "test", "tests", "testcase", "testcases"],
                )
            }
            "map_type" => {
                let Some(value) = type_node.child_by_field_name("value") else {
                    return GoRows::No;
                };
                (value, &["case", "cases", "testcase", "testcases"])
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
        let Some(name) = go_type_name(row) else {
            return GoRows::No;
        };
        let by_name = go_name_words(text(name, src))
            .iter()
            .any(|word| words.contains(&word.as_str()));
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

    /// Collects the rows of every case table under `node`.
    fn count_tables(&self, node: Node, total: &mut Option<Vec<String>>, non_literal: &mut bool) {
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
                    total.get_or_insert_with(Vec::new).extend(
                        elements(body)
                            .into_iter()
                            .map(|row| row_text(row, self.src)),
                    );
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
pub fn extract_go_cases(body_node: Node, src: &[u8]) -> CaseList {
    let file = GoFile::new(body_node, src);
    let mut table_cases: Option<Vec<String>> = None;
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

    match table_cases {
        Some(rows) => CaseList::literal(rows),
        None if has_non_literal => CaseList::non_literal(),
        None => CaseList::none(),
    }
}

// ============================================================================
// Java
// ============================================================================

/// The rows of a `textBlock` string, as written in the source with its delimiters: its
/// lines that are neither blank nor a comment. JUnit reads a line starting with `#` as a
/// comment, so a row behind one no longer runs. A string that is not a text block is
/// one row.
fn text_block_rows(literal: &str) -> Vec<String> {
    let block = literal
        .strip_prefix("\"\"\"")
        .and_then(|rest| rest.strip_suffix("\"\"\""));
    match block {
        Some(block) => block
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(str::to_string)
            .collect(),
        None => vec![literal.to_string()],
    }
}

/// The values a Java annotation element holds: the elements of an array initialiser, or
/// the one value written without braces. A name (`ROWS`, `Fixtures.ROWS`) stands for a
/// value the annotation does not show.
fn java_element_rows(value: Node, src: &[u8]) -> Result<Vec<String>, ()> {
    match value.kind() {
        "element_value_array_initializer" | "array_initializer" => Ok(elements(value)
            .into_iter()
            .map(|v| row_text(v, src))
            .collect()),
        "identifier" | "field_access" | "scoped_identifier" => Err(()),
        _ => Ok(vec![row_text(value, src)]),
    }
}

/// Reads one Java annotation: `Ok(Some(rows))` for a literal case source and its cases,
/// `Ok(None)` for an annotation that is not a case source, `Err(())` for a case source
/// whose cases cannot be counted from the source.
fn java_annotation_cases(annotation: Node, src: &[u8]) -> Result<Option<Vec<String>>, ()> {
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
        "NullSource" | "EmptySource" => Ok(Some(vec![format!("@{name}")])),
        "NullAndEmptySource" => Ok(Some(vec![
            "@NullSource".to_string(),
            "@EmptySource".to_string(),
        ])),
        // `@ValueSource(ints = {1, 2})`: one typed array, whichever its name.
        "ValueSource" => match args.as_slice() {
            [(_, value)] => java_element_rows(*value, src).map(Some),
            _ => Err(()),
        },
        "CsvSource" => {
            let mut rows = Vec::new();
            let mut found = false;
            for (key, value) in &args {
                match key {
                    // `@CsvSource({"a,1", "b,2"})` and `@CsvSource(value = {..})`.
                    None | Some("value") => {
                        rows.extend(match value.kind() {
                            "string_literal" => text_block_rows(text(*value, src)),
                            _ => java_element_rows(*value, src)?,
                        });
                        found = true;
                    }
                    Some("textBlock") => match value.kind() {
                        "string_literal" => {
                            rows.extend(text_block_rows(text(*value, src)));
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
                Some((_, names)) if !has_mode => java_element_rows(*names, src).map(Some),
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
pub fn extract_java_cases(modifiers_node: Node, src: &[u8]) -> CaseList {
    let mut total_cases: Option<Vec<String>> = None;
    let mut has_non_literal = false;

    for child in elements(modifiers_node) {
        if child.kind() == "annotation" || child.kind() == "marker_annotation" {
            match java_annotation_cases(child, src) {
                Ok(Some(rows)) => total_cases.get_or_insert_with(Vec::new).extend(rows),
                Ok(None) => {}
                Err(()) => has_non_literal = true,
            }
        }
    }

    sum_of_sources(total_cases, has_non_literal)
}

/// The cases of the sources on one test, which add up; one source that cannot be
/// counted makes the whole uncountable.
fn sum_of_sources(rows: Option<Vec<String>>, has_non_literal: bool) -> CaseList {
    match rows {
        _ if has_non_literal => CaseList::non_literal(),
        Some(rows) => CaseList::literal(rows),
        None => CaseList::none(),
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

/// The elements of a literal Kotlin array: `[a, b]`, or a call of `arrayOf` or one of its
/// typed forms (`intArrayOf(1, 2)`). Any other expression (a constant, another call) has
/// no elements that can be read from the source.
fn kotlin_literal_array_rows(expr: Node, src: &[u8]) -> Option<Vec<String>> {
    let texts = |nodes: Vec<Node>| nodes.into_iter().map(|n| row_text(n, src)).collect();
    match expr.kind() {
        "collection_literal" => Some(texts(kotlin_elements(expr))),
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
            all_plain.then(|| texts(elements))
        }
        _ => None,
    }
}

/// The rows of a `textBlock` string: its lines that are neither blank nor a comment
/// (JUnit reads a line starting with `#` as one).
fn kotlin_text_block_rows(literal: Node, src: &[u8]) -> Vec<String> {
    text(literal, src)
        .trim_matches('"')
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// Reads one Kotlin annotation: `Ok(Some(rows))` for a literal case source and its cases,
/// `Ok(None)` for an annotation that is not a case source, `Err(())` for a case source
/// whose cases cannot be counted from the source.
///
/// In the bundled grammar an annotation with arguments is
/// `annotation > constructor_invocation > (user_type, value_arguments > value_argument*)`,
/// and one without is `annotation > user_type`.
fn kotlin_annotation_cases(annotation: Node, src: &[u8]) -> Result<Option<Vec<String>>, ()> {
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
            Some(source @ ("NullSource" | "EmptySource")) => Ok(Some(vec![format!("@{source}")])),
            Some("NullAndEmptySource") => Ok(Some(vec![
                "@NullSource".to_string(),
                "@EmptySource".to_string(),
            ])),
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
            [(Some(_), Some(value))] => kotlin_literal_array_rows(*value, src).map(Some).ok_or(()),
            _ => Err(()),
        },
        "CsvSource" => {
            let mut rows = Vec::new();
            let mut found = false;
            for (arg_name, value) in &args {
                let Some(value) = value else { return Err(()) };
                match arg_name {
                    // `@CsvSource("a,1", "b,2")`: each positional string is one row.
                    None => match value.kind() {
                        "string_literal" | "multiline_string_literal" => {
                            rows.push(row_text(*value, src));
                            found = true;
                        }
                        // One positional array is the whole `value`.
                        _ => {
                            rows.extend(kotlin_literal_array_rows(*value, src).ok_or(())?);
                            found = true;
                        }
                    },
                    Some("value") => {
                        rows.extend(kotlin_literal_array_rows(*value, src).ok_or(())?);
                        found = true;
                    }
                    Some("textBlock") => match value.kind() {
                        "string_literal" | "multiline_string_literal" => {
                            rows.extend(kotlin_text_block_rows(*value, src));
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
pub fn extract_kotlin_cases(node: Node, src: &[u8]) -> CaseList {
    let mut total_cases: Option<Vec<String>> = None;
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
            Ok(Some(rows)) => total_cases.get_or_insert_with(Vec::new).extend(rows),
            Ok(None) => {}
            Err(()) => has_non_literal = true,
        }
    }

    sum_of_sources(total_cases, has_non_literal)
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
pub fn extract_csharp_cases(method_node: Node, src: &[u8]) -> CaseList {
    let mut rows = Vec::new();
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
                                    // The row is the attribute's arguments, whichever
                                    // framework's attribute carries them.
                                    rows.push(
                                        elements(attr)
                                            .into_iter()
                                            .filter(|c| c.kind() == "attribute_argument_list")
                                            .map(|list| row_text(list, src))
                                            .collect::<Vec<_>>()
                                            .join(" "),
                                    );
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

    sum_of_sources(found.then_some(rows), has_non_literal)
}

// ============================================================================
// Rust
// ============================================================================

/// The path of a Rust `attribute` as its segments (`rstest::case::name` is `rstest`,
/// `case`, `name`), and its arguments when it has some.
fn rust_attribute_path<'a, 'tree>(
    attr: Node<'tree>,
    src: &'a [u8],
) -> (Vec<&'a str>, Option<Node<'tree>>) {
    fn segments<'a>(path: Node, src: &'a [u8], out: &mut Vec<&'a str>) {
        match path.kind() {
            "scoped_identifier" => {
                if let Some(prefix) = path.child_by_field_name("path") {
                    segments(prefix, src, out);
                }
                if let Some(name) = path.child_by_field_name("name") {
                    segments(name, src, out);
                }
            }
            _ => out.push(text(path, src)),
        }
    }
    let mut path = Vec::new();
    let mut arguments = None;
    let mut cursor = attr.walk();
    for child in attr.children(&mut cursor) {
        match child.kind() {
            "identifier" | "scoped_identifier" if path.is_empty() => {
                segments(child, src, &mut path)
            }
            "token_tree" => arguments = Some(child),
            _ => {}
        }
    }
    (path, arguments)
}

/// The arguments of a Rust `attribute` when it is one case of its function: `#[case(..)]`
/// of `rstest` (also `#[case::name(..)]`, `#[rstest::case(..)]` and
/// `#[rstest::case::name(..)]`), or `#[test_case(..)]` of the `test-case` crate, which
/// needs its arguments to be a case. The attribute is told by its path, segment by
/// segment: `#[case_x(..)]` is another attribute.
fn rust_case_row(attr: Node, src: &[u8]) -> Option<String> {
    let (path, arguments) = rust_attribute_path(attr, src);
    let unqualified = match path.as_slice() {
        ["rstest", rest @ ..] => rest,
        all => all,
    };
    let row = || arguments.map_or_else(String::new, |args| row_text(args, src));
    match unqualified {
        ["case"] | ["case", _] => Some(row()),
        _ if path.last() == Some(&"test_case") && arguments.is_some() => Some(row()),
        _ => None,
    }
}

/// The values in the token tree of `#[values(..)]`: its comma-separated arguments. A
/// value is one argument however many tokens it takes (`-1`, `f(2)`); a comment is not a
/// token of any, and a trailing comma adds none.
fn rust_values(token_tree: Node, src: &[u8]) -> Vec<String> {
    let mut values: Vec<Vec<&str>> = Vec::new();
    let mut in_value = false;
    let last = token_tree.child_count().saturating_sub(1);
    let mut cursor = token_tree.walk();
    for (i, token) in token_tree.children(&mut cursor).enumerate() {
        match token.kind() {
            // The delimiters of the tree itself.
            "(" | ")" | "[" | "]" | "{" | "}" if i == 0 || i == last => {}
            "line_comment" | "block_comment" => {}
            "," => in_value = false,
            _ => {
                if !in_value {
                    values.push(Vec::new());
                }
                in_value = true;
                if let Some(value) = values.last_mut() {
                    row_tokens(token, src, value);
                }
            }
        }
    }
    values.iter().map(|tokens| join_row(tokens)).collect()
}

/// Extracts test cases from Rust outer attributes and arguments (`rstest`, `test-case`).
///
/// Recognizes `#[case(...)]` and `#[test_case(...)]` attribute count on function.
/// Also handles combinations of `#[values(...)]` in argument attributes, which multiply
/// each other and the cases they sit beside.
pub fn extract_rust_cases(fn_node: Node, src: &[u8]) -> CaseList {
    let mut cases: Vec<String> = Vec::new();
    let mut values = CaseList::none();

    // Check preceding attribute_item siblings
    let mut prev = fn_node.prev_sibling();
    while let Some(p) = prev {
        match p.kind() {
            "attribute_item" => {
                let mut sub_cursor = p.walk();
                for attr in p.children(&mut sub_cursor) {
                    if attr.kind() == "attribute" {
                        cases.extend(rust_case_row(attr, src));
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
                if attr.kind() == "attribute" {
                    cases.extend(rust_case_row(attr, src));
                }
            }
        } else if child.kind() == "parameters" {
            let mut param_cursor = child.walk();
            for p_child in child.children(&mut param_cursor) {
                if p_child.kind() == "attribute_item" {
                    let mut a_cursor = p_child.walk();
                    for attr in p_child.children(&mut a_cursor) {
                        if attr.kind() != "attribute" {
                            continue;
                        }
                        // `#[values(..)]` and `#[rstest::values(..)]`, by path:
                        // `#[values_x(..)]` is another attribute.
                        let (path, arguments) = rust_attribute_path(attr, src);
                        if !matches!(path.as_slice(), ["values"] | ["rstest", "values"]) {
                            continue;
                        }
                        if let Some(args) = arguments {
                            let of_argument = rust_values(args, src);
                            if !of_argument.is_empty() {
                                values = multiply_cases(values, CaseList::literal(of_argument));
                            }
                        }
                    }
                }
            }
        }
    }

    if cases.is_empty() {
        values
    } else {
        multiply_cases(CaseList::literal(cases), values)
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
        let tree = crate::ast::source_text::parse(&mut parser, code).unwrap();
        let root = tree.root_node();
        let fn_node = root.child(0).unwrap();
        let mut decorators = Vec::new();
        let mut cursor = fn_node.walk();
        for child in fn_node.children(&mut cursor) {
            if child.kind() == "decorator" {
                decorators.push(child);
            }
        }
        let CaseList {
            count: cases,
            non_literal,
            ..
        } = extract_python_cases(Some(&decorators), code.as_bytes());
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
        let tree = crate::ast::source_text::parse(&mut parser, code).unwrap();
        let root = tree.root_node();
        let fn_node = root.child(0).unwrap();
        let mut decorators = Vec::new();
        let mut cursor = fn_node.walk();
        for child in fn_node.children(&mut cursor) {
            if child.kind() == "decorator" {
                decorators.push(child);
            }
        }
        let CaseList {
            count: cases,
            non_literal,
            ..
        } = extract_python_cases(Some(&decorators), code.as_bytes());
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
        let tree = crate::ast::source_text::parse(&mut parser, code).unwrap();
        let root = tree.root_node();
        let fn_node = root.child(0).unwrap();
        let mut decorators = Vec::new();
        let mut cursor = fn_node.walk();
        for child in fn_node.children(&mut cursor) {
            if child.kind() == "decorator" {
                decorators.push(child);
            }
        }
        let CaseList {
            count: cases,
            non_literal,
            ..
        } = extract_python_cases(Some(&decorators), code.as_bytes());
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
        let tree = crate::ast::source_text::parse(&mut parser, code).unwrap();
        let root = tree.root_node();
        let fn_node = root.child(0).unwrap();
        let mut decorators = Vec::new();
        let mut cursor = fn_node.walk();
        for child in fn_node.children(&mut cursor) {
            if child.kind() == "decorator" {
                decorators.push(child);
            }
        }
        let CaseList {
            count: cases,
            non_literal,
            ..
        } = extract_python_cases(Some(&decorators), code.as_bytes());
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
        let tree = crate::ast::source_text::parse(&mut parser, code).unwrap();
        let root = tree.root_node();
        let expr_stmt = root.child(0).unwrap();
        let outer_call = expr_stmt.child(0).unwrap();
        let func_node = outer_call.child_by_field_name("function").unwrap();
        let CaseList {
            count: cases,
            non_literal,
            ..
        } = extract_javascript_cases(func_node, code.as_bytes());
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
        let tree = crate::ast::source_text::parse(&mut parser, code).unwrap();
        let root = tree.root_node();
        let expr_stmt = root.child(0).unwrap();
        let outer_call = expr_stmt.child(0).unwrap();
        let func_node = outer_call.child_by_field_name("function").unwrap();
        let CaseList {
            count: cases,
            non_literal,
            ..
        } = extract_javascript_cases(func_node, code.as_bytes());
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
        let tree = crate::ast::source_text::parse(&mut parser, code).unwrap();
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
        let CaseList {
            count: cases,
            non_literal,
            ..
        } = extract_javascript_cases(func_node, code.as_bytes());
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
        let tree = crate::ast::source_text::parse(&mut parser, code).unwrap();
        let root = tree.root_node();
        let fn_node = root.child(1).unwrap();
        let body = fn_node.child_by_field_name("body").unwrap();
        let CaseList {
            count: cases,
            non_literal,
            ..
        } = extract_go_cases(body, code.as_bytes());
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
        let tree = crate::ast::source_text::parse(&mut parser, code).unwrap();
        let root = tree.root_node();
        let fn_node = root.child(1).unwrap();
        let body = fn_node.child_by_field_name("body").unwrap();
        let CaseList {
            count: cases,
            non_literal,
            ..
        } = extract_go_cases(body, code.as_bytes());
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
        let tree = crate::ast::source_text::parse(&mut parser, code).unwrap();
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
        let CaseList {
            count: cases,
            non_literal,
            ..
        } = extract_java_cases(modifiers, code.as_bytes());
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
        let tree = crate::ast::source_text::parse(&mut parser, code).unwrap();
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
        let CaseList {
            count: cases,
            non_literal,
            ..
        } = extract_java_cases(modifiers, code.as_bytes());
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
        let tree = crate::ast::source_text::parse(&mut parser, code).unwrap();
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
        let CaseList {
            count: cases,
            non_literal,
            ..
        } = extract_java_cases(modifiers, code.as_bytes());
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
        let tree = crate::ast::source_text::parse(&mut parser, code).unwrap();
        let root = tree.root_node();
        let class_decl = root.child(0).unwrap();
        let class_body = class_decl.child_by_field_name("body").unwrap();
        let mut cursor = class_body.walk();
        let method_decl = class_body
            .children(&mut cursor)
            .find(|c| c.kind() == "method_declaration")
            .unwrap();
        let CaseList {
            count: cases,
            non_literal,
            ..
        } = extract_csharp_cases(method_decl, code.as_bytes());
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
        let tree = crate::ast::source_text::parse(&mut parser, code).unwrap();
        let root = tree.root_node();
        let class_decl = root.child(0).unwrap();
        let class_body = class_decl.child_by_field_name("body").unwrap();
        let mut cursor = class_body.walk();
        let method_decl = class_body
            .children(&mut cursor)
            .find(|c| c.kind() == "method_declaration")
            .unwrap();
        let CaseList {
            count: cases,
            non_literal,
            ..
        } = extract_csharp_cases(method_decl, code.as_bytes());
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
        let tree = crate::ast::source_text::parse(&mut parser, code).unwrap();
        let root = tree.root_node();
        let mut cursor = root.walk();
        let fn_node = root
            .children(&mut cursor)
            .find(|c| c.kind() == "function_item")
            .unwrap();
        let CaseList {
            count: cases,
            non_literal,
            ..
        } = extract_rust_cases(fn_node, code.as_bytes());
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
        let tree = crate::ast::source_text::parse(&mut parser, code).unwrap();
        let root = tree.root_node();
        let mut cursor = root.walk();
        let fn_node = root
            .children(&mut cursor)
            .find(|c| c.kind() == "function_item")
            .unwrap();
        let CaseList {
            count: cases,
            non_literal,
            ..
        } = extract_rust_cases(fn_node, code.as_bytes());
        assert_eq!(cases, Some(6));
        assert!(!non_literal);
    }

    // ------------------------------------------------------------------
    // Case content, spreads, templates and exact names (issue 596)
    // ------------------------------------------------------------------

    /// The first test the language pack for `path` reads in `src`.
    fn first_test(path: &str, src: &str) -> crate::ast::TestFn {
        let registry = crate::ast::default_registry();
        let pack = registry.find_pack(path).expect("a language pack");
        let facts = pack
            .extract(path, src, &crate::ast::AssertVocabulary::default())
            .expect("the source parses");
        facts.tests.first().expect("a test").clone()
    }

    fn rows(path: &str, src: &str) -> Vec<String> {
        first_test(path, src).case_rows.expect("literal rows")
    }

    const PY_LAYOUT_A: &str = "import pytest\n\n@pytest.mark.parametrize(\"a,b\", [(1, 2), (\"x y\", 3)])\ndef test_x(a, b):\n    assert a\n";
    const PY_LAYOUT_B: &str = "import pytest\n\n@pytest.mark.parametrize(\"a,b\", [\n    (1,2,),  # one\n    (\n        \"x y\",  # inside the row\n        3,\n    ),\n])\ndef test_x(a, b):\n    assert a\n";
    const PY_OTHER_STRING: &str = "import pytest\n\n@pytest.mark.parametrize(\"a,b\", [(1, 2), (\"x  y\", 3)])\ndef test_x(a, b):\n    assert a\n";

    #[test]
    fn a_row_is_its_tokens_whatever_the_layout() {
        let a = rows("tests/test_a.py", PY_LAYOUT_A);
        assert_eq!(a, ["( 1 , 2 )", "( \" x y \" , 3 )"]);
        assert_eq!(rows("tests/test_a.py", PY_LAYOUT_B), a);
        // Control: whitespace inside a string is the string's own.
        assert_ne!(rows("tests/test_a.py", PY_OTHER_STRING), a);
    }

    const PY_SPLAT: &str = "import pytest\n\n@pytest.mark.parametrize(\"x\", [1, *MORE])\ndef test_x(x):\n    assert x\n";
    const PY_NESTED_SPLAT: &str = "import pytest\n\n@pytest.mark.parametrize(\"x\", [(1, *MORE), (2,)])\ndef test_x(x):\n    assert x\n";
    const JS_SPREAD: &str = "test.each([[1], ...more])('x', (x) => { expect(x).toBe(1); });\n";
    const JS_NESTED_SPREAD: &str =
        "test.each([[1, ...more], [2]])('x', (x) => { expect(x).toBe(1); });\n";

    #[test]
    fn a_list_holding_a_spread_is_not_a_literal_list() {
        for (path, src) in [("tests/test_a.py", PY_SPLAT), ("a.test.js", JS_SPREAD)] {
            let test = first_test(path, src);
            assert_eq!((test.cases, test.non_literal_cases), (None, true), "{path}");
            assert_eq!(test.case_rows, None, "{path}");
        }
        // Control: a spread inside one row is that row's content.
        for (path, src) in [
            ("tests/test_a.py", PY_NESTED_SPLAT),
            ("a.test.js", JS_NESTED_SPREAD),
        ] {
            let test = first_test(path, src);
            assert_eq!(
                (test.cases, test.non_literal_cases),
                (Some(2), false),
                "{path}"
            );
        }
    }

    const JS_ONE_COLUMN: &str =
        "test.each`\n  n\n  ${1}\n\n  ${2}\n  ${3}\n`('x $n', ({ n }) => { expect(n).toBe(1); });\n";
    const JS_HEADER_ONLY: &str = "test.each`\n  n\n`('x $n', ({ n }) => { expect(n).toBe(1); });\n";
    const JS_VALUE_OVER_LINES: &str = "test.each`\n  a | b\n  ${f(\n    A | B\n  )} | ${2}\n  ${1} | ${3}\n`('x $a', ({ a }) => { expect(a).toBe(1); });\n";

    #[test]
    fn a_template_is_its_header_and_one_case_per_line_after_it() {
        assert_eq!(
            rows("a.test.js", JS_ONE_COLUMN),
            ["${ 1 }", "${ 2 }", "${ 3 }"]
        );
        assert_eq!(first_test("a.test.js", JS_HEADER_ONLY).cases, Some(0));
        // A line break inside a value is not a row of the table.
        assert_eq!(first_test("a.test.js", JS_VALUE_OVER_LINES).cases, Some(2));
    }

    #[test]
    fn go_names_are_split_into_words() {
        for (name, words) in [
            ("testCase", vec!["test", "case"]),
            ("TestCases", vec!["test", "cases"]),
            ("HTTPCase", vec!["http", "case"]),
            ("test_case2", vec!["test", "case"]),
            ("Showcase", vec!["showcase"]),
            ("Contestant", vec!["contestant"]),
            ("structured", vec!["structured"]),
        ] {
            assert_eq!(go_name_words(name), words, "{name}");
        }
    }

    /// A Go test whose table is a slice of `@T@`, walked by index.
    const GO_TABLE_OF: &str = "package p\n\nimport \"testing\"\n\nfunc TestX(t *testing.T) {\n\trows := @T@{{1}, {2}, {3}}\n\tfor i := 0; i < len(rows); i++ {\n\t\tif rows[i].n < 0 {\n\t\t\tt.Fatal(i)\n\t\t}\n\t}\n}\n";

    #[test]
    fn go_row_type_is_named_for_cases_by_a_whole_word() {
        for ty in [
            "[]testCase",
            "[]*testCase",
            "[]pkg.TestCase",
            "[3]addTest",
            "map[string]caseRow",
        ] {
            let src = GO_TABLE_OF.replace("@T@", ty);
            assert_eq!(first_test("x_test.go", &src).cases, Some(3), "{ty}");
        }
        for ty in [
            "[]Showcase",
            "[]Contestant",
            "[]structured",
            "map[string]Showcase",
            // A map's rows are named for cases, not for tests.
            "map[string]addTest",
        ] {
            let src = GO_TABLE_OF.replace("@T@", ty);
            assert_eq!(first_test("x_test.go", &src).cases, None, "{ty}");
        }
    }

    /// An `rstest` test with `@ATTRS@` on the function and `@PARAM@` on its argument.
    const RUST_RSTEST: &str =
        "#[rstest]\n@ATTRS@\nfn t(@PARAM@ a: i32) {\n    assert!(a > 0);\n}\n";

    #[test]
    fn rust_case_and_values_are_told_by_their_path() {
        let cases = |attrs: &str, param: &str| {
            let src = RUST_RSTEST
                .replace("@ATTRS@", attrs)
                .replace("@PARAM@", param);
            first_test("tests/a.rs", &src).cases
        };
        assert_eq!(cases("#[case(1)]\n#[case::two(2)]", "#[case]"), Some(2));
        assert_eq!(
            cases("#[rstest::case(1)]\n#[rstest::case::two(2)]", "#[case]"),
            Some(2)
        );
        assert_eq!(
            cases("#[test_case(1)]\n#[test_case::test_case(2)]", ""),
            Some(2)
        );
        assert_eq!(cases("", "#[values(1, 2, 3)]"), Some(3));
        assert_eq!(cases("", "#[rstest::values(1, 2, 3)]"), Some(3));
        // Other attributes whose name starts the same way.
        assert_eq!(
            cases("#[case_x(1)]\n#[cases(2)]\n#[casey::case(3)]", ""),
            None
        );
        assert_eq!(cases("", "#[values_x(1, 2, 3)]"), None);
        assert_eq!(cases("", "#[other::values(1, 2, 3)]"), None);
    }

    const PY_STACKED: &str = "import pytest\n\n@pytest.mark.parametrize(\"x\", [1, 2])\n@pytest.mark.parametrize(\"y\", [\"a\", \"b\"])\ndef test_x(x, y):\n    assert x\n";

    #[test]
    fn rows_of_a_product_are_its_combinations() {
        let mut product = rows("tests/test_a.py", PY_STACKED);
        product.sort();
        assert_eq!(product.len(), 4);
        assert!(
            product.contains(&"1 \u{d7} \" a \"".to_string()),
            "{product:?}"
        );
        assert!(
            product.contains(&"2 \u{d7} \" b \"".to_string()),
            "{product:?}"
        );

        // Past the limit the count stays and the rows are not written out.
        let wide = CaseList::literal((0..100).map(|i| i.to_string()).collect());
        let product = multiply_cases(wide.clone(), wide);
        assert_eq!(product.count, Some(10_000));
        assert_eq!(product.rows, None);
    }

    const JAVA_SOURCES: &str = "class ATest {\n    @ParameterizedTest\n    @NullAndEmptySource\n    @ValueSource(strings = {\"a\", \"b\"})\n    @CsvSource(textBlock = \"\"\"\n        x, 1\n        # y, 2\n        z, 3\n        \"\"\")\n    void t(String s) {\n        assertNotNull(s);\n    }\n}\n";
    const CSHARP_ROWS: &str = "public class ATest {\n    [Theory]\n    [InlineData(1, 2)]\n    [InlineData( 3,4 )]\n    public void T(int a, int b) {\n        Assert.Equal(a, b);\n    }\n}\n";
    const KOTLIN_ROWS: &str = "class ATest {\n    @ParameterizedTest\n    @ValueSource(ints = [1, 2])\n    fun t(x: Int) {\n        assertEquals(1, x)\n    }\n}\n";
    const GO_ROWS: &str = "package p\n\nimport \"testing\"\n\nfunc TestX(t *testing.T) {\n\tfor _, c := range []struct{ a, b int }{\n\t\t{1, 2}, // first\n\t\t{a: 3, b: 4},\n\t} {\n\t\tif c.a > c.b {\n\t\t\tt.Fatal(c)\n\t\t}\n\t}\n}\n";
    const RUST_ROWS: &str = "#[rstest]\n#[case::first(1, 2)]\n#[case( 3,4, )]\nfn t(#[case] a: i32, #[case] b: i32, #[values(-1, f(2))] c: i32) {\n    assert!(a < b);\n}\n";

    #[test]
    fn every_language_keeps_the_text_of_its_literal_cases() {
        assert_eq!(
            rows("src/test/java/ATest.java", JAVA_SOURCES),
            [
                "@NullSource",
                "@EmptySource",
                "\" a \"",
                "\" b \"",
                "x, 1",
                "z, 3"
            ]
        );
        assert_eq!(
            rows("tests/ATest.cs", CSHARP_ROWS),
            ["( 1 , 2 )", "( 3 , 4 )"]
        );
        assert_eq!(rows("src/test/kotlin/ATest.kt", KOTLIN_ROWS), ["1", "2"]);
        assert_eq!(
            rows("x_test.go", GO_ROWS),
            ["{ 1 , 2 }", "{ a : 3 , b : 4 }"]
        );
        let mut rust = rows("tests/a.rs", RUST_ROWS);
        rust.sort();
        assert_eq!(
            rust,
            [
                "( 1 , 2 ) \u{d7} - 1",
                "( 1 , 2 ) \u{d7} f ( 2 )",
                "( 3 , 4 ) \u{d7} - 1",
                "( 3 , 4 ) \u{d7} f ( 2 )"
            ]
        );
    }
}
