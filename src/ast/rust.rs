//! Rust language pack: tree-sitter AST extraction of tests, assertions, and unsafe sites.

use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser};

use super::{AssertVocabulary, EscapeHatchSite, LanguagePack, ParsedFileFacts, TestFn, UnsafeSite};

/// Rust language pack implementing [`LanguagePack`].
pub struct RustPack;

impl LanguagePack for RustPack {
    fn id(&self) -> &'static str {
        "rust"
    }

    fn name(&self) -> &'static str {
        "Rust"
    }

    fn matches(&self, path: &str) -> bool {
        super::extension(path) == Some("rs")
    }

    fn extract(&self, _path: &str, src: &str, vocab: &AssertVocabulary) -> Result<ParsedFileFacts> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .map_err(|e| anyhow!("failed to load the Rust grammar: {e}"))?;
        let tree = parser
            .parse(src, None)
            .ok_or_else(|| anyhow!("tree-sitter returned no tree"))?;
        let root = tree.root_node();

        let mut cx = Extractor {
            src: src.as_bytes(),
            lines: src.lines().collect(),
            line_starts: std::iter::once(0)
                .chain(src.match_indices('\n').map(|(i, _)| i + 1))
                .collect(),
            vocab,
            comments: Vec::new(),
            in_fn: 0,
            in_const: 0,
            in_test: 0,
            facts: ParsedFileFacts {
                has_parse_errors: root.has_error(),
                ..Default::default()
            },
            helpers: std::collections::HashMap::new(),
            test_calls: Vec::new(),
        };
        cx.collect_comments(root);
        cx.visit(root, &mut Vec::new());
        cx.resolve_same_file_helpers();
        cx.facts.build_compile_time_test();
        Ok(cx.facts)
    }
}

#[derive(Default, Clone)]
struct HelperFacts {
    total_asserts: usize,
    strong_asserts: usize,
    tautologies: usize,
    fatal_asserts: usize,
}

struct Comment {
    start_row: usize,
    end_row: usize,
    start_byte: usize,
    end_byte: usize,
    has_safety: bool,
}

struct Extractor<'a> {
    src: &'a [u8],
    lines: Vec<&'a str>,
    /// Byte offset of each line start (robust to CRLF, unlike summing `lines`).
    line_starts: Vec<usize>,
    vocab: &'a AssertVocabulary,
    comments: Vec<Comment>,
    in_fn: usize,
    in_const: usize,
    in_test: usize,
    facts: ParsedFileFacts,
    helpers: std::collections::HashMap<String, HelperFacts>,
    test_calls: Vec<Vec<String>>,
}

