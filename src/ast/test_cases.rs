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
//!
//! Python, JS / TS array and Go struct tables also yield each row's cells (`CaseRow`), so
//! a row whose one value was rewritten in place can be told from a row dropped or added
//! (`rewritten_cells`).

use super::ancestry::Ancestry;
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
    /// The cells of the rows whose cells are read (`CaseRow`): the rows of the packs
    /// that read them, in a table of this test's own file.
    pub cells: Vec<CaseRow>,
}

/// One cell of a case row: a positional value, or one written under a field or keyword.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// The field or keyword the value is written under (`want` of `{want: 4}`), `None`
    /// for a positional value.
    pub key: Option<String>,
    /// The value as text (`row_text`).
    pub value: String,
    /// The value is one literal: a number, string, boolean, or `None` / `nil` / `null`.
    pub literal: bool,
    /// The line the value starts on.
    pub line: usize,
}

/// The cells of one literal case row, and the table it is a row of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseRow {
    /// What tells one table of a test from another: the table's type for a Go table,
    /// empty where a test has one case source.
    pub table: String,
    pub cells: Vec<Cell>,
}

impl CaseRow {
    /// The row as compared: its table, and each cell's key and value. Layout and lines
    /// are not part of it.
    fn content(&self) -> (&str, Vec<(Option<&str>, &str)>) {
        (
            &self.table,
            self.cells
                .iter()
                .map(|c| (c.key.as_deref(), c.value.as_str()))
                .collect(),
        )
    }
}

