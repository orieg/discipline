//! Parametrized and table-driven test case counting.
//!
//! Tracks literal test case tables across supported ecosystems:
//! - Python: `@pytest.mark.parametrize` list / tuple literals
//! - JS / TS: `test.each([...])`, `it.each`, `describe.each`
//! - Go: `[]struct{...}{...}` composite literals
//! - Java / Kotlin: `@ValueSource` array lengths, `@CsvSource` rows
//! - C#: `[InlineData]` / `[TestCase]` attribute counts
//! - Rust: `#[case]` attribute counts, `#[values]` combinations
//!
//! Non-literal sources (fixtures, generators, dynamic function calls) yield `cases = None`
//! and set `non_literal = true`, so gate notes report that they were not compared.

use tree_sitter::Node;

fn text<'a>(node: Node, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

// ============================================================================
// Python
// ============================================================================

/// Extracts test case count from Python decorators.
///
/// Recognizes `@pytest.mark.parametrize(names, values)`.
/// If values is a list `[...]` or tuple `(...)`, counts elements.
/// When multiple `@pytest.mark.parametrize` decorators are present,
/// computes the Cartesian product (total test executions).
/// Non-literal values (function calls, identifiers) set `non_literal = true`.
pub fn extract_python_cases(decorators: Option<&[Node]>, src: &[u8]) -> (Option<usize>, bool) {
    let Some(decs) = decorators else {
        return (None, false);
    };

    let mut total_cases: usize = 1;
    let mut found = false;
    let mut has_non_literal = false;

    for dec in decs {
        // Decorator has an expression, usually a call: `@pytest.mark.parametrize(...)`
        let mut cursor = dec.walk();
        for child in dec.children(&mut cursor) {
            if child.kind() == "call" {
                if let Some(func) = child.child_by_field_name("function") {
                    let fn_name = text(func, src);
                    if fn_name.ends_with("parametrize") {
                        found = true;
                        if let Some(args) = child.child_by_field_name("arguments") {
                            let mut values_node = None;
                            let mut positional_idx = 0;
                            let mut arg_cursor = args.walk();
                            for arg in args.named_children(&mut arg_cursor) {
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

                            if let Some(val_node) = values_node {
                                match val_node.kind() {
                                    "list" | "tuple" => {
                                        let count = val_node.named_child_count();
                                        total_cases = total_cases.saturating_mul(count);
                                    }
                                    _ => {
                                        has_non_literal = true;
                                    }
                                }
                            } else {
                                has_non_literal = true;
                            }
                        }
                    }
                }
            }
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

// ============================================================================
// JavaScript / TypeScript
// ============================================================================

/// Extracts test cases from JS/TS `test.each(...)`, `it.each(...)`, `describe.each(...)`.
///
/// In JS AST: `test.each([...])("title", fn)` has an outer call whose `function` child
/// is an inner call `test.each([...])` or a tagged template `test.each`\`...\`.
pub fn extract_javascript_cases(func_node: Node, src: &[u8]) -> (Option<usize>, bool) {
    if func_node.kind() == "call_expression" {
        let mut cursor = func_node.walk();
        let children: Vec<Node> = func_node.children(&mut cursor).collect();
        if let Some(first) = children.first() {
            let fn_text = text(*first, src);
            if fn_text.ends_with(".each") || fn_text == "each" {
                if let Some(template) = children.iter().find(|c| c.kind() == "template_string") {
                    let template_text = text(*template, src);
                    let lines: Vec<&str> = template_text
                        .lines()
                        .map(|l| l.trim())
                        .filter(|l| !l.is_empty() && l.contains('|'))
                        .collect();
                    if lines.len() > 1 {
                        return (Some(lines.len() - 1), false);
                    } else if !lines.is_empty() {
                        return (Some(lines.len()), false);
                    }
                }
            }
        }

        if let Some(inner_fn) = func_node.child_by_field_name("function") {
            let fn_text = text(inner_fn, src);
            if fn_text.ends_with(".each") || fn_text == "each" {
                if let Some(args) = func_node.child_by_field_name("arguments") {
                    let mut cursor = args.walk();
                    let first_arg = args.named_children(&mut cursor).next();
                    if let Some(arg) = first_arg {
                        if arg.kind() == "array" {
                            let count = arg.named_child_count();
                            return (Some(count), false);
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
                if let Some(template) = template {
                    let template_text = text(template, src);
                    // Count non-empty table lines after the header row
                    let lines: Vec<&str> = template_text
                        .lines()
                        .map(|l| l.trim())
                        .filter(|l| !l.is_empty() && l.contains('|'))
                        .collect();
                    if lines.len() > 1 {
                        // lines[0] is header row (a | b | expected), rest are cases
                        return (Some(lines.len() - 1), false);
                    } else if !lines.is_empty() {
                        return (Some(lines.len()), false);
                    }
                }
            }
        }
    }
    (None, false)
}

// ============================================================================
// Go
// ============================================================================

/// Extracts test cases from a Go test function body.
///
/// Looks for table composite literals: `[]struct{...}{...}`, `[...]struct{...}{...}`,
/// or `map[...]...{...}`.
/// If a `range` loop in the test iterates over a non-literal (function call or external slice),
/// sets `non_literal = true`.
pub fn extract_go_cases(body_node: Node, src: &[u8]) -> (Option<usize>, bool) {
    let mut table_cases: Option<usize> = None;
    let mut has_non_literal = false;

    // Scan for composite literals in the test body
    fn scan_node<'a>(
        node: Node<'a>,
        src: &'a [u8],
        table_cases: &mut Option<usize>,
        has_non_literal: &mut bool,
    ) {
        if node.kind() == "composite_literal" {
            if let Some(type_node) = node.child_by_field_name("type") {
                let type_kind = type_node.kind();
                if type_kind == "slice_type" || type_kind == "array_type" || type_kind == "map_type"
                {
                    // Check if element is a struct or test case struct
                    let is_table_type = if let Some(elem) = type_node.child_by_field_name("element")
                    {
                        elem.kind() == "struct_type"
                            || text(elem, src).contains("struct")
                            || text(elem, src).to_lowercase().contains("case")
                            || text(elem, src).to_lowercase().contains("test")
                    } else if type_kind == "map_type" {
                        if let Some(val) = type_node.child_by_field_name("value") {
                            val.kind() == "struct_type"
                                || text(val, src).contains("struct")
                                || text(val, src).to_lowercase().contains("case")
                        } else {
                            false
                        }
                    } else {
                        false
                    };

                    if is_table_type {
                        if let Some(body) = node.child_by_field_name("body") {
                            // Body is literal_value
                            let count = body.named_child_count();
                            if table_cases.is_none() || Some(count) > *table_cases {
                                *table_cases = Some(count);
                            }
                        }
                    }
                }
            }
        } else if node.kind() == "range_clause" {
            // Check what is being ranged over: `for _, tc := range ...`
            if let Some(right) = node.child_by_field_name("right") {
                if right.kind() == "call_expression" {
                    *has_non_literal = true;
                }
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            scan_node(child, src, table_cases, has_non_literal);
        }
    }

    scan_node(body_node, src, &mut table_cases, &mut has_non_literal);

    if has_non_literal && table_cases.is_none() {
        (None, true)
    } else {
        (table_cases, false)
    }
}

// ============================================================================
// Java
// ============================================================================

fn find_array_initializer(node: Node) -> Option<Node> {
    if node.kind() == "array_initializer" || node.kind() == "element_value_array_initializer" {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = find_array_initializer(child) {
            return Some(found);
        }
    }
    None
}

fn find_first_string_literal(node: Node) -> Option<Node> {
    if node.kind() == "string_literal" || node.kind() == "multiline_string_literal" {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = find_first_string_literal(child) {
            return Some(found);
        }
    }
    None
}

/// Extracts test cases from Java JUnit 5 annotations on a method (`modifiers` node).
///
/// Recognizes `@ValueSource(strings = {...})`, `@CsvSource({...})`.
/// `@MethodSource`, `@CsvFileSource`, `@ArgumentsSource` mark `non_literal = true`.
pub fn extract_java_cases(modifiers_node: Node, src: &[u8]) -> (Option<usize>, bool) {
    let mut total_cases: Option<usize> = None;
    let mut has_non_literal = false;

    let mut cursor = modifiers_node.walk();
    for child in modifiers_node.children(&mut cursor) {
        if child.kind() == "annotation" || child.kind() == "marker_annotation" {
            if let Some(name_node) = child.child_by_field_name("name") {
                let name = text(name_node, src);
                let clean_name = name.rsplit('.').next().unwrap_or(name);
                match clean_name {
                    "ValueSource" => {
                        if let Some(arr) = find_array_initializer(child) {
                            total_cases = Some(arr.named_child_count());
                        }
                    }
                    "CsvSource" => {
                        if let Some(arr) = find_array_initializer(child) {
                            total_cases = Some(arr.named_child_count());
                        } else if let Some(str_node) = find_first_string_literal(child) {
                            let s = text(str_node, src);
                            let lines: Vec<&str> = s
                                .lines()
                                .map(|l| l.trim())
                                .filter(|l| !l.is_empty())
                                .collect();
                            total_cases = Some(lines.len().max(1));
                        }
                    }
                    "MethodSource" | "CsvFileSource" | "ArgumentsSource" => {
                        has_non_literal = true;
                    }
                    _ => {}
                }
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

/// Extracts test cases from Kotlin annotations on a function.
pub fn extract_kotlin_cases(node: Node, src: &[u8]) -> (Option<usize>, bool) {
    let mut total_cases: Option<usize> = None;
    let mut has_non_literal = false;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifiers" || child.kind() == "annotation" {
            let mut sub_cursor = child.walk();
            let check_node =
                |n: Node, total_cases: &mut Option<usize>, has_non_literal: &mut bool| {
                    let txt = text(n, src);
                    if txt.contains("ValueSource") || txt.contains("CsvSource") {
                        // Check if collection literal or arrayOf
                        let mut a_cursor = n.walk();
                        for a_child in n.children(&mut a_cursor) {
                            if a_child.kind() == "collection_literal" {
                                *total_cases = Some(a_child.named_child_count());
                            } else if a_child.kind() == "call_expression"
                                && text(a_child, src).contains("arrayOf")
                            {
                                if let Some(args) = a_child.child_by_field_name("arguments") {
                                    *total_cases = Some(args.named_child_count());
                                }
                            }
                        }
                    } else if txt.contains("MethodSource") || txt.contains("CsvFileSource") {
                        *has_non_literal = true;
                    }
                };
            if child.kind() == "annotation" {
                check_node(child, &mut total_cases, &mut has_non_literal);
            } else {
                for grandchild in child.children(&mut sub_cursor) {
                    if grandchild.kind() == "annotation" {
                        check_node(grandchild, &mut total_cases, &mut has_non_literal);
                    }
                }
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
// C#
// ============================================================================

/// Extracts test cases from C# attribute lists on a method declaration.
///
/// Recognizes `[InlineData(...)]` (xUnit) and `[TestCase(...)]` (NUnit).
/// Non-literal attributes `[MemberData]`, `[ClassData]`, `[TestCaseSource]` set `non_literal = true`.
pub fn extract_csharp_cases(method_node: Node, src: &[u8]) -> (Option<usize>, bool) {
    let mut count = 0;
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
                            "InlineData" | "TestCase" => {
                                count += 1;
                            }
                            "MemberData" | "ClassData" | "TestCaseSource" => {
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
    } else if count > 0 {
        (Some(count), false)
    } else {
        (None, false)
    }
}

// ============================================================================
// Rust
// ============================================================================

/// Extracts test cases from Rust outer attributes and arguments (`rstest`).
///
/// Recognizes `#[case(...)]` attribute count on function.
/// Also handles combinations of `#[values(...)]` in argument attributes.
pub fn extract_rust_cases(fn_node: Node, src: &[u8]) -> (Option<usize>, bool) {
    let mut case_count = 0;
    let mut values_product: usize = 1;
    let mut has_values = false;

    // Check preceding attribute_item siblings
    let mut prev = fn_node.prev_sibling();
    while let Some(p) = prev {
        match p.kind() {
            "attribute_item" => {
                let mut sub_cursor = p.walk();
                for attr in p.children(&mut sub_cursor) {
                    if attr.kind() == "attribute" {
                        let attr_text = text(attr, src);
                        if attr_text.starts_with("case") || attr_text.starts_with("rstest::case") {
                            case_count += 1;
                        }
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
                    let attr_text = text(attr, src);
                    if attr_text.starts_with("case") || attr_text.starts_with("rstest::case") {
                        case_count += 1;
                    }
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
                                    let count = args.named_child_count();
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

    if case_count > 0 {
        (Some(case_count), false)
    } else if has_values {
        (Some(values_product), false)
    } else {
        (None, false)
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