impl<'a> Extractor<'a> {
    fn text(&self, node: Node) -> &'a str {
        node.utf8_text(self.src).unwrap_or("")
    }

    fn collect_comments(&mut self, node: Node) {
        if matches!(node.kind(), "line_comment" | "block_comment") {
            let text = self.text(node);
            self.comments.push(Comment {
                start_row: node.start_position().row,
                end_row: node.end_position().row,
                start_byte: node.start_byte(),
                end_byte: node.end_byte(),
                has_safety: has_valid_safety_comment_with_placeholders(
                    text,
                    &self.vocab.safety_placeholders,
                ),
            });
            return;
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_comments(child);
        }
    }

    fn visit(&mut self, node: Node, mods: &mut Vec<String>) {
        match node.kind() {
            "mod_item" => {
                let name = node
                    .child_by_field_name("name")
                    .map(|n| self.text(n).to_string())
                    .unwrap_or_default();
                mods.push(name);
                self.visit_children(node, mods);
                mods.pop();
                return;
            }
            "function_item" => {
                let mut direct_calls = Vec::new();
                let test_opt = self.test_fn(node, mods, &mut direct_calls);
                let is_test = test_opt.is_some();
                if let Some(test) = test_opt {
                    self.facts.tests.push(test);
                    self.test_calls.push(direct_calls);
                } else if let Some(name_node) = node.child_by_field_name("name") {
                    let fn_name = self.text(name_node).to_string();
                    let mut helper_test = TestFn::default();
                    let is_fallible_return = node
                        .child_by_field_name("return_type")
                        .map(|rt| {
                            let text = self.text(rt);
                            text.contains("Result") || text.contains("Option")
                        })
                        .unwrap_or(false);
                    let mut dummy_calls = Vec::new();
                    if let Some(body) = node.child_by_field_name("body") {
                        self.count_asserts(
                            body,
                            &mut helper_test,
                            is_fallible_return,
                            &mut dummy_calls,
                        );
                    }
                    let facts = HelperFacts {
                        total_asserts: helper_test.total_asserts,
                        strong_asserts: helper_test.strong_asserts,
                        tautologies: helper_test.tautologies,
                        fatal_asserts: helper_test.fatal_asserts,
                    };
                    self.helpers.insert(fn_name, facts);
                }
                self.in_fn += 1;
                if is_test {
                    self.in_test += 1;
                }
                self.visit_children(node, mods);
                if is_test {
                    self.in_test -= 1;
                }
                self.in_fn -= 1;
                return;
            }
            "const_item" => {
                self.in_const += 1;
                self.visit_children(node, mods);
                self.in_const -= 1;
                return;
            }
            "macro_invocation" => {
                if self.in_test == 0 {
                    if let Some(m) = node.child_by_field_name("macro") {
                        let full_name = self.text(m);
                        let short_name = last_segment(full_name);
                        let is_cta = if self.in_const > 0 {
                            self.is_assert_macro(short_name)
                                || short_name.starts_with("const_assert")
                                || full_name.contains("static_assertions")
                        } else if self.in_fn == 0 {
                            short_name.starts_with("const_assert")
                                || full_name.contains("static_assertions")
                                || short_name.starts_with("assert_")
                        } else {
                            false
                        };
                        if is_cta {
                            self.facts.compile_time_asserts += 1;
                            if self.facts.compile_time_assert_line.is_none() {
                                self.facts.compile_time_assert_line =
                                    Some(node.start_position().row + 1);
                            }
                        }
                    }
                }
            }
            "unsafe_block" => self.unsafe_site(node, "unsafe block"),
            "impl_item" => {
                let mut cursor = node.walk();
                if node.children(&mut cursor).any(|c| c.kind() == "unsafe") {
                    self.unsafe_site(node, "unsafe impl");
                }
                self.visit_children(node, mods);
                return;
            }
            "trait_item" => {
                let mut cursor = node.walk();
                if node.children(&mut cursor).any(|c| c.kind() == "unsafe") {
                    self.unsafe_site(node, "unsafe trait");
                }
                self.visit_children(node, mods);
                return;
            }
            _ => {}
        }
        self.visit_children(node, mods);
    }

    fn visit_children(&mut self, node: Node, mods: &mut Vec<String>) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.visit(child, mods);
        }
    }

    fn resolve_same_file_helpers(&mut self) {
        for (i, test) in self.facts.tests.iter_mut().enumerate() {
            if let Some(calls) = self.test_calls.get(i) {
                for call in calls {
                    if let Some(h) = self.helpers.get(call) {
                        if self.vocab.helper_fns.iter().any(|name| name == call) {
                            test.total_asserts = test.total_asserts.saturating_sub(1);
                        }
                        test.total_asserts += h.total_asserts;
                        test.strong_asserts += h.strong_asserts;
                        test.tautologies += h.tautologies;
                        test.fatal_asserts += h.fatal_asserts;
                    }
                }
            }
        }
    }

    fn test_fn(
        &self,
        node: Node,
        mods: &[String],
        direct_calls: &mut Vec<String>,
    ) -> Option<TestFn> {
        let mut is_test = false;
        let mut ignored = false;
        let mut conditional_ignore = None;
        let mut should_panic = false;
        let mut has_commented_out_test = false;
        let mut prev = node.prev_sibling();
        while let Some(p) = prev {
            match p.kind() {
                "attribute_item" => {
                    let text = self.text(p);
                    let name = attribute_name(text);
                    let mut check_attr = |n: &str| match n {
                        "test" | "rstest" | "test_case" | "quickcheck" => is_test = true,
                        "ignore" => ignored = true,
                        "should_panic" => should_panic = true,
                        _ => {}
                    };
                    check_attr(&name);
                    if name == "cfg_attr" {
                        if let Some((cond, subs)) = parse_cfg_attr(text) {
                            for sub in subs {
                                let sub_name = attribute_name(&sub);
                                if matches!(
                                    sub_name.as_str(),
                                    "test" | "rstest" | "test_case" | "quickcheck"
                                ) {
                                    is_test = true;
                                } else if sub_name == "ignore" {
                                    let norm = cond.replace(' ', "");
                                    if norm == "all()" || norm == "test" {
                                        ignored = true;
                                    } else {
                                        conditional_ignore = Some(cond.clone());
                                    }
                                } else if sub_name == "should_panic" {
                                    should_panic = true;
                                }
                            }
                        }
                    }
                    if name == "cfg" && is_cfg_test_suppression(text) {
                        ignored = true;
                    }
                    prev = p.prev_sibling();
                }
                "line_comment" | "block_comment" => {
                    let text = self.text(p);
                    if is_commented_out_test(text) {
                        has_commented_out_test = true;
                    }
                    prev = p.prev_sibling();
                }
                _ => break,
            }
        }
        if !is_test && has_commented_out_test {
            is_test = true;
            ignored = true;
        }
        if !is_test {
            return None;
        }

        let name = self.text(node.child_by_field_name("name")?);
        let qualified = mods
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(name))
            .collect::<Vec<_>>()
            .join("::");

        let mut test = TestFn {
            name: qualified,
            line: node.start_position().row + 1,
            total_asserts: 0,
            strong_asserts: 0,
            tautologies: 0,
            ignored,
            conditional_ignore,
            fatal_asserts: 0,
            should_panic,
        };
        let is_fallible_return = node
            .child_by_field_name("return_type")
            .map(|rt| {
                let text = self.text(rt);
                text.contains("Result") || text.contains("Option")
            })
            .unwrap_or(false);
        if let Some(body) = node.child_by_field_name("body") {
            self.count_asserts(body, &mut test, is_fallible_return, direct_calls);
        }
        Some(test)
    }

    fn count_asserts(
        &self,
        node: Node,
        test: &mut TestFn,
        is_fallible_return: bool,
        direct_calls: &mut Vec<String>,
    ) {
        match node.kind() {
            "function_item" => {
                // Do not recurse into nested function items.
                return;
            }
            "try_expression" => {
                if is_fallible_return {
                    test.total_asserts += 1;
                }
            }
            "macro_invocation" => {
                if let Some(m) = node.child_by_field_name("macro") {
                    let name = last_segment(self.text(m));
                    if self.is_assert_macro(name) {
                        test.total_asserts += 1;
                        if is_strong(name) {
                            test.strong_asserts += 1;
                        }
                        let args = node
                            .children(&mut node.walk())
                            .find(|c| c.kind() == "token_tree")
                            .map(|t| self.text(t))
                            .unwrap_or("");
                        if is_tautology(name, args) {
                            test.tautologies += 1;
                        }
                    }
                }
            }
            "call_expression" => {
                if let Some(f) = node.child_by_field_name("function") {
                    if f.kind() == "field_expression" {
                        if let Some(field) = f.child_by_field_name("field") {
                            let method = self.text(field);
                            if method == "unwrap" || method == "expect" {
                                test.total_asserts += 1;
                            }
                        }
                    }
                    let name = last_segment(self.text(f));
                    direct_calls.push(name.to_string());
                    if self.vocab.helper_fns.iter().any(|h| h == name) {
                        test.total_asserts += 1;
                    }
                }
            }
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.count_asserts(child, test, is_fallible_return, direct_calls);
        }
    }

    fn is_assert_macro(&self, name: &str) -> bool {
        name.starts_with("assert")
            || name.starts_with("debug_assert")
            || name.starts_with("prop_assert")
            || self.vocab.extra_macros.iter().any(|m| m == name)
    }

    fn unsafe_site(&mut self, node: Node, kind: &'static str) {
        let documented = self.is_documented(node);
        let row = node.start_position().row;
        let snippet = self
            .lines
            .get(row)
            .map(|l| l.trim().to_string())
            .unwrap_or_default();
        let site = UnsafeSite {
            line: row + 1,
            kind,
            documented,
            snippet: snippet.clone(),
        };
        self.facts.unsafe_sites.push(site);
        self.facts
            .escape_hatches
            .push(EscapeHatchSite::UnsafeBlock {
                line: row + 1,
                kind,
                documented,
                snippet,
            });
    }

    /// A site is documented when a `SAFETY:` comment sits in the contiguous
    /// comment / attribute run directly above the unsafe node or above any
    /// ancestor up to its enclosing statement, or inline between the start of
    /// that statement and the `unsafe` keyword.
    fn is_documented(&self, node: Node) -> bool {
        let mut rows = vec![node.start_position().row];
        let mut statement = node;
        while let Some(parent) = statement.parent() {
            if matches!(
                parent.kind(),
                "block" | "source_file" | "declaration_list" | "unsafe_block"
            ) {
                break;
            }
            statement = parent;
            rows.push(statement.start_position().row);
        }
        rows.dedup();

        if rows.iter().any(|&row| self.safety_run_above(row)) {
            return true;
        }
        self.comments.iter().any(|c| {
            c.has_safety
                && c.start_byte >= statement.start_byte()
                && c.end_byte <= node.start_byte()
        })
    }

    fn safety_run_above(&self, row: usize) -> bool {
        let mut row = row;
        let mut run_comments = Vec::new();
        while row > 0 {
            row -= 1;
            let line = self.lines.get(row).copied().unwrap_or("").trim_start();
            if let Some(c) = self.comment_on_row(row) {
                run_comments.push(c);
                row = c.start_row;
            } else if line.starts_with("#[") {
                continue;
            } else {
                break;
            }
        }
        if run_comments.is_empty() {
            return false;
        }
        if run_comments.iter().any(|c| c.has_safety) {
            return true;
        }
        run_comments.reverse();
        let mut combined = String::new();
        for c in run_comments {
            let start = c.start_byte;
            let end = c.end_byte;
            if end <= self.src.len() && start < end {
                if let Ok(text) = std::str::from_utf8(&self.src[start..end]) {
                    combined.push_str(text);
                    combined.push('\n');
                }
            }
        }
        has_valid_safety_comment_with_placeholders(&combined, &self.vocab.safety_placeholders)
    }

    /// A comment node that *begins the line* on `row` (or spans it), so a
    /// string literal that merely contains `// SAFETY:` does not qualify.
    fn comment_on_row(&self, row: usize) -> Option<&Comment> {
        let line = self.lines.get(row).copied().unwrap_or("");
        let indent = line.len() - line.trim_start().len();
        let line_start = self.line_starts.get(row).copied().unwrap_or(0);
        self.comments.iter().find(|c| {
            (c.start_row < row && c.end_row >= row)
                || (c.start_row == row && c.start_byte == line_start + indent)
        })
    }
}