/// Lines of `head` case cells whose value was rewritten in place: a head row of a table
/// that equals a base row of the same table in every cell but one, where both values of
/// that cell are literals under the same key.
///
/// Rows equal on both sides are taken out first, one for one, wherever they stand, so a
/// reordered table is unchanged. Of the rows left, a base row and a head row are paired
/// only when each is one cell away from the other and from no other row left: an
/// ambiguous pairing is not reported. A row edited in more than one cell, a row added or
/// removed, and a row of one cell (a case replaced, with no other cell beside it that
/// stayed) are not paired.
pub fn rewritten_cells(base: &[CaseRow], head: &[CaseRow]) -> Vec<usize> {
    use std::collections::HashMap;
    // Equal rows, one for one.
    let mut unmatched: HashMap<_, usize> = HashMap::new();
    for row in base {
        *unmatched.entry(row.content()).or_default() += 1;
    }
    let mut head_left = Vec::new();
    for row in head {
        match unmatched.get_mut(&row.content()) {
            Some(n) if *n > 0 => *n -= 1,
            _ => head_left.push(row),
        }
    }
    let mut seen: HashMap<_, usize> = HashMap::new();
    let mut base_left = Vec::new();
    for row in base {
        let content = row.content();
        let left = unmatched.get(&content).copied().unwrap_or(0);
        let n = seen.entry(content).or_default();
        // The first `left` copies of a row are the ones no head row took.
        if *n < left {
            base_left.push(row);
        }
        *n += 1;
    }
    if base_left.is_empty() || head_left.is_empty() {
        return Vec::new();
    }
    // Two rows left over, of the same table and arity, are one cell apart at `i` when
    // they agree on every other cell: no two of them are equal any more. Each row is
    // filed under each of its cells masked out.
    type Masked<'a> = (&'a str, usize, Vec<(Option<&'a str>, &'a str)>);
    fn masked(row: &CaseRow, i: usize) -> Masked<'_> {
        let (table, mut cells) = row.content();
        cells.remove(i);
        (table, i, cells)
    }
    let mut by_mask: HashMap<Masked, (Vec<usize>, Vec<usize>)> = HashMap::new();
    for (at, row) in base_left
        .iter()
        .enumerate()
        .filter(|(_, r)| r.cells.len() >= 2)
    {
        for i in 0..row.cells.len() {
            by_mask.entry(masked(row, i)).or_default().0.push(at);
        }
    }
    for (at, row) in head_left
        .iter()
        .enumerate()
        .filter(|(_, r)| r.cells.len() >= 2)
    {
        for i in 0..row.cells.len() {
            if let Some(entry) = by_mask.get_mut(&masked(row, i)) {
                entry.1.push(at);
            }
        }
    }
    // For each row left: its partners one cell away, with the cell.
    let mut base_partners: Vec<Vec<(usize, usize)>> = vec![Vec::new(); base_left.len()];
    let mut head_partners: Vec<usize> = vec![0; head_left.len()];
    for ((_, i, _), (bases, heads)) in &by_mask {
        for &b in bases {
            for &h in heads {
                base_partners[b].push((h, *i));
                head_partners[h] += 1;
            }
        }
    }
    let mut lines: Vec<usize> = base_partners
        .iter()
        .enumerate()
        .filter_map(|(b, partners)| match partners.as_slice() {
            [(h, i)] if head_partners[*h] == 1 => {
                let (was, now) = (&base_left[b].cells[*i], &head_left[*h].cells[*i]);
                (was.key == now.key && was.literal && now.literal).then_some(now.line)
            }
            _ => None,
        })
        .collect();
    lines.sort_unstable();
    lines.dedup();
    lines
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
            cells: Vec::new(),
        }
    }

    /// A literal case source with these rows.
    pub fn literal(rows: Vec<String>) -> Self {
        Self {
            count: Some(rows.len()),
            non_literal: false,
            rows: Some(rows),
            cells: Vec::new(),
        }
    }

    /// A literal case source with these rows and their cells.
    fn literal_with_cells(rows: Vec<String>, cells: Vec<CaseRow>) -> Self {
        Self {
            cells,
            ..Self::literal(rows)
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
            // A row of the product is the cells of one side beside those of the other.
            let cells = if outer.cells.len().saturating_mul(inner.cells.len()) <= ROW_PRODUCT_LIMIT
            {
                outer
                    .cells
                    .iter()
                    .flat_map(|l| {
                        inner.cells.iter().map(move |r| CaseRow {
                            table: format!("{} \u{d7} {}", l.table, r.table),
                            cells: l.cells.iter().chain(&r.cells).cloned().collect(),
                        })
                    })
                    .collect()
            } else {
                Vec::new()
            };
            CaseList {
                count: Some(count),
                non_literal: false,
                rows,
                cells,
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
/// What a Python file binds the names `pytest.mark.parametrize` is reached through to.
///
/// `parametrize` is a case source when the name it is reached through is pytest's:
/// `pytest.mark.parametrize` under the name the file imports `pytest` by
/// (`import pytest`, `import pytest as pt`), `mark.parametrize` under the name
/// `from pytest import mark` binds, and a name the file assigns either of those to
/// (`parametrize = pytest.mark.parametrize`, `mark = pytest.mark`). The name `pytest`
/// itself counts in a file that binds it to nothing, as a fragment without its imports
/// reads. A function or method merely named `parametrize` (`helpers.parametrize(..)`,
/// `self.parametrize(..)`, a `pytest` the file defines or imports from elsewhere) is not
/// a case source.
#[derive(Debug, Default, Clone)]
pub struct PytestNames {
    /// Names bound to the `pytest` module.
    modules: Vec<String>,
    /// Names bound to `pytest.mark`.
    marks: Vec<String>,
    /// Names bound to `pytest.mark.parametrize`.
    parametrize: Vec<String>,
    /// Names the file binds to something else. A file binds a name for each function
    /// it defines, and each is looked up among them: they are kept as a set.
    other: std::collections::HashSet<String>,
}

impl PytestNames {
    /// Reads the bindings of the file `root` is the module of: its imports and
    /// assignments outside function and class bodies.
    pub fn read(root: Node, src: &[u8]) -> Self {
        let mut names = Self::default();
        names.read_block(root, src);
        names
    }

    fn bind(&mut self, name: &str, to: Option<usize>) {
        for (at, list) in [&mut self.modules, &mut self.marks, &mut self.parametrize]
            .into_iter()
            .enumerate()
        {
            list.retain(|n| n != name);
            if to == Some(at) {
                list.push(name.to_string());
            }
        }
        super::ancestry::count(1);
        if to.is_none() {
            self.other.insert(name.to_string());
        } else {
            self.other.remove(name);
        }
    }

    /// Whether a dotted expression names the `pytest` module, under a name the file
    /// binds to it or as `pytest` itself.
    pub fn is_module(&self, dotted: &str) -> bool {
        self.resolve(dotted) == Some(0)
    }

    /// What a dotted expression is: `Some(0)` the module, `Some(1)` its `mark`,
    /// `Some(2)` `parametrize`.
    fn resolve(&self, dotted: &str) -> Option<usize> {
        let parts: Vec<&str> = dotted.split('.').map(str::trim).collect();
        let module = |p: &str| {
            self.modules.iter().any(|m| m == p) || (p == "pytest" && !self.other.contains(p))
        };
        let mark = |m: &str| self.marks.iter().any(|n| n == m);
        match parts.as_slice() {
            [p] if self.parametrize.iter().any(|n| n == p) => Some(2),
            [p] if mark(p) => Some(1),
            [p] if module(p) => Some(0),
            [p, "mark"] if module(p) => Some(1),
            [m, "parametrize"] if mark(m) => Some(2),
            [p, "mark", "parametrize"] if module(p) => Some(2),
            _ => None,
        }
    }

    fn read_block(&mut self, block: Node, src: &[u8]) {
        for statement in elements(block) {
            match statement.kind() {
                "import_statement" => {
                    for name in elements(statement) {
                        let (module, bound) = match name.kind() {
                            "aliased_import" => (
                                name.child_by_field_name("name").map(|n| text(n, src)),
                                name.child_by_field_name("alias").map(|n| text(n, src)),
                            ),
                            // `import a.b` binds `a`.
                            _ => (Some(text(name, src)), text(name, src).split('.').next()),
                        };
                        if let (Some(module), Some(bound)) = (module, bound) {
                            let whole = name.kind() == "aliased_import" || !module.contains('.');
                            self.bind(bound, (module == "pytest" && whole).then_some(0));
                        }
                    }
                }
                "import_from_statement" => {
                    let module = statement
                        .child_by_field_name("module_name")
                        .map(|m| text(m, src))
                        .unwrap_or("");
                    let mut cursor = statement.walk();
                    let imported: Vec<Node> = statement
                        .children_by_field_name("name", &mut cursor)
                        .collect();
                    for name in imported {
                        let (item, bound) = match name.kind() {
                            "aliased_import" => (
                                name.child_by_field_name("name").map(|n| text(n, src)),
                                name.child_by_field_name("alias").map(|n| text(n, src)),
                            ),
                            _ => (Some(text(name, src)), Some(text(name, src))),
                        };
                        if let (Some(item), Some(bound)) = (item, bound) {
                            self.bind(bound, (module == "pytest" && item == "mark").then_some(1));
                        }
                    }
                }
                "function_definition" | "class_definition" => {
                    if let Some(name) = statement.child_by_field_name("name") {
                        self.bind(text(name, src), None);
                    }
                }
                "decorated_definition" => {
                    let name = statement
                        .child_by_field_name("definition")
                        .and_then(|d| d.child_by_field_name("name"));
                    if let Some(name) = name {
                        self.bind(text(name, src), None);
                    }
                }
                "expression_statement" => {
                    for assignment in elements(statement) {
                        let (Some(left), Some(right)) = (
                            assignment.child_by_field_name("left"),
                            assignment.child_by_field_name("right"),
                        ) else {
                            continue;
                        };
                        if assignment.kind() == "assignment" && left.kind() == "identifier" {
                            let to = matches!(right.kind(), "identifier" | "attribute")
                                .then(|| self.resolve(text(right, src)))
                                .flatten();
                            self.bind(text(left, src), to);
                        }
                    }
                }
                // Imports under `try:` / `if TYPE_CHECKING:` bind module names too.
                "try_statement" | "if_statement" | "with_statement" | "block" | "else_clause"
                | "elif_clause" | "except_clause" | "finally_clause" => {
                    self.read_block(statement, src);
                }
                _ => {}
            }
        }
    }

    /// Whether `func`, the callee of a call, is pytest's `parametrize`.
    fn is_parametrize(&self, func: Node, src: &[u8]) -> bool {
        matches!(func.kind(), "identifier" | "attribute")
            && self.resolve(text(func, src)) == Some(2)
    }

    /// Whether `call` is `pytest.param(..)` under the name the file binds `pytest` to.
    fn is_param(&self, call: Node, src: &[u8]) -> bool {
        call.child_by_field_name("function")
            .filter(|f| f.kind() == "attribute")
            .is_some_and(|f| {
                f.child_by_field_name("attribute")
                    .is_some_and(|a| text(a, src) == "param")
                    && f.child_by_field_name("object")
                        .is_some_and(|o| self.resolve(text(o, src)) == Some(0))
            })
    }
}

/// One cell: `value` under `key`, read as a literal by `literal`.
fn cell(key: Option<String>, value: Node, src: &[u8], literal: bool) -> Cell {
    Cell {
        key,
        value: row_text(value, src),
        literal,
        line: value.start_position().row + 1,
    }
}

/// The cells of one `parametrize` value: the items of a tuple or list, the arguments of
/// `pytest.param(..)` but its `id` and `marks`, or the value itself.
fn python_row_cells(row: Node, src: &[u8], names: &PytestNames) -> Vec<Cell> {
    let lit = super::expectations::py_lit;
    match row.kind() {
        "tuple" | "list" => elements(row)
            .into_iter()
            .map(|v| cell(None, v, src, lit(v)))
            .collect(),
        "call" if names.is_param(row, src) => {
            let Some(args) = row.child_by_field_name("arguments") else {
                return Vec::new();
            };
            elements(args)
                .into_iter()
                .filter_map(|arg| {
                    if arg.kind() != "keyword_argument" {
                        return Some(cell(None, arg, src, lit(arg)));
                    }
                    let key = text(arg.child_by_field_name("name")?, src);
                    let value = arg.child_by_field_name("value")?;
                    (!matches!(key, "id" | "marks"))
                        .then(|| cell(Some(key.to_string()), value, src, lit(value)))
                })
                .collect()
        }
        _ => vec![cell(None, row, src, lit(row))],
    }
}

fn python_parametrize_call(call: Node, src: &[u8], names: &PytestNames) -> Option<CaseList> {
    if call.kind() != "call" {
        return None;
    }
    let func = call.child_by_field_name("function")?;
    if !names.is_parametrize(func, src) {
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
                let cells = values
                    .iter()
                    .map(|v| CaseRow {
                        table: String::new(),
                        cells: python_row_cells(*v, src, names),
                    })
                    .collect();
                CaseList::literal_with_cells(
                    values.into_iter().map(|v| row_text(v, src)).collect(),
                    cells,
                )
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
pub fn extract_python_cases(
    decorators: Option<&[Node]>,
    src: &[u8],
    names: &PytestNames,
) -> CaseList {
    let Some(decs) = decorators else {
        return CaseList::none();
    };
    python_product(decs.iter().flat_map(|dec| {
        // Decorator has an expression, usually a call: `@pytest.mark.parametrize(...)`
        elements(*dec)
            .into_iter()
            .filter_map(|child| python_parametrize_call(child, src, names))
    }))
}

/// Extracts the case count a `pytestmark = ...` assignment gives every test of its
/// module or class: one `pytest.mark.parametrize(...)` call, or a list or tuple of marks.
/// `block` is the module or the class body; only its own statements are read.
pub fn extract_python_pytestmark_cases(block: Node, src: &[u8], names: &PytestNames) -> CaseList {
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
                        .filter_map(|mark| python_parametrize_call(mark, src, names)),
                ),
                _ => calls.extend(python_parametrize_call(right, src, names)),
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
/// The names of a JavaScript or TypeScript file that are not the test runner's.
///
/// `.each` is a case source when it is the runner's: reached through `it`, `test`,
/// `describe` and their spellings (`it.each`, `describe.skip.each`), where that name is
/// the runner's global, a name the file imports from a runner ([`RUNNER_MODULES`]), or
/// one it derives from such a name (`const test = base.extend({..})`). Under a name the
/// file declares itself, and on any other receiver (`rows.each(..)`,
/// `helpers.each(..)`), `each` is a project function and no case source. Under a name
/// the file imports from another module (`import { test } from './fixtures'`), what it
/// is cannot be told here: the module may hand on the runner's own function, so the
/// test has a case source whose cases are not counted (`non_literal`).
#[derive(Debug, Default, Clone)]
pub struct RunnerNames {
    /// Names the file declares as something that is not a runner's function.
    pub not_runner: Vec<String>,
    /// Names the file imports from a module that is not a runner.
    pub imported: Vec<String>,
}

/// Modules whose `it` / `test` / `describe` are a test runner's.
pub const RUNNER_MODULES: &[&str] = &[
    "vitest",
    "@jest/globals",
    "bun:test",
    "node:test",
    "test",
    "mocha",
    "@playwright/test",
    "jest-circus",
    "@japa/runner",
];

/// The names a runner's test and suite functions go by.
pub const RUNNER_FUNCTIONS: &[&str] = &[
    "it",
    "test",
    "describe",
    "context",
    "suite",
    "fit",
    "xit",
    "xtest",
    "fdescribe",
    "xdescribe",
    "xcontext",
];

impl RunnerNames {
    /// Whether `callee` is a runner's `each`: `<runner function>[.modifier..].each`.
    fn is_each(&self, callee: Node, src: &[u8]) -> bool {
        self.each_on(callee, src)
            .is_some_and(|name| !self.imported.iter().any(|n| n == name))
    }

    /// Whether `callee` is `each` on a runner function name the file imports from a
    /// module that is not a runner.
    fn is_imported_each(&self, callee: Node, src: &[u8]) -> bool {
        self.each_on(callee, src)
            .is_some_and(|name| self.imported.iter().any(|n| n == name))
    }

    /// The runner function name `callee` reaches `each` through, when the file does not
    /// declare that name as something else.
    fn each_on<'s>(&self, callee: Node, src: &'s [u8]) -> Option<&'s str> {
        if callee.kind() != "member_expression"
            || callee
                .child_by_field_name("property")
                .is_none_or(|p| text(p, src) != "each")
        {
            return None;
        }
        let mut on = callee;
        while on.kind() == "member_expression" {
            on = on.child_by_field_name("object")?;
        }
        let name = text(on, src);
        (on.kind() == "identifier"
            && RUNNER_FUNCTIONS.contains(&name)
            && !self.not_runner.iter().any(|n| n == name))
        .then_some(name)
    }
}

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
        let cells = rows
            .iter()
            .map(|row| CaseRow {
                table: String::new(),
                cells: javascript_row_cells(*row, src),
            })
            .collect();
        CaseList::literal_with_cells(
            rows.into_iter().map(|row| row_text(row, src)).collect(),
            cells,
        )
    }
}

/// Whether a JavaScript value is one literal: a number (signed or not), a string or a
/// template without substitutions, a boolean, `null` or `undefined`.
fn javascript_literal(n: Node, src: &[u8]) -> bool {
    match n.kind() {
        "number" | "string" | "true" | "false" | "null" | "undefined" => true,
        "template_string" => !elements(n)
            .iter()
            .any(|c| c.kind() == "template_substitution"),
        "identifier" => text(n, src) == "undefined",
        "unary_expression" => {
            n.child_by_field_name("operator")
                .is_some_and(|o| matches!(text(o, src), "-" | "+"))
                && n.child_by_field_name("argument")
                    .is_some_and(|a| a.kind() == "number")
        }
        _ => false,
    }
}

/// The cells of one `test.each` row: the items of an array, the properties of an object
/// under their names, or the value itself.
fn javascript_row_cells(row: Node, src: &[u8]) -> Vec<Cell> {
    match row.kind() {
        "array" => elements(row)
            .into_iter()
            .map(|v| cell(None, v, src, javascript_literal(v, src)))
            .collect(),
        "object" => elements(row)
            .into_iter()
            .map(|property| {
                let key = property.child_by_field_name("key");
                match (key, property.child_by_field_name("value")) {
                    (Some(key), Some(value)) if property.kind() == "pair" => cell(
                        Some(text(key, src).to_string()),
                        value,
                        src,
                        javascript_literal(value, src),
                    ),
                    // A shorthand property, a spread, a method: not a literal.
                    _ => cell(None, property, src, false),
                }
            })
            .collect(),
        _ => vec![cell(None, row, src, javascript_literal(row, src))],
    }
}

/// Extracts test cases from JS/TS `test.each(...)`, `it.each(...)`, `describe.each(...)`.
///
/// In JS AST: `test.each([...])("title", fn)` has an outer call whose `function` child
/// is an inner call `test.each([...])` or a tagged template `test.each`\`...\`.
/// A comment between the rows of the array is not a row.
pub fn extract_javascript_cases(func_node: Node, src: &[u8], names: &RunnerNames) -> CaseList {
    let is_each = |callee: Node| names.is_each(callee, src);
    // `test.each(..)` under a `test` imported from a project module: a case source that
    // may be the runner's, whose cases are not counted.
    let callees = match func_node.kind() {
        "call_expression" => [
            func_node.child(0),
            func_node.child_by_field_name("function"),
        ],
        "tagged_template_expression" => [
            func_node
                .child_by_field_name("tag")
                .or_else(|| func_node.child(0)),
            None,
        ],
        _ => [None, None],
    };
    if callees
        .into_iter()
        .flatten()
        .any(|callee| names.is_imported_each(callee, src))
    {
        return CaseList::non_literal();
    }
    if func_node.kind() == "call_expression" {
        let mut cursor = func_node.walk();
        let children: Vec<Node> = func_node.children(&mut cursor).collect();
        if let Some(first) = children.first() {
            if is_each(*first) {
                if let Some(template) = children.iter().find(|c| c.kind() == "template_string") {
                    if let Some(rows) = javascript_template_rows(*template, src) {
                        return CaseList::literal(rows);
                    }
                }
            }
        }

        if let Some(inner_fn) = func_node.child_by_field_name("function") {
            if is_each(inner_fn) {
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
            if is_each(tag) {
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

/// Whether `node` names a type qualified by a package (`pkg.T`, `*pkg.T`, `pkg.T[K]`).
fn go_is_qualified_type(node: Node) -> bool {
    match node.kind() {
        "qualified_type" => true,
        "pointer_type" | "parenthesized_type" => {
            elements(node).into_iter().any(go_is_qualified_type)
        }
        "generic_type" => node
            .child_by_field_name("type")
            .is_some_and(go_is_qualified_type),
        _ => false,
    }
}

/// Collects from a `type_declaration` node the names of struct types that declare at
/// least one field (`type T struct { ... }` and `type T = struct { ... }`).
fn go_collect_struct_types(decl: Node, src: &[u8], out: &mut Vec<String>) {
    for spec in elements(decl) {
        let (Some(name), Some(ty)) = (
            spec.child_by_field_name("name"),
            spec.child_by_field_name("type"),
        ) else {
            continue;
        };
        if matches!(spec.kind(), "type_spec" | "type_alias")
            && ty.kind() == "struct_type"
            && go_struct_has_fields(ty)
        {
            out.push(text(name, src).to_string());
        }
    }
}

/// Collects from a `for_clause` node (`for i := 0; i < len(rows); i++`) the identifiers
/// passed to `len(...)`.
fn go_collect_for_clause_lengths(node: Node, src: &[u8], out: &mut Vec<String>) {
    if node.kind() == "call_expression" {
        let is_len = node
            .child_by_field_name("function")
            .is_some_and(|f| f.kind() == "identifier" && text(f, src) == "len");
        if is_len {
            if let Some(args) = node.child_by_field_name("arguments") {
                for arg in elements(args) {
                    if arg.kind() == "identifier" {
                        out.push(text(arg, src).to_string());
                    }
                }
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        go_collect_for_clause_lengths(child, src, out);
    }
}

/// Whether every entry of a Go map literal's `body` has a struct literal with named
/// fields (`"n1": {DisplayName: "step", Phase: "Succeeded"}`) as its value.
fn go_map_body_has_struct_rows(body: Node) -> bool {
    let entries = elements(body);
    !entries.is_empty()
        && entries.into_iter().all(|entry| {
            if entry.kind() != "keyed_element" {
                return false;
            }
            let parts = elements(entry);
            let Some(val_elem) = parts.get(1) else {
                return false;
            };
            let Some(val) = elements(*val_elem).into_iter().next() else {
                return false;
            };
            let inner = match val.kind() {
                "literal_value" => Some(val),
                "composite_literal" => val.child_by_field_name("body"),
                "unary_expression" => val
                    .child_by_field_name("operand")
                    .filter(|op| op.kind() == "composite_literal")
                    .and_then(|op| op.child_by_field_name("body")),
                _ => None,
            };
            let Some(inner) = inner else {
                return false;
            };
            let fields = elements(inner);
            !fields.is_empty()
                && fields.into_iter().all(|field| {
                    field.kind() == "keyed_element"
                        && elements(field)
                            .first()
                            .and_then(|k| elements(*k).into_iter().next())
                            .is_some_and(|k| matches!(k.kind(), "identifier" | "field_identifier"))
                })
        })
}

/// What the file a test stands in says about the names its body uses.
struct GoFile<'a, 'tree> {
    src: &'a [u8],
    /// The ancestors of the nodes of this file's tree.
    anc: &'a Ancestry<'tree>,
    /// Types declared at package level as a struct with fields, in this file and in
    /// the other files of its package.
    struct_types: &'a [String],
    /// Types declared inside the test body as a struct with fields.
    local_struct_types: Vec<String>,
    /// Identifiers the test body iterates over (`for _, c := range cases`,
    /// `for i := 0; i < len(cases); i++`).
    ranged: Vec<String>,
    /// Every identifier in the test body.
    used: Vec<String>,
    /// What this file declares at package level.
    declared: &'a GoDeclared<'tree>,
    /// This is the test's own file: the lines of its cells are lines of the test's file.
    own: bool,
}

/// The rows of a Go test's tables, and the cells of those in the test's own file.
#[derive(Default)]
struct GoTables {
    rows: Option<Vec<String>>,
    cells: Vec<CaseRow>,
}

/// Whether a Go value is one literal: a number (signed or not), a string, a rune, a
/// boolean or `nil`.
fn go_literal(n: Node, src: &[u8]) -> bool {
    super::expectations::go_lit(n)
        || (n.kind() == "unary_expression"
            && n.child_by_field_name("operator")
                .is_some_and(|o| matches!(text(o, src), "-" | "+"))
            && n.child_by_field_name("operand")
                .is_some_and(|o| matches!(o.kind(), "int_literal" | "float_literal")))
}

/// The expression a `literal_element` holds.
fn go_element_value(element: Node) -> Option<Node> {
    if element.kind() == "literal_element" {
        elements(element).into_iter().next()
    } else {
        Some(element)
    }
}

/// The cells of one Go table row: the fields of its struct literal, positional or under
/// their names. The key of a map table is the case's name, not a cell. A row that is no
/// struct literal is one cell.
fn go_row_cells(row: Node, src: &[u8]) -> Vec<Cell> {
    let value = match row.kind() {
        "keyed_element" => elements(row).get(1).copied().and_then(go_element_value),
        _ => go_element_value(row),
    };
    let Some(value) = value else {
        return Vec::new();
    };
    let body = match value.kind() {
        "literal_value" => Some(value),
        "composite_literal" => value.child_by_field_name("body"),
        "unary_expression" => value
            .child_by_field_name("operand")
            .filter(|op| op.kind() == "composite_literal")
            .and_then(|op| op.child_by_field_name("body")),
        _ => None,
    };
    let Some(body) = body else {
        return vec![cell(None, value, src, go_literal(value, src))];
    };
    elements(body)
        .into_iter()
        .filter_map(|field| {
            if field.kind() != "keyed_element" {
                let v = go_element_value(field)?;
                return Some(cell(None, v, src, go_literal(v, src)));
            }
            let parts = elements(field);
            let key = parts.first().copied().and_then(go_element_value)?;
            let v = parts.get(1).copied().and_then(go_element_value)?;
            Some(cell(
                Some(text(key, src).to_string()),
                v,
                src,
                go_literal(v, src),
            ))
        })
        .collect()
}

/// What one Go file declares at package level, for the tests of the file.
///
/// Every test of a file reads the struct types and the `var` specifications of its
/// package. Reading the declarations of the file again for each test cost the file for
/// each: 1000 tests, in a file of 87 kB, cost 1.5e10 instructions. They are read once,
/// when the first test asks, and a test finds the tables it names by their names.
#[derive(Default)]
pub struct GoPackageLevel<'tree> {
    declared: std::cell::OnceCell<GoDeclared<'tree>>,
    /// The struct types of the file, then those of the other files of its package.
    package_types: std::cell::OnceCell<Vec<String>>,
}

#[derive(Default)]
struct GoDeclared<'tree> {
    /// Types declared at package level as a struct with fields.
    struct_types: Vec<String>,
    /// Package-level `var` specifications.
    package_vars: Vec<Node<'tree>>,
    /// For a name, the places in `package_vars` of the specifications that declare it.
    declaring: std::collections::HashMap<String, Vec<usize>>,
}

impl<'tree> GoPackageLevel<'tree> {
    /// What the file whose tree has the root `root` declares at package level.
    fn declared(&self, root: Node<'tree>, src: &[u8]) -> &GoDeclared<'tree> {
        self.declared.get_or_init(|| GoDeclared::read(root, src))
    }
}

/// Another file of a test's package: its source and the root of its tree.
pub struct GoSibling<'s, 'tree> {
    pub src: &'s [u8],
    pub root: Node<'tree>,
    /// The ancestors of the nodes of the sibling's own tree.
    pub(crate) anc: Ancestry<'tree>,
    /// What the sibling declares at package level.
    level: GoPackageLevel<'tree>,
}

impl<'s, 'tree> GoSibling<'s, 'tree> {
    /// The file `src` whose tree has the root `root`.
    pub fn new(src: &'s [u8], root: Node<'tree>) -> Self {
        Self {
            src,
            root,
            anc: Ancestry::new(root),
            level: GoPackageLevel::default(),
        }
    }
}

/// The name of the package a Go file declares (`package calc_test`).
pub fn go_package_name<'s>(root: Node, src: &'s [u8]) -> Option<&'s str> {
    let clause = elements(root)
        .into_iter()
        .find(|c| c.kind() == "package_clause")?;
    elements(clause)
        .into_iter()
        .find(|c| c.kind() == "package_identifier")
        .map(|name| text(name, src))
}

impl<'tree> GoDeclared<'tree> {
    /// The struct types and `var` specifications `root` declares at package level.
    fn read(root: Node<'tree>, src: &[u8]) -> Self {
        let mut declared = Self::default();
        for decl in elements(root) {
            super::ancestry::count(1);
            match decl.kind() {
                "type_declaration" => {
                    go_collect_struct_types(decl, src, &mut declared.struct_types);
                }
                "var_declaration" => {
                    for spec in elements(decl) {
                        match spec.kind() {
                            "var_spec" => declared.package_vars.push(spec),
                            "var_spec_list" => declared.package_vars.extend(
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
        for (at, spec) in declared.package_vars.iter().enumerate() {
            let mut cursor = spec.walk();
            for name in spec.children_by_field_name("name", &mut cursor) {
                let places = declared
                    .declaring
                    .entry(text(name, src).to_string())
                    .or_default();
                if places.last() != Some(&at) {
                    places.push(at);
                }
            }
        }
        declared
    }
}

impl<'a, 'tree> GoFile<'a, 'tree> {
    /// Adds to `total` the rows of the package-level tables of this file that `named`
    /// holds the name of, in the order the file declares them.
    fn count_named_package_tables(&self, named: &[String], total: &mut GoTables) {
        let mut named_in_body: Vec<usize> = named
            .iter()
            .filter_map(|name| self.declared.declaring.get(name))
            .flatten()
            .copied()
            .collect();
        super::ancestry::count(named.len() + named_in_body.len());
        named_in_body.sort_unstable();
        named_in_body.dedup();
        for at in named_in_body {
            // A call ranged over inside a package-level initialiser is not this test's.
            let mut ignored = false;
            self.count_tables(self.declared.package_vars[at], total, &mut ignored);
        }
    }

    fn collect_names(&mut self, node: Node<'tree>) {
        match node.kind() {
            "identifier" => self.used.push(text(node, self.src).to_string()),
            "type_declaration" => {
                go_collect_struct_types(node, self.src, &mut self.local_struct_types);
            }
            "range_clause" => {
                if let Some(right) = node.child_by_field_name("right") {
                    if right.kind() == "identifier" {
                        self.ranged.push(text(right, self.src).to_string());
                    }
                }
            }
            "for_clause" => {
                go_collect_for_clause_lengths(node, self.src, &mut self.ranged);
            }
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_names(child);
        }
    }

    /// How the element type of a slice, array or map literal reads as a row type.
    fn rows(&self, type_node: Node, body: Node) -> GoRows {
        let src = self.src;
        let is_map = type_node.kind() == "map_type";
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
        let struct_node = match row.kind() {
            "struct_type" => Some(row),
            "pointer_type" => elements(row)
                .into_iter()
                .find(|c| c.kind() == "struct_type"),
            _ => None,
        };
        if let Some(st) = struct_node {
            return if go_struct_has_fields(st) {
                GoRows::Table
            } else {
                GoRows::No
            };
        }
        let Some(name) = go_type_name(row) else {
            return GoRows::No;
        };
        let name_text = text(name, src);
        let by_name = go_name_words(name_text)
            .iter()
            .any(|word| words.contains(&word.as_str()));
        if by_name {
            GoRows::Table
        } else if !go_is_qualified_type(row)
            && (self.struct_types.iter().any(|t| t == name_text)
                || self.local_struct_types.iter().any(|t| t == name_text))
        {
            if is_map {
                GoRows::Table
            } else {
                GoRows::NamedStruct
            }
        } else if is_map && go_map_body_has_struct_rows(body) {
            GoRows::Table
        } else {
            GoRows::No
        }
    }

    /// The variable a composite literal is bound to: `cases` in `cases := []T{..}`,
    /// `var cases = []T{..}` and `cases = []T{..}`.
    fn bound_name(&self, literal: Node<'tree>) -> Option<&'a str> {
        let anc = self.anc;
        let list = anc
            .parent(literal)
            .filter(|p| p.kind() == "expression_list")?;
        let index = elements(list).iter().position(|e| e.id() == literal.id())?;
        let statement = anc.parent(list)?;
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
    fn count_tables(&self, node: Node<'tree>, total: &mut GoTables, non_literal: &mut bool) {
        if node.kind() == "composite_literal" {
            if let (Some(type_node), Some(body)) = (
                node.child_by_field_name("type"),
                node.child_by_field_name("body"),
            ) {
                let is_table = match self.rows(type_node, body) {
                    GoRows::Table => true,
                    GoRows::NamedStruct => {
                        self.anc
                            .parent(node)
                            .is_some_and(|p| p.kind() == "range_clause")
                            || self
                                .bound_name(node)
                                .is_some_and(|name| self.ranged.iter().any(|r| r == name))
                    }
                    GoRows::No => false,
                };
                if is_table {
                    // The body is a `literal_value`; a row turned into a comment is not
                    // one of its elements.
                    let rows = elements(body);
                    total
                        .rows
                        .get_or_insert_with(Vec::new)
                        .extend(rows.iter().map(|row| row_text(*row, self.src)));
                    if self.own {
                        let table = row_text(type_node, self.src);
                        total.cells.extend(rows.iter().map(|row| CaseRow {
                            table: table.clone(),
                            cells: go_row_cells(*row, self.src),
                        }));
                    }
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
#[cfg(test)]
pub fn extract_go_cases<'t>(body_node: Node<'t>, anc: &Ancestry<'t>, src: &[u8]) -> CaseList {
    extract_go_cases_in_package(body_node, anc, src, &[], &GoPackageLevel::default())
}

/// [`extract_go_cases`] with the other files of the test's package read too: a struct
/// type one of them declares is a row type as one of the test's own file is, and a
/// package-level `var` one of them holds counts toward every test whose body names it.
/// A table or row type moved to another file of the package therefore keeps its rows.
///
/// `level` is what the test's own file declares at package level: one value for every
/// test of the file with these `siblings`, so the file and its siblings are read once.
pub fn extract_go_cases_in_package<'t>(
    body_node: Node<'t>,
    anc: &Ancestry<'t>,
    src: &[u8],
    siblings: &[GoSibling<'_, 't>],
    level: &GoPackageLevel<'t>,
) -> CaseList {
    let root = anc.root();
    // The body itself is the root when a caller hands in a detached block: such a
    // block has no package level, and what is kept for a file is not asked.
    let detached = root.id() == body_node.id();
    let nothing = GoDeclared::default();
    let declared = if detached {
        &nothing
    } else {
        level.declared(root, src)
    };
    // Every file of the package sees the struct types of all of them.
    let all_types = || {
        let mut struct_types = declared.struct_types.clone();
        for sibling in siblings {
            let other = sibling.level.declared(sibling.root, sibling.src);
            struct_types.extend(other.struct_types.iter().cloned());
        }
        struct_types
    };
    let of_detached;
    let struct_types: &[String] = if detached {
        of_detached = all_types();
        &of_detached
    } else {
        level.package_types.get_or_init(all_types)
    };
    let mut file = GoFile {
        src,
        anc,
        struct_types,
        local_struct_types: Vec::new(),
        ranged: Vec::new(),
        used: Vec::new(),
        declared,
        own: true,
    };
    file.collect_names(body_node);
    let others: Vec<GoFile> = siblings
        .iter()
        .map(|sibling| GoFile {
            src: sibling.src,
            anc: &sibling.anc,
            struct_types,
            local_struct_types: file.local_struct_types.clone(),
            ranged: file.ranged.clone(),
            used: Vec::new(),
            declared: sibling.level.declared(sibling.root, sibling.src),
            own: false,
        })
        .collect();
    let mut table_cases = GoTables::default();
    let mut has_non_literal = false;

    file.count_tables(body_node, &mut table_cases, &mut has_non_literal);
    file.count_named_package_tables(&file.used, &mut table_cases);
    for other in &others {
        other.count_named_package_tables(&file.used, &mut table_cases);
    }

    match table_cases.rows {
        Some(rows) => CaseList::literal_with_cells(rows, table_cases.cells),
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
        // `@ValueSource(ints = {1, 2})`: one typed array, whichever its name. A name
        // in place of the array (`strings = NAME`) is one value: an annotation element
        // takes constant expressions, and no array is one.
        "ValueSource" => match args.as_slice() {
            [(_, value)]
                if matches!(
                    value.kind(),
                    "identifier" | "field_access" | "scoped_identifier"
                ) =>
            {
                Ok(Some(vec![row_text(*value, src)]))
            }
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
pub fn extract_rust_cases<'t>(fn_node: Node<'t>, anc: &Ancestry<'t>, src: &[u8]) -> CaseList {
    let mut cases: Vec<String> = Vec::new();
    let mut values = CaseList::none();

    // Check preceding attribute_item siblings
    let mut prev = anc.prev_sibling(fn_node);
    while let Some(p) = prev {
        match p.kind() {
            "attribute_item" => {
                let mut sub_cursor = p.walk();
                for attr in p.children(&mut sub_cursor) {
                    if attr.kind() == "attribute" {
                        cases.extend(rust_case_row(attr, src));
                    }
                }
                prev = anc.prev_sibling(p);
            }
            "line_comment" | "block_comment" => {
                prev = anc.prev_sibling(p);
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
        } = extract_python_cases(Some(&decorators), code.as_bytes(), &PytestNames::default());
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
        } = extract_python_cases(Some(&decorators), code.as_bytes(), &PytestNames::default());
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
        } = extract_python_cases(Some(&decorators), code.as_bytes(), &PytestNames::default());
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
        } = extract_python_cases(Some(&decorators), code.as_bytes(), &PytestNames::default());
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
        } = extract_javascript_cases(func_node, code.as_bytes(), &RunnerNames::default());
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
        } = extract_javascript_cases(func_node, code.as_bytes(), &RunnerNames::default());
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
        } = extract_javascript_cases(func_node, code.as_bytes(), &RunnerNames::default());
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
        } = extract_go_cases(body, &Ancestry::new(root), code.as_bytes());
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
        } = extract_go_cases(body, &Ancestry::new(root), code.as_bytes());
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
        } = extract_rust_cases(fn_node, &Ancestry::new(root), code.as_bytes());
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
        } = extract_rust_cases(fn_node, &Ancestry::new(root), code.as_bytes());
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

    #[test]
    fn go_named_struct_map_is_a_case_table() {
        let pkg_struct = "package p\n\nimport \"testing\"\n\ntype cdWorkflowNode struct {\n\tDisplayName string\n\tPhase       string\n}\n\nfunc TestNodes(t *testing.T) {\n\tnodes := map[string]cdWorkflowNode{\n\t\t\"n1\": {DisplayName: \"step-a\", Phase: \"Succeeded\"},\n\t\t\"n2\": {DisplayName: \"step-b\", Phase: \"Failed\"},\n\t}\n\tif len(nodes) == 0 {\n\t\tt.Fatal(\"empty\")\n\t}\n}\n";
        assert_eq!(first_test("x_test.go", pkg_struct).cases, Some(2));

        let ptr_struct =
            pkg_struct.replace("map[string]cdWorkflowNode", "map[string]*cdWorkflowNode");
        assert_eq!(first_test("x_test.go", &ptr_struct).cases, Some(2));

        let local_struct = "package p\n\nimport \"testing\"\n\nfunc TestNodes(t *testing.T) {\n\ttype cdWorkflowNode = struct {\n\t\tDisplayName string\n\t\tPhase       string\n\t}\n\tnodes := map[string]cdWorkflowNode{\n\t\t\"n1\": {DisplayName: \"step-a\", Phase: \"Succeeded\"},\n\t\t\"n2\": {DisplayName: \"step-b\", Phase: \"Failed\"},\n\t}\n\tif len(nodes) == 0 {\n\t\tt.Fatal(\"empty\")\n\t}\n}\n";
        assert_eq!(first_test("x_test.go", local_struct).cases, Some(2));

        let external_struct_rows = "package p\n\nimport \"testing\"\n\nfunc TestNodes(t *testing.T) {\n\tnodes := map[string]wfv1.NodeStatus{\n\t\t\"n1\": {DisplayName: \"step-a\", Phase: \"Succeeded\"},\n\t\t\"n2\": {DisplayName: \"step-b\", Phase: \"Failed\"},\n\t}\n\tif len(nodes) == 0 {\n\t\tt.Fatal(\"empty\")\n\t}\n}\n";
        assert_eq!(first_test("x_test.go", external_struct_rows).cases, Some(2));

        // Negative controls: an empty struct type is a set, not a case table; and a map of
        // an undeclared non-case type with positional values is not a case table.
        let empty_struct = "package p\n\nimport \"testing\"\n\ntype emptyNode struct{}\n\nfunc TestSeen(t *testing.T) {\n\tseen := map[string]emptyNode{\n\t\t\"a\": {},\n\t\t\"b\": {},\n\t}\n\tif len(seen) == 0 {\n\t\tt.Fatal(\"empty\")\n\t}\n}\n";
        assert_eq!(first_test("x_test.go", empty_struct).cases, None);
    }

    /// The head lines `rewritten_cells` reports between the first test of `base` and the
    /// first test of `head`.
    fn rewritten(path: &str, base: &str, head: &str) -> Vec<usize> {
        rewritten_cells(
            &first_test(path, base).case_cells,
            &first_test(path, head).case_cells,
        )
    }

    /// A pytest file whose one test runs the rows `rows`, on line 3.
    fn py_table(rows: &str) -> String {
        format!(
            "import pytest\n\n@pytest.mark.parametrize(\"x,y\", [{rows}])\ndef test_f(x, y):\n    assert f(x) == y\n"
        )
    }

    #[test]
    fn python_a_row_with_one_value_rewritten_is_reported() {
        let base = py_table("(1, 2), (2, 4)");
        assert_eq!(
            rewritten("tests/test_a.py", &base, &py_table("(1, 2), (2, 5)")),
            [3]
        );
        // A string, a boolean, `None` and a signed number are literals too.
        let base = py_table("(1, \"a\"), (2, True), (3, None), (4, -1)");
        let head = py_table("(1, \"b\"), (2, False), (3, 0), (4, -2)");
        assert_eq!(rewritten("tests/test_a.py", &base, &head), [3]);
        // `pytest.param` rows: its `id` is not a cell, its values are.
        let base = py_table("pytest.param(1, 2, id=\"one\"), pytest.param(2, 4, id=\"two\")");
        let head = py_table("pytest.param(1, 2, id=\"one\"), pytest.param(2, 5, id=\"two\")");
        assert_eq!(rewritten("tests/test_a.py", &base, &head), [3]);
        // Control: the same table on both sides.
        assert!(rewritten("tests/test_a.py", &base, &base).is_empty());
    }

    #[test]
    fn python_rows_edited_otherwise_are_not_reported() {
        let base = py_table("(1, 2), (2, 4)");
        let cases = [
            // Reordered, unchanged.
            "(2, 4), (1, 2)",
            // Two cells changed.
            "(1, 2), (3, 6)",
            // A row added beside unchanged rows.
            "(1, 2), (2, 4), (3, 6)",
            // A row removed (`test-cases-reduced`).
            "(1, 2)",
            // The changed cell is not a literal.
            "(1, 2), (2, g(4))",
        ];
        for head in cases {
            assert!(
                rewritten("tests/test_a.py", &base, &py_table(head)).is_empty(),
                "{head}"
            );
        }
        // Only the `id` of a `pytest.param` row changed: no value did.
        let base = py_table("pytest.param(1, 2, id=\"one\"), pytest.param(2, 4, id=\"two\")");
        let head = py_table("pytest.param(1, 2, id=\"one\"), pytest.param(2, 4, id=\"four\")");
        assert!(rewritten("tests/test_a.py", &base, &head).is_empty());
    }

    #[test]
    fn an_ambiguous_pairing_is_not_reported() {
        // One base row left, one cell away from two head rows.
        let base = py_table("(1, 2), (2, 4)");
        let head = py_table("(1, 2), (2, 5), (2, 6)");
        assert!(rewritten("tests/test_a.py", &base, &head).is_empty());
        // Two base rows left, each one cell away from the one head row left.
        let base = py_table("(1, 2), (1, 3)");
        let head = py_table("(1, 4)");
        assert!(rewritten("tests/test_a.py", &base, &head).is_empty());
        // Control: the rows left pair one for one.
        let head = py_table("(1, 5), (1, 3)");
        assert_eq!(rewritten("tests/test_a.py", &base, &head), [3]);
    }

    #[test]
    fn a_row_of_one_cell_is_a_case_replaced_not_paired() {
        let one = |rows: &str| {
            format!(
                "import pytest\n\n@pytest.mark.parametrize(\"x\", [{rows}])\ndef test_f(x):\n    assert f(x)\n"
            )
        };
        assert!(rewritten("tests/test_a.py", &one("1, 2"), &one("1, 3")).is_empty());
        // Stacked: a row of the product is a value of each list, so it has two cells; a
        // value changed in one list is reported once, at its line.
        let stacked = |xs: &str| {
            format!(
                "import pytest\n\n@pytest.mark.parametrize(\"x\", [{xs}])\n@pytest.mark.parametrize(\"y\", [\"a\", \"b\"])\ndef test_f(x, y):\n    assert f(x, y)\n"
            )
        };
        assert_eq!(
            rewritten("tests/test_a.py", &stacked("1, 2"), &stacked("1, 3")),
            [3]
        );
    }

    /// A Go test whose table is `table`, on line 6.
    fn go_table(table: &str) -> String {
        format!(
            "package p\n\nimport \"testing\"\n\nfunc TestF(t *testing.T) {{\n\tcases := {table}\n\tfor _, c := range cases {{\n\t\tif F(c.in) != c.want {{\n\t\t\tt.Error(c)\n\t\t}}\n\t}}\n}}\n"
        )
    }

    #[test]
    fn go_a_row_with_one_value_rewritten_is_reported() {
        let base = go_table("[]struct{ in, want int }{{1, 2}, {2, 4}}");
        let head = go_table("[]struct{ in, want int }{{1, 2}, {2, 5}}");
        assert_eq!(rewritten("p_test.go", &base, &head), [6]);
        // Fields under their names, a row on several lines: the line of the value.
        let keyed = |want: &str| {
            go_table(&format!(
                "[]struct {{\n\t\tname string\n\t\tin, want int\n\t}}{{\n\t\t{{name: \"one\", in: 1, want: 2}},\n\t\t{{\n\t\t\tname: \"two\",\n\t\t\tin:   2,\n\t\t\twant: {want},\n\t\t}},\n\t}}"
            ))
        };
        assert_eq!(rewritten("p_test.go", &keyed("4"), &keyed("-5")), [14]);
        // A map table: the key is the case's name, the struct's fields are the cells.
        let map = |key: &str, want: &str| {
            go_table(&format!(
                "map[string]struct{{ in, want int }}{{\"one\": {{1, 2}}, \"{key}\": {{2, {want}}}}}"
            ))
        };
        assert_eq!(
            rewritten("p_test.go", &map("two", "4"), &map("two", "5")),
            [6]
        );
        // Control: only the key changed.
        assert!(rewritten("p_test.go", &map("two", "4"), &map("2", "4")).is_empty());
    }

    #[test]
    fn go_rows_edited_otherwise_are_not_reported() {
        let base = go_table("[]struct{ in, want int }{{1, 2}, {2, 4}}");
        let cases = [
            "[]struct{ in, want int }{{2, 4}, {1, 2}}",
            "[]struct{ in, want int }{{1, 2}, {3, 6}}",
            "[]struct{ in, want int }{{1, 2}, {2, 4}, {3, 6}}",
            "[]struct{ in, want int }{{1, 2}, {2, four}}",
            // A row one cell away from two rows left.
            "[]struct{ in, want int }{{1, 2}, {2, 5}, {2, 6}}",
        ];
        for head in cases {
            assert!(
                rewritten("p_test.go", &base, &go_table(head)).is_empty(),
                "{head}"
            );
        }
        // A field renamed is not the same cell.
        let keyed = |field: &str| {
            go_table(&format!(
                "[]struct{{ in, want, got int }}{{{{in: 1, want: 2}}, {{in: 2, {field}: 4}}}}"
            ))
        };
        assert!(rewritten("p_test.go", &keyed("want"), &keyed("got")).is_empty());
    }

    /// A JS file whose one test runs the rows `rows`, on line 1.
    fn js_table(rows: &str) -> String {
        format!("test.each([{rows}])('f', (a, b) => {{\n  expect(f(a)).toBe(b);\n}});\n")
    }

    #[test]
    fn javascript_a_row_with_one_value_rewritten_is_reported() {
        let base = js_table("[1, 2], [2, 4]");
        assert_eq!(
            rewritten("a.test.js", &base, &js_table("[1, 2], [2, 5]")),
            [1]
        );
        // Object rows: properties under their names.
        let base = js_table("{ a: 1, b: 'x' }, { a: 2, b: 'y' }");
        let head = js_table("{ a: 1, b: 'x' }, { a: 2, b: 'z' }");
        assert_eq!(rewritten("a.test.ts", &base, &head), [1]);
        // Control: a property renamed is not the same cell.
        let head = js_table("{ a: 1, b: 'x' }, { a: 2, c: 'y' }");
        assert!(rewritten("a.test.ts", &base, &head).is_empty());
    }

    #[test]
    fn javascript_rows_edited_otherwise_are_not_reported() {
        let base = js_table("[1, 2], [2, 4]");
        for head in [
            "[2, 4], [1, 2]",
            "[1, 2], [3, 6]",
            "[1, 2], [2, 4], [3, 6]",
            "[1, 2], [2, g(4)]",
            "[1, 2], [2, 5], [2, 6]",
        ] {
            assert!(
                rewritten("a.test.js", &base, &js_table(head)).is_empty(),
                "{head}"
            );
        }
        // A template table's rows are text, not cells: not paired.
        let template = |v: &str| {
            format!("test.each`\n  a | b\n  ${{1}} | ${{{v}}}\n`('f', ({{ a, b }}) => {{ expect(f(a)).toBe(b); }});\n")
        };
        assert!(rewritten("a.test.js", &template("2"), &template("3")).is_empty());
    }
}