#[allow(dead_code)]
pub(crate) fn has_valid_safety_comment(text: &str) -> bool {
    has_valid_safety_comment_with_placeholders(text, &[])
}

pub(crate) fn has_valid_safety_comment_with_placeholders(
    text: &str,
    custom_placeholders: &[String],
) -> bool {
    let default_list = crate::config::DEFAULT_SAFETY_PLACEHOLDERS;
    let placeholders: Vec<String> = if custom_placeholders.is_empty() {
        default_list.iter().map(|s| s.to_lowercase()).collect()
    } else {
        custom_placeholders
            .iter()
            .map(|s| s.to_lowercase())
            .collect()
    };

    let mut multi_word = Vec::new();
    let mut single_word = std::collections::HashSet::new();
    for p in &placeholders {
        let p = p.trim();
        if p.contains(' ') {
            multi_word.push(p.to_string());
        } else if !p.is_empty() {
            single_word.insert(p.to_string());
        }
    }

    let mut search_from = 0;
    while let Some(rel_idx) = text[search_from..].find("SAFETY:") {
        let idx = search_from + rel_idx + "SAFETY:".len();
        let after = &text[idx..];

        let mut comment_lines = Vec::new();
        for line in after.lines() {
            let trimmed = line
                .trim()
                .trim_start_matches('/')
                .trim_start_matches('*')
                .trim_end_matches('*')
                .trim_end_matches('/')
                .trim();
            if trimmed.is_empty() && !comment_lines.is_empty() {
                break;
            }
            comment_lines.push(trimmed);
        }
        let mut comment_text = comment_lines.join(" ").to_lowercase();

        for mw in &multi_word {
            comment_text = comment_text.replace(mw, " ");
        }

        let substantive_words = comment_text
            .split_whitespace()
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_'))
            .filter(|w| !w.is_empty())
            .filter(|w| {
                let w_lower = w.to_lowercase();
                if single_word.contains(&w_lower) {
                    return false;
                }
                if w_lower == "unsafe" || w_lower == "safety" {
                    return false;
                }
                true
            })
            .count();

        if substantive_words > 0 {
            return true;
        }
        search_from = idx;
    }
    false
}

fn is_cfg_test_suppression(attr_text: &str) -> bool {
    let normalized: String = attr_text.chars().filter(|c| !c.is_whitespace()).collect();
    let lower = normalized.to_lowercase();
    lower.contains("not(ci)")
        || lower.contains("not(test)")
        || lower.contains("not(any(ci")
        || lower.contains("not(all(ci")
        || lower.contains("skip_ci")
        || lower.contains("ci_skip")
}

fn is_commented_out_test(comment_text: &str) -> bool {
    for line in comment_text.lines() {
        let trimmed = line
            .trim()
            .trim_start_matches('/')
            .trim_start_matches('*')
            .trim_end_matches('*')
            .trim_end_matches('/')
            .trim();
        if trimmed.starts_with("#[") || trimmed.starts_with("#![") {
            let attr = attribute_name(trimmed);
            if matches!(
                attr.as_str(),
                "test" | "rstest" | "test_case" | "quickcheck"
            ) {
                return true;
            }
        }
    }
    false
}

fn parse_cfg_attr(attr_text: &str) -> Option<(String, Vec<String>)> {
    let start = attr_text.find("cfg_attr")?;
    let rest = &attr_text[start + "cfg_attr".len()..];
    let open_paren = rest.find('(')?;
    let inside = &rest[open_paren + 1..];

    // Find first comma at paren depth 0 (relative to inside)
    let mut depth = 0;
    let mut condition_end = None;
    for (i, c) in inside.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            ',' if depth == 0 => {
                condition_end = Some(i);
                break;
            }
            _ => {}
        }
    }

    let comma_pos = condition_end?;
    let condition = inside[..comma_pos].trim().to_string();

    let sub_attrs_text = &inside[comma_pos + 1..];
    let mut sub_attrs = Vec::new();
    let mut current = String::new();
    let mut depth = 0;
    for c in sub_attrs_text.chars() {
        match c {
            '(' | '[' | '{' => {
                depth += 1;
                current.push(c);
            }
            ')' if depth == 0 => {
                break;
            }
            ')' | ']' | '}' => {
                depth -= 1;
                current.push(c);
            }
            ',' if depth == 0 => {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    sub_attrs.push(trimmed.to_string());
                }
                current.clear();
            }
            _ => current.push(c),
        }
    }
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        sub_attrs.push(trimmed.to_string());
    }

    Some((condition, sub_attrs))
}

fn attribute_name(attr_text: &str) -> String {
    // `#[tokio::test(flavor = "multi_thread")]` -> `test`
    let inner = attr_text
        .trim_start_matches('#')
        .trim_start_matches('!')
        .trim_start_matches('[')
        .trim();
    let path: String = inner
        .chars()
        .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | ':'))
        .collect();
    last_segment(&path).to_string()
}

fn last_segment(path: &str) -> &str {
    path.rsplit("::").next().unwrap_or(path).trim()
}

fn is_strong(name: &str) -> bool {
    name.contains("_eq") || name.contains("_ne") || name.contains("matches")
}

fn is_tautology(name: &str, token_tree: &str) -> bool {
    let inner = token_tree
        .trim()
        .trim_start_matches(['(', '[', '{'])
        .trim_end_matches([')', ']', '}']);
    let args = split_top_level(inner);
    if name.contains("_eq") {
        if args.len() >= 2 && args[0].trim() == args[1].trim() && !args[0].trim().is_empty() {
            return true;
        }
        return args.len() >= 2 && is_constant_argument(args[0]) && is_constant_argument(args[1]);
    }
    if name.contains("_ne") || name.contains("matches") {
        return args.len() >= 2 && is_constant_argument(args[0]) && is_constant_argument(args[1]);
    }
    if let Some(first) = args.first() {
        return is_constant_argument(first);
    }
    false
}

fn is_constant_argument(arg: &str) -> bool {
    let trimmed = arg.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed == "true" || trimmed == "false" {
        return true;
    }
    let code = format!("fn _discipline_check() {{ let _ = ({trimmed}); }}");
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .is_err()
    {
        return false;
    }
    let Some(tree) = parser.parse(&code, None) else {
        return false;
    };
    let root = tree.root_node();
    if root.has_error() {
        return false;
    }
    let Some(fn_item) = root.child(0) else {
        return false;
    };
    let Some(body) = fn_item.child_by_field_name("body") else {
        return false;
    };
    let mut cursor = body.walk();
    let let_decl = body
        .children(&mut cursor)
        .find(|c| c.kind() == "let_declaration");
    let Some(let_node) = let_decl else {
        return false;
    };
    let Some(value_node) = let_node.child_by_field_name("value") else {
        return false;
    };

    let mut has_literal = false;
    let mut has_forbidden = false;

    check_constant_node(value_node, &mut has_literal, &mut has_forbidden);
    has_literal && !has_forbidden
}

fn check_constant_node(n: Node, has_literal: &mut bool, has_forbidden: &mut bool) {
    match n.kind() {
        "integer_literal" | "float_literal" | "boolean_literal" | "string_literal"
        | "char_literal" | "raw_string_literal" => {
            *has_literal = true;
        }
        "identifier"
        | "field_identifier"
        | "type_identifier"
        | "call_expression"
        | "field_expression"
        | "index_expression"
        | "macro_invocation"
        | "closure_expression"
        | "scoped_identifier"
        | "generic_type_with_arguments"
        | "ERROR" => {
            *has_forbidden = true;
            return;
        }
        _ => {}
    }
    let mut cursor = n.walk();
    for child in n.children(&mut cursor) {
        check_constant_node(child, has_literal, has_forbidden);
        if *has_forbidden {
            return;
        }
    }
}

fn split_top_level(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut start, mut in_str) = (0i32, 0usize, false);
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' if i == 0 || bytes[i - 1] != b'\\' => in_str = !in_str,
            _ if in_str => {}
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(src: &str) -> ParsedFileFacts {
        RustPack
            .extract("test.rs", src, &AssertVocabulary::default())
            .expect("analyze")
    }

    #[test]
    fn counts_assertions_per_test_and_ignores_comments_and_strings() {
        let f = facts(
            r#"
#[test]
fn real() {
    // assert_eq!(1, 2);
    let _s = "assert!(false)";
    let x = 1;
    assert_eq!(x + 1, 2);
    assert!(x > 0);
}
fn not_a_test() { assert!(true); }
"#,
        );
        assert_eq!(f.tests.len(), 1);
        assert_eq!(f.tests[0].name, "real");
        assert_eq!(f.tests[0].total_asserts, 2);
        assert_eq!(f.tests[0].strong_asserts, 1);
        assert!(!f.tests[0].is_vacuous());
    }

    #[test]
    fn detects_vacuous_and_tautological_tests() {
        let f = facts(
            r#"
mod tests {
    #[test] fn empty() {}
    #[test] fn tautology() { assert!(true); assert_eq!(1, 1); }
    #[test] #[should_panic(expected = "boom")] fn panics() { boom(); }
    #[tokio::test] async fn real() { assert_eq!(f().await, 3); }
}
"#,
        );
        let by_name = |n: &str| f.tests.iter().find(|t| t.name == n).unwrap().clone();
        assert!(by_name("tests::empty").is_vacuous());
        assert!(by_name("tests::tautology").is_vacuous());
        assert_eq!(by_name("tests::tautology").tautologies, 2);
        assert!(!by_name("tests::panics").is_vacuous());
        assert!(!by_name("tests::real").is_vacuous());
    }

    #[test]
    fn helper_fns_and_extra_macros_count_only_when_configured() {
        let src = "#[test] fn t() { check_invariants(&x); verify!(x); }";
        assert!(facts(src).tests[0].is_vacuous());
        let vocab = AssertVocabulary {
            extra_macros: vec!["verify".into()],
            helper_fns: vec!["check_invariants".into()],
            ..Default::default()
        };
        let f = RustPack.extract("test.rs", src, &vocab).unwrap();
        assert_eq!(f.tests[0].total_asserts, 2);
    }

    #[test]
    fn ignore_attribute_is_detected() {
        let f = facts("#[test]\n#[ignore = \"flaky\"]\nfn t() { assert_eq!(a(), 1); }");
        assert!(f.tests[0].ignored);

        let f2 = facts("#[test]\n#[cfg_attr(all(), ignore)]\nfn t2() { assert_eq!(a(), 1); }");
        assert!(f2.tests[0].ignored);

        let f3 = facts(
            "#[cfg_attr(feature = \"ignore_something\", test)]\nfn t3() { assert_eq!(a(), 1); }",
        );
        assert!(!f3.tests[0].ignored);
        assert_eq!(f3.tests.len(), 1);

        let f4 = facts("// #[test]\nfn t4() { assert_eq!(a(), 1); }");
        assert_eq!(f4.tests.len(), 1);
        assert!(f4.tests[0].ignored);

        let f5 = facts("/* #[test] */\nfn t5() { assert_eq!(a(), 1); }");
        assert_eq!(f5.tests.len(), 1);
        assert!(f5.tests[0].ignored);

        let f6 = facts("#[test]\n#[cfg(not(ci))]\nfn t6() { assert_eq!(a(), 1); }");
        assert_eq!(f6.tests.len(), 1);
        assert!(f6.tests[0].ignored);

        let f7 = facts("#[test]\n#[cfg(not(test))]\nfn t7() { assert_eq!(a(), 1); }");
        assert_eq!(f7.tests.len(), 1);
        assert!(f7.tests[0].ignored);
    }

    #[test]
    fn safety_comment_above_block_or_statement_documents_it() {
        let f = facts(
            r#"
fn a(p: *const u8) -> u8 {
    // SAFETY: p is valid for reads.
    unsafe { *p }
}
fn b(p: *const u8) -> u8 {
    // SAFETY: p is valid for reads.
    let v = unsafe { *p };
    v
}
fn c(p: *const u8) -> u8 {
    let v = /* SAFETY: pointer is valid for reads */ unsafe { *p };
    v
}
// SAFETY: T is safe to send across threads.
unsafe impl Send for X {}
"#,
        );
        assert_eq!(f.unsafe_sites.len(), 4);
        assert!(
            f.unsafe_sites.iter().all(|s| s.documented),
            "{:?}",
            f.unsafe_sites
        );
        assert_eq!(f.escape_hatches.len(), 4);
    }

    #[test]
    fn undocumented_unsafe_is_flagged_even_without_a_space_before_the_brace() {
        let f = facts(
            r#"
fn a(p: *const u8) -> u8 { let v = unsafe{ *p }; v }
fn b(p: *const u8) -> u8 {
    // Safety: lower-case label does not count.
    unsafe { *p }
}
fn c(p: *const u8) -> u8 {
    let _doc = "// SAFETY: a string is not a comment";
    unsafe { *p }
}
fn d(p: *const u8) -> u8 {
    unsafe { *p } // SAFETY: trailing comment is after the fact
}
fn e(p: *const u8) -> u8 {
    // SAFETY: ok
    unsafe { *p }
}
fn f(p: *const u8) -> u8 {
    // SAFETY: valid
    unsafe { *p }
}
fn g(p: *const u8) -> u8 {
    // SAFETY: this is totally fine ok
    unsafe { *p }
}
unsafe impl Sync for X {}
"#,
        );
        assert_eq!(f.unsafe_sites.len(), 8);
        assert!(
            f.unsafe_sites.iter().all(|s| !s.documented),
            "{:?}",
            f.unsafe_sites
        );
        assert_eq!(f.escape_hatches.len(), 8);
    }

    #[test]
    fn the_word_unsafe_in_comments_and_strings_is_not_a_site() {
        let f = facts("// unsafe { }\nfn a() { let _ = \"unsafe { x }\"; }");
        assert!(f.unsafe_sites.is_empty());
        assert!(f.escape_hatches.is_empty());
    }

    #[test]
    fn parse_errors_are_surfaced() {
        assert!(facts("fn broken( {").has_parse_errors);
        assert!(!facts("fn fine() {}").has_parse_errors);
    }

    #[test]
    fn constant_expression_tautologies_are_flagged() {
        let f = facts(
            r#"
mod tests {
    #[test] fn t_const_eq() { assert_eq!(1, 1); }
    #[test] fn t_const_math() { assert!(1 + 1 > 0); }
    #[test] fn t_const_binary() { assert!(1 == 1); }
    #[test] fn t_const_ne() { assert_ne!(1, 2); }
    #[test] fn t_const_strings() { assert_ne!("a", "b"); }
    #[test] fn t_const_with_msg() { assert!(1 == 1, "failed with: {}", x); }
    #[test] fn t_ident_eq() { assert_eq!(x, x); }

    // NOT tautologies:
    #[test] fn real_call() { assert!(f() == 1); }
    #[test] fn real_ident() { assert_eq!(N, 4); }
    #[test] fn real_method() { assert!(x.len() > 0); }
    #[test] fn real_ne_ident() { assert_ne!(x, y); }
}
"#,
        );
        let by_name = |n: &str| f.tests.iter().find(|t| t.name == n).unwrap().clone();
        assert!(
            by_name("tests::t_const_eq").is_vacuous(),
            "t_const_eq should be vacuous"
        );
        assert!(
            by_name("tests::t_const_math").is_vacuous(),
            "t_const_math should be vacuous"
        );
        assert!(
            by_name("tests::t_const_binary").is_vacuous(),
            "t_const_binary should be vacuous"
        );
        assert!(
            by_name("tests::t_const_ne").is_vacuous(),
            "t_const_ne should be vacuous"
        );
        assert!(
            by_name("tests::t_const_strings").is_vacuous(),
            "t_const_strings should be vacuous"
        );
        assert!(
            by_name("tests::t_const_with_msg").is_vacuous(),
            "t_const_with_msg should be vacuous"
        );
        assert!(
            by_name("tests::t_ident_eq").is_vacuous(),
            "t_ident_eq should be vacuous"
        );

        assert!(
            !by_name("tests::real_call").is_vacuous(),
            "real_call should not be vacuous"
        );
        assert!(
            !by_name("tests::real_ident").is_vacuous(),
            "real_ident should not be vacuous"
        );
        assert!(
            !by_name("tests::real_method").is_vacuous(),
            "real_method should not be vacuous"
        );
        assert!(
            !by_name("tests::real_ne_ident").is_vacuous(),
            "real_ne_ident should not be vacuous"
        );
    }

    #[test]
    fn idiomatic_result_option_and_unwrap_assertions() {
        let f = facts(
            r#"
mod tests {
    #[test]
    fn parses() -> Result<(), Box<dyn std::error::Error>> {
        let _n: i32 = "4".parse()?;
        Ok(())
    }

    #[test]
    fn options() -> Option<()> {
        let _v = map.get(&k)?;
        Some(())
    }

    #[test]
    fn result_no_assert_is_vacuous() -> Result<(), Box<dyn std::error::Error>> {
        let _n = 4;
        Ok(())
    }

    #[test]
    fn unwrap_counts_as_assertion() {
        let _n: i32 = "4".parse().unwrap();
    }

    #[test]
    fn expect_counts_as_assertion() {
        let _n: i32 = "4".parse().expect("valid int");
    }
}
"#,
        );
        let by_name = |n: &str| f.tests.iter().find(|t| t.name == n).unwrap().clone();
        assert!(
            !by_name("tests::parses").is_vacuous(),
            "parses with ? should not be vacuous"
        );
        assert_eq!(by_name("tests::parses").total_asserts, 1);
        assert_eq!(by_name("tests::parses").strong_asserts, 0);

        assert!(
            !by_name("tests::options").is_vacuous(),
            "options with ? should not be vacuous"
        );
        assert_eq!(by_name("tests::options").total_asserts, 1);

        assert!(
            by_name("tests::result_no_assert_is_vacuous").is_vacuous(),
            "result without ? or assert must be vacuous"
        );
        assert_eq!(
            by_name("tests::result_no_assert_is_vacuous").total_asserts,
            0
        );

        assert!(
            !by_name("tests::unwrap_counts_as_assertion").is_vacuous(),
            "unwrap should count as assertion"
        );
        assert_eq!(
            by_name("tests::unwrap_counts_as_assertion").total_asserts,
            1
        );

        assert!(
            !by_name("tests::expect_counts_as_assertion").is_vacuous(),
            "expect should count as assertion"
        );
        assert_eq!(
            by_name("tests::expect_counts_as_assertion").total_asserts,
            1
        );
    }

    #[test]
    fn safety_comment_placeholders_and_substantive_discrimination() {
        // Substantive short comments pass regardless of word count
        assert!(has_valid_safety_comment(
            "// SAFETY: caller-checked non-null."
        ));
        assert!(has_valid_safety_comment(
            "// SAFETY: pointer is valid for reads"
        ));
        assert!(has_valid_safety_comment(
            "// SAFETY: index is bounded by length"
        ));

        // Hollow padding and placeholders fail
        assert!(!has_valid_safety_comment(
            "// SAFETY: this is totally fine ok"
        ));
        assert!(!has_valid_safety_comment("// SAFETY: todo"));
        assert!(!has_valid_safety_comment("// SAFETY: tbd"));
        assert!(!has_valid_safety_comment("// SAFETY: n/a"));
        assert!(!has_valid_safety_comment("// SAFETY: safe"));
        assert!(!has_valid_safety_comment("// SAFETY: ok"));
        assert!(!has_valid_safety_comment("// SAFETY: fine"));
        assert!(!has_valid_safety_comment("// SAFETY: trust me"));
        assert!(!has_valid_safety_comment("// SAFETY: safety"));
        assert!(!has_valid_safety_comment("// SAFETY: unsafe"));

        // Custom configurable placeholders
        let custom = vec!["custom_filler".to_string()];
        assert!(!has_valid_safety_comment_with_placeholders(
            "// SAFETY: custom_filler",
            &custom
        ));
        assert!(has_valid_safety_comment_with_placeholders(
            "// SAFETY: custom_filler with non_null pointer",
            &custom
        ));
    }

    #[test]
    fn compile_time_assertions_outside_tests_are_extracted() {
        let src = r#"
struct MyStruct {
    a: u64,
    b: u64,
}

const _: () = assert!(std::mem::size_of::<MyStruct>() == 16);
const _: () = {
    assert!(std::mem::align_of::<MyStruct>() == 8);
    assert_eq!(std::mem::size_of::<u64>(), 8);
};

static_assertions::assert_eq_size!(MyStruct, [u8; 16]);
const_assert!(std::mem::size_of::<MyStruct>() > 0);

fn runtime_fn(x: i32) {
    assert!(x > 0);
}

#[test]
fn normal_test() {
    assert_eq!(1, 1);
}
"#;
        let pack = RustPack;
        let facts = pack
            .extract("src/lib.rs", src, &AssertVocabulary::default())
            .unwrap();

        assert_eq!(facts.compile_time_asserts, 5);
        assert_eq!(facts.compile_time_assert_line, Some(7));
        assert!(facts.compile_time_test.is_some());
        let ctt = facts.compile_time_test.unwrap();
        assert_eq!(ctt.name, "compile-time-assertions");
        assert_eq!(ctt.total_asserts, 5);
        assert_eq!(ctt.strong_asserts, 5);
        assert_eq!(facts.tests.len(), 1);
    }

    #[test]
    fn cfg_attr_miri_ignore_is_conditional_not_unconditional() {
        let src = r#"
#[test]
#[cfg_attr(miri, ignore)]
fn test_miri_skipped() {
    assert_eq!(1, 1);
}

#[test]
#[cfg_attr(all(), ignore)]
fn test_unconditional_skipped() {
    assert_eq!(1, 1);
}
"#;
        let pack = RustPack;
        let facts = pack
            .extract("tests/cfg_attr.rs", src, &AssertVocabulary::default())
            .unwrap();

        let miri_test = facts
            .tests
            .iter()
            .find(|t| t.name == "test_miri_skipped")
            .unwrap();
        assert!(
            !miri_test.ignored,
            "miri test must NOT be unconditionally ignored"
        );
        assert_eq!(miri_test.conditional_ignore.as_deref(), Some("miri"));

        let unspec_test = facts
            .tests
            .iter()
            .find(|t| t.name == "test_unconditional_skipped")
            .unwrap();
        assert!(
            unspec_test.ignored,
            "all() condition must be unconditionally ignored"
        );
    }

    #[test]
    fn same_file_helper_functions_are_resolved_for_tests() {
        let src = r#"
fn assert_roundtrip(x: i32) {
    assert_eq!(x, x);
    assert_ne!(x, x + 1);
}

#[test]
fn test_via_helper() {
    assert_roundtrip(42);
}
"#;
        let pack = RustPack;
        let facts = pack
            .extract("tests/helper.rs", src, &AssertVocabulary::default())
            .unwrap();

        let test = facts
            .tests
            .iter()
            .find(|t| t.name == "test_via_helper")
            .unwrap();
        assert!(
            !test.is_vacuous(),
            "test calling helper must not be vacuous"
        );
        assert_eq!(test.total_asserts, 2);
        assert_eq!(test.strong_asserts, 2);
    }
}
