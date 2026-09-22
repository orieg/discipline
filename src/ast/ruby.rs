//! Ruby language pack: tree-sitter AST extraction of tests, assertions, and escape hatches.

use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser};

use super::functions::{self, FunctionSpec};
use super::{AssertVocabulary, EscapeHatchSite, Fact, LanguagePack, ParsedFileFacts, TestFn};

/// Ruby language pack implementing [`LanguagePack`].
pub struct RubyPack;

impl LanguagePack for RubyPack {
    fn id(&self) -> &'static str {
        "ruby"
    }

    fn supplies(&self, fact: Fact) -> bool {
        matches!(
            fact,
            Fact::Tests | Fact::EscapeHatches | Fact::Functions | Fact::Handlers | Fact::Prose
        )
    }

    fn name(&self) -> &'static str {
        "Ruby"
    }

    fn matches(&self, path: &str) -> bool {
        matches!(super::extension(path), Some("rb" | "rake" | "gemspec"))
            || path.ends_with("Rakefile")
            || path.ends_with("Gemfile")
    }

    fn extract(&self, path: &str, src: &str, vocab: &AssertVocabulary) -> Result<ParsedFileFacts> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_ruby::LANGUAGE.into())
            .map_err(|e| anyhow!("failed to load the Ruby grammar: {e}"))?;
        let tree = parser
            .parse(src, None)
            .ok_or_else(|| anyhow!("tree-sitter returned no tree"))?;
        let root = tree.root_node();

        let mut extractor = RubyExtractor {
            dead: super::reach::dead_ranges(root, src, &RUBY_REACH),
            src: src.as_bytes(),
            vocab,
            is_test_path: is_ruby_test_path(path),
            facts: ParsedFileFacts {
                has_parse_errors: root.has_error(),
                ..Default::default()
            },
            helpers: std::collections::HashMap::new(),
            test_calls: Vec::new(),
        };

        extractor.collect_comments_and_escape_hatches(root);
        extractor.visit_root(root);
        extractor.resolve_same_file_helpers();
        extractor.facts.functions = functions::extract(root, src, path, &RUBY_FUNCTIONS);
        super::mocks::count(
            root,
            src,
            &mut extractor.facts.tests,
            &RUBY_MOCKS,
            &vocab.mock_setup_fns,
            &vocab.mock_assert_fns,
        );
        {
            let tests = &extractor.facts.tests;
            let spans: Vec<(usize, usize)> = tests
                .iter()
                .map(|t| (t.line, t.end_line.max(t.line)))
                .collect();
            let whole_file = is_ruby_test_path(path)
                || functions::test_path(path)
                || functions::declared_test_path(path, &vocab.test_paths);
            let is_test_line =
                |l: usize| whole_file || spans.iter().any(|(a, b)| *a <= l && l <= *b);
            extractor.facts.swallowed =
                super::handlers::extract(root, src, &RUBY_HANDLERS, &is_test_line);
        }
        super::retries::mark(root, src, &mut extractor.facts.tests, &RUBY_RETRIES);
        if functions::declared_test_path(path, &vocab.test_paths) {
            for f in &mut extractor.facts.functions {
                f.is_test = true;
            }
        }
        super::calls::count(
            root,
            src,
            &mut extractor.facts.tests,
            &RUBY_MOCKS,
            super::calls::SLEEP_VOCAB,
            super::calls::sleeps,
        );
        super::calls::count(
            root,
            src,
            &mut extractor.facts.tests,
            &RUBY_MOCKS,
            super::calls::TRIVIAL_ASSERT_VOCAB,
            super::calls::trivial_asserts,
        );
        extractor.facts.prose =
            super::prose::extract(root, src, &["comment", "string", "heredoc_body"]);
        Ok(extractor.facts)
    }
}

fn ruby_fn_is_test(node: Node, src: &str, path: &str) -> bool {
    let name = node
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(src.as_bytes()).ok())
        .unwrap_or("");
    name.starts_with("test_")
        || name == "test"
        || is_ruby_test_path(path)
        || functions::test_path(path)
}

pub const RUBY_FUNCTIONS: FunctionSpec = FunctionSpec {
    // `def x; end` has no body node and is not described.
    function_kinds: &["method", "singleton_method"],
    name_fields: &["name"],
    body_fields: &["body"],
    ignored_kinds: &["comment"],
    skip: functions::skip_none,
    is_test: ruby_fn_is_test,
    classify: functions::classify_ruby,
};

pub const RUBY_MOCKS: super::mocks::MockSpec = super::mocks::MockSpec {
    call_kinds: &["call"],
    callee_fields: &["method"],
};

pub const RUBY_HANDLERS: super::handlers::HandlerSpec = super::handlers::HandlerSpec {
    // A `rescue` with no body has no `body` field and is judged by its own text.
    handler_kinds: &["rescue"],
    body_fields: &["body"],
    ignored_kinds: &["comment"],
    trivial: &[
        "nil",
        "false",
        "return",
        "return nil",
        "return false",
        "next",
        "[]",
        "{}",
    ],
    discard_kinds: &[],
    discards: super::handlers::no_discard,
    classify_discard: None,
    call_value_kinds: &[],
    // `call rescue nil`: the modifier form, when its handler is a constant.
    silence_kinds: &["rescue_modifier"],
    silences: super::handlers::ruby_silences,
};

pub const RUBY_RETRIES: super::retries::RetrySpec = super::retries::RetrySpec {
    marker_kinds: &["call"],
};

/// Determines whether a path is conventionally a Ruby test file.
pub fn is_ruby_test_path(path: &str) -> bool {
    let filename = path.rsplit('/').next().unwrap_or(path);
    filename.starts_with("test_")
        || filename.ends_with("_test.rb")
        || filename.ends_with("_spec.rb")
        || path.starts_with("test/")
        || path.contains("/test/")
        || path.starts_with("tests/")
        || path.contains("/tests/")
        || path.starts_with("spec/")
        || path.contains("/spec/")
}

struct RubyExtractor<'a> {
    /// Byte ranges no execution reaches (`super::reach`).
    dead: super::reach::DeadRanges,
    src: &'a [u8],
    vocab: &'a AssertVocabulary,
    is_test_path: bool,
    facts: ParsedFileFacts,
    helpers: std::collections::HashMap<String, super::HelperFacts>,
    test_calls: Vec<Vec<String>>,
}

impl<'a> RubyExtractor<'a> {
    fn text(&self, node: Node) -> &'a str {
        node.utf8_text(self.src).unwrap_or("")
    }

    fn collect_comments_and_escape_hatches(&mut self, node: Node) {
        let kind = node.kind();
        if kind == "comment" {
            let text = self.text(node).trim();
            let line = node.start_position().row + 1;
            if text.starts_with("# rubocop:disable") || text.starts_with("# rubocop:todo") {
                let rule = text
                    .trim_start_matches("# rubocop:disable")
                    .trim_start_matches("# rubocop:todo")
                    .trim();
                self.facts
                    .escape_hatches
                    .push(EscapeHatchSite::LinterDisable {
                        line,
                        rule: rule.to_string(),
                        snippet: text.to_string(),
                    });
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_comments_and_escape_hatches(child);
        }
    }

    fn visit_root(&mut self, root: Node) {
        let mut class_stack = Vec::new();
        self.walk_scope(root, &mut class_stack, false);
    }

    fn walk_scope(&mut self, scope: Node, class_stack: &mut Vec<String>, parent_skipped: bool) {
        let mut cursor = scope.walk();
        for child in scope.children(&mut cursor) {
            let kind = child.kind();
            if kind == "class" || kind == "module" {
                let name = child
                    .child_by_field_name("name")
                    .map(|n| self.text(n).to_string())
                    .unwrap_or_default();
                class_stack.push(name);
                if let Some(body) = child.child_by_field_name("body") {
                    self.walk_scope(body, class_stack, parent_skipped);
                }
                class_stack.pop();
            } else if kind == "method" || kind == "singleton_method" {
                let name_node = child.child_by_field_name("name");
                let method_name = name_node.map(|n| self.text(n)).unwrap_or("");
                let is_test = method_name.starts_with("test_") || method_name == "test";

                if is_test {
                    if let Some((test_fn, calls)) =
                        self.extract_method(child, class_stack, parent_skipped)
                    {
                        self.facts.tests.push(test_fn);
                        self.test_calls.push(calls);
                    }
                } else if self.is_test_path {
                    if let Some(body) = child.child_by_field_name("body") {
                        let mut helper_fn = TestFn::default();
                        let mut dummy_calls = Vec::new();
                        self.extract_assertions_in_body(body, &mut helper_fn, &mut dummy_calls);
                        helper_fn.total_asserts += super::count_failure_exits(
                            body,
                            self.src,
                            &["call", "identifier"],
                            &["raise ", "raise(", "fail "],
                            &["block", "do_block", "lambda", "method", "singleton_method"],
                        );
                        let facts = super::HelperFacts {
                            total_asserts: helper_fn.total_asserts,
                            strong_asserts: helper_fn.strong_asserts,
                            tautologies: helper_fn.tautologies,
                            fatal_asserts: helper_fn.fatal_asserts,
                        };
                        self.helpers.insert(method_name.to_string(), facts);
                    }
                }
            } else if kind == "call" {
                if let Some((test_fn, calls)) =
                    self.extract_block_test(child, class_stack, parent_skipped)
                {
                    self.facts.tests.push(test_fn);
                    self.test_calls.push(calls);
                } else if let Some(block) = child.child_by_field_name("block") {
                    // Check for describe/context blocks
                    let method_name = child
                        .child_by_field_name("method")
                        .map(|m| self.text(m))
                        .unwrap_or("");
                    if matches!(
                        method_name,
                        "describe" | "context" | "feature" | "xdescribe" | "xcontext"
                    ) {
                        let is_block_skipped = parent_skipped
                            || method_name.starts_with('x')
                            || self.has_skip_metadata(child);
                        if let Some(body) = block.child_by_field_name("body") {
                            self.walk_scope(body, class_stack, is_block_skipped);
                        } else {
                            self.walk_scope(block, class_stack, is_block_skipped);
                        }
                    }
                }
            }
        }
    }

    fn extract_method(
        &self,
        node: Node,
        class_stack: &[String],
        parent_skipped: bool,
    ) -> Option<(TestFn, Vec<String>)> {
        let name_node = node.child_by_field_name("name")?;
        let method_name = self.text(name_node);

        let full_name = if class_stack.is_empty() {
            method_name.to_string()
        } else {
            format!("{}#{}", class_stack.join("::"), method_name)
        };

        let line = node.start_position().row + 1;
        let end_line = node.end_position().row + 1;
        let mut test_fn = TestFn {
            name: full_name,
            line,
            end_line,
            total_asserts: 0,
            strong_asserts: 0,
            tautologies: 0,
            ignored: parent_skipped,
            should_panic: false,
            ..Default::default()
        };

        let mut direct_calls = Vec::new();
        if let Some(body) = node.child_by_field_name("body") {
            self.extract_assertions_in_body(body, &mut test_fn, &mut direct_calls);
        }

        Some((test_fn, direct_calls))
    }

    fn extract_block_test(
        &self,
        node: Node,
        class_stack: &[String],
        parent_skipped: bool,
    ) -> Option<(TestFn, Vec<String>)> {
        // e.g. it "does something" do ... end
        // test "description" do ... end
        // specify "something" do ... end
        // xit "skipped" do ... end
        let method_name = node
            .child_by_field_name("method")
            .map(|m| self.text(m))
            .or_else(|| {
                // If it's a simple call without receiver, first named child might be identifier
                node.child(0)
                    .filter(|c| c.kind() == "identifier")
                    .map(|c| self.text(c))
            })?;

        let is_it = matches!(method_name, "it" | "specify" | "example" | "test");
        let is_xit = matches!(method_name, "xit" | "xspecify" | "xexample");

        if !is_it && !is_xit {
            return None;
        }

        let block = node.child_by_field_name("block").or_else(|| {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if matches!(child.kind(), "block" | "do_block") {
                    return Some(child);
                }
            }
            None
        });

        let line = node.start_position().row + 1;
        let desc = self.get_call_string_argument(node).unwrap_or(method_name);
        let full_name = if class_stack.is_empty() {
            desc.to_string()
        } else {
            format!("{} {}", class_stack.join("::"), desc)
        };

        let is_ignored = parent_skipped || is_xit || self.has_skip_metadata(node);
        let end_line = block
            .map(|b| b.end_position().row + 1)
            .unwrap_or_else(|| node.end_position().row + 1);

        let mut test_fn = TestFn {
            name: full_name,
            line,
            end_line,
            total_asserts: 0,
            strong_asserts: 0,
            tautologies: 0,
            ignored: is_ignored,
            should_panic: false,
            ..Default::default()
        };

        let mut direct_calls = Vec::new();
        if let Some(b) = block {
            if let Some(body) = b.child_by_field_name("body") {
                self.extract_assertions_in_body(body, &mut test_fn, &mut direct_calls);
            } else {
                self.extract_assertions_in_body(b, &mut test_fn, &mut direct_calls);
            }
        }

        Some((test_fn, direct_calls))
    }

    fn has_skip_metadata(&self, call_node: Node) -> bool {
        // Look for arguments: :skip, skip: true, skip: "reason"
        if let Some(args) = call_node.child_by_field_name("arguments") {
            let text = self.text(args);
            if text.contains(":skip") || text.contains("skip:") {
                return true;
            }
        }
        false
    }

    fn get_call_string_argument(&self, call_node: Node) -> Option<&'a str> {
        let args = call_node.child_by_field_name("arguments")?;
        let mut cursor = args.walk();
        for child in args.children(&mut cursor) {
            if child.kind() == "string"
                || child.kind() == "simple_symbol"
                || child.kind() == "symbol"
            {
                let raw = self.text(child);
                let trimmed = raw
                    .trim_matches('"')
                    .trim_matches('\'')
                    .trim_start_matches(':');
                return Some(trimmed);
            }
        }
        None
    }

    fn extract_assertions_in_body(
        &self,
        node: Node,
        test_fn: &mut TestFn,
        direct_calls: &mut Vec<String>,
    ) {
        if super::reach::is_dead(&self.dead, node.start_byte()) {
            return;
        }
        let kind = node.kind();
        if kind == "identifier" {
            let name = self.text(node);
            if matches!(name, "skip" | "omit" | "pending") {
                test_fn.ignored = true;
                return;
            }
        }

        if kind == "call" {
            let method_name = node
                .child_by_field_name("method")
                .map(|m| self.text(m))
                .unwrap_or("");

            let recv = node.child_by_field_name("receiver");
            let is_local_call = match recv {
                None => true,
                Some(r) => self.text(r).trim() == "self",
            };
            if is_local_call && !method_name.is_empty() {
                direct_calls.push(method_name.to_string());
            }

            if matches!(method_name, "skip" | "omit" | "pending") {
                test_fn.ignored = true;
                return;
            }

            if matches!(method_name, "to" | "not_to" | "to_not") {
                if let Some(recv) = node.child_by_field_name("receiver") {
                    if recv.kind() == "call" {
                        let recv_method = recv
                            .child_by_field_name("method")
                            .map(|m| self.text(m))
                            .unwrap_or("");
                        if recv_method == "expect" {
                            self.handle_rspec_expectation(node, recv, test_fn);
                            return;
                        }
                    }
                }
            }

            if self.is_assertion(method_name) {
                self.handle_assertion(node, method_name, test_fn);
                return;
            }

            if self.vocab.extra_macros.iter().any(|m| m == method_name)
                || self.vocab.helper_fns.iter().any(|h| h == method_name)
            {
                test_fn.total_asserts += 1;
                test_fn.strong_asserts += 1;
                return;
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.extract_assertions_in_body(child, test_fn, direct_calls);
        }
    }

    fn resolve_same_file_helpers(&mut self) {
        for (i, test) in self.facts.tests.iter_mut().enumerate() {
            if let Some(calls) = self.test_calls.get(i) {
                for call in calls {
                    if let Some(h) = self.helpers.get(call) {
                        if self.vocab.helper_fns.iter().any(|name| name == call) {
                            test.total_asserts = test.total_asserts.saturating_sub(1);
                            test.strong_asserts = test.strong_asserts.saturating_sub(1);
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
        }
    }

    fn handle_rspec_expectation(&self, to_call: Node, expect_call: Node, test_fn: &mut TestFn) {
        test_fn.total_asserts += 1;
        let args = self.get_call_arguments(to_call);
        let matcher = args.first();
        let matcher_name = matcher
            .and_then(|m| {
                if m.kind() == "call" {
                    m.child_by_field_name("method").map(|n| self.text(n))
                } else if m.kind() == "identifier" {
                    Some(self.text(*m))
                } else {
                    None
                }
            })
            .unwrap_or("");

        let is_weak = matches!(
            matcher_name,
            "be_nil" | "be_truthy" | "be_falsey" | "be_empty" | "be_true" | "be_false"
        );

        if !is_weak {
            test_fn.strong_asserts += 1;

            if matches!(matcher_name, "eq" | "eql" | "equal") {
                if let Some(matcher_node) = matcher {
                    let matcher_args = self.get_call_arguments(*matcher_node);
                    let expect_args = self.get_call_arguments(expect_call);
                    if let (Some(a), Some(b)) = (expect_args.first(), matcher_args.first()) {
                        if self.text(*a).trim() == self.text(*b).trim() {
                            test_fn.tautologies += 1;
                        }
                    }
                }
            }
        }
    }

    fn is_assertion(&self, name: &str) -> bool {
        name.starts_with("assert")
            || name.starts_with("refute")
            || name == "expect"
            || name == "should"
            || name == "should_not"
    }

    fn handle_assertion(&self, call_node: Node, method_name: &str, test_fn: &mut TestFn) {
        test_fn.total_asserts += 1;

        if method_name == "expect" {
            // Standalone expect call without .to
            test_fn.strong_asserts += 1;
            return;
        }

        let is_strong = matches!(
            method_name,
            "assert_equal"
                | "refute_equal"
                | "assert_same"
                | "refute_same"
                | "assert_match"
                | "refute_match"
                | "assert_includes"
                | "refute_includes"
                | "assert_in_delta"
                | "refute_in_delta"
                | "assert_in_epsilon"
                | "refute_in_epsilon"
                | "assert_raises"
                | "assert_output"
                | "assert_silent"
                | "assert_throws"
                | "assert_respond_to"
                | "refute_respond_to"
        );

        if is_strong {
            test_fn.strong_asserts += 1;

            if matches!(
                method_name,
                "assert_equal" | "assert_same" | "refute_equal" | "refute_same"
            ) {
                let args = self.get_call_arguments(call_node);
                if let (Some(a), Some(b)) = (args.first(), args.get(1)) {
                    if self.text(*a).trim() == self.text(*b).trim() {
                        test_fn.tautologies += 1;
                    }
                }
            }
        } else if matches!(method_name, "assert" | "refute") {
            let args = self.get_call_arguments(call_node);
            if let Some(first) = args.first() {
                let txt = self.text(*first).trim();
                if (method_name == "assert" && txt == "true")
                    || (method_name == "refute" && txt == "false")
                {
                    test_fn.tautologies += 1;
                }
            }
        }
    }

    fn get_call_arguments<'b>(&self, call_node: Node<'b>) -> Vec<Node<'b>> {
        let Some(args_node) = call_node.child_by_field_name("arguments") else {
            return Vec::new();
        };
        let mut cursor = args_node.walk();
        args_node
            .children(&mut cursor)
            .filter(|c| c.is_named())
            .collect()
    }
}

pub const RUBY_REACH: super::reach::ReachSpec = super::reach::ReachSpec {
    if_kinds: &["if"],
    block_kinds: &["then", "body_statement", "block_body"],
    ignored_kinds: &["comment"],
    terminators: &["return", "raise", "next", "break"],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ruby_pack_registration_and_extension_matching() {
        let pack = RubyPack;
        assert_eq!(pack.id(), "ruby");
        assert_eq!(pack.name(), "Ruby");
        assert!(pack.matches("test/test_foo.rb"));
        assert!(pack.matches("lib/tasks/setup.rake"));
        assert!(pack.matches("foo.gemspec"));
        assert!(pack.matches("Rakefile"));
        assert!(pack.matches("Gemfile"));
        assert!(!pack.matches("test.py"));
    }

    #[test]
    fn test_minitest_extraction_and_assertions() {
        let src = r#"
require "minitest/autorun"

class CalcTest < Minitest::Test
  def test_addition
    assert_equal 4, 2 + 2
    assert_match(/\d+/, "42")
    assert true
  end

  def test_skipped
    skip "work in progress"
    assert_equal 1, 1
  end
end
"#;
        let pack = RubyPack;
        let facts = pack
            .extract("test/test_calc.rb", src, &AssertVocabulary::default())
            .expect("extract succeeds");

        assert_eq!(facts.tests.len(), 2);
        let t1 = &facts.tests[0];
        assert_eq!(t1.name, "CalcTest#test_addition");
        assert_eq!(t1.total_asserts, 3);
        assert_eq!(t1.strong_asserts, 2); // assert_equal, assert_match
        assert_eq!(t1.tautologies, 1); // assert true
        assert!(!t1.ignored);

        let t2 = &facts.tests[1];
        assert_eq!(t2.name, "CalcTest#test_skipped");
        assert!(t2.ignored);
    }

    #[test]
    fn test_rspec_extraction_and_expectations() {
        let src = r#"
RSpec.describe Calculator do
  context "basic math" do
    it "adds numbers" do
      expect(1 + 1).to eq(2)
      expect(true).to be_truthy
    end

    xit "pending implementation" do
      expect(2 * 2).to eq(4)
    end

    it "skipped with tag", :skip do
      expect(3 * 3).to eq(9)
    end
  end
end
"#;
        let pack = RubyPack;
        let facts = pack
            .extract("spec/calc_spec.rb", src, &AssertVocabulary::default())
            .expect("extract succeeds");

        assert_eq!(facts.tests.len(), 3);
        let t1 = &facts.tests[0];
        assert_eq!(t1.name, "adds numbers");
        assert_eq!(t1.total_asserts, 2);
        assert_eq!(t1.strong_asserts, 1); // eq(2) is strong, be_truthy is weak
        assert!(!t1.ignored);

        let t2 = &facts.tests[1];
        assert_eq!(t2.name, "pending implementation");
        assert!(t2.ignored);

        let t3 = &facts.tests[2];
        assert_eq!(t3.name, "skipped with tag");
        assert!(t3.ignored);
    }

    #[test]
    fn test_ruby_tautologies_and_vacuous_tests() {
        let src = r#"
class TautologyTest < Minitest::Test
  def test_tautology_equal
    assert_equal 42, 42
  end

  def test_tautology_bool
    assert true
  end

  def test_empty
  end
end
"#;
        let pack = RubyPack;
        let facts = pack
            .extract("test/tautology_test.rb", src, &AssertVocabulary::default())
            .expect("extract succeeds");

        assert_eq!(facts.tests.len(), 3);
        assert!(facts.tests[0].is_vacuous());
        assert_eq!(facts.tests[0].tautologies, 1);

        assert!(facts.tests[1].is_vacuous());
        assert_eq!(facts.tests[1].tautologies, 1);

        assert!(facts.tests[2].is_vacuous());
        assert_eq!(facts.tests[2].total_asserts, 0);
    }

    #[test]
    fn test_ruby_escape_hatches_and_custom_vocab() {
        let src = r#"
# rubocop:disable Metrics/MethodLength
class CustomTest < Minitest::Test
  # rubocop:todo Style/FrozenStringLiteralComment
  def test_custom_helper
    custom_check_ok(result)
  end
end
"#;
        let mut vocab = AssertVocabulary::default();
        vocab.helper_fns.push("custom_check_ok".to_string());

        let pack = RubyPack;
        let facts = pack
            .extract("test/custom_test.rb", src, &vocab)
            .expect("extract succeeds");

        assert_eq!(facts.escape_hatches.len(), 2);
        assert_eq!(facts.tests.len(), 1);
        assert_eq!(facts.tests[0].total_asserts, 1);
        assert_eq!(facts.tests[0].strong_asserts, 1);
        assert!(!facts.tests[0].is_vacuous());
    }

    #[test]
    fn test_example_ruby_test_fixture() {
        let src = r#"
require "minitest/autorun"
require_relative "../lib/example"

class TestExample < Minitest::Test
  def test_version
    refute_nil Example.version
    assert_match(/\d+\.\d+\.\d+/, Example.version)
  end

  def test_set
    set = Example::Set.new
    assert_equal 0, set.size
    assert set.empty?

    assert set.add(42)
    assert set.add(100)
    assert set.add(10)
    refute set.add(42)

    assert_equal 3, set.size
    assert set.include?(42)
    assert set.include?(100)
    assert set.include?(10)
    refute set.include?(999)

    assert_equal 10, set.first
    assert_equal 100, set.last
    assert_equal 42, set.next(10)
    assert_equal 10, set.prev(42)

    assert_equal 1, set.rank(42)
    assert_equal 100, set.select(2)
    assert_equal 2, set.count_range(10, 42)

    items = []
    set.each { |k| items << k }
    assert_equal [10, 42, 100], items

    assert set.delete(42)
    refute set.include?(42)
    assert_equal 2, set.size

    set.clear
    assert_equal 0, set.size
  end

  def test_map
    map = Example::Map.new
    assert_equal 0, map.size

    map[10] = 100
    map[20] = 200
    map[30] = 300

    assert_equal 3, map.size
    assert_equal 100, map[10]
    assert_equal 200, map[20]
    assert_equal 300, map[30]
    assert_nil map[99]

    assert map.key?(10)
    refute map.key?(99)

    assert_equal [10, 100], map.first
    assert_equal [20, 200], map.next(10)

    pairs = []
    map.each { |k, v| pairs << [k, v] }
    assert_equal [[10, 100], [20, 200], [30, 300]], pairs

    assert_equal 100, map.delete(10)
    refute map.key?(10)
    assert_equal 2, map.size
  end

  def test_strmap
    strmap = Example::StrMap.new
    assert_equal 0, strmap.size

    strmap["alpha"] = 1
    strmap["beta"] = 2
    strmap["gamma"] = 3

    assert_equal 3, strmap.size
    assert_equal 1, strmap["alpha"]
    assert_equal 2, strmap["beta"]
    assert_nil strmap["delta"]

    assert strmap.key?("alpha")
    refute strmap.key?("delta")

    assert_equal 1, strmap.delete("alpha")
    refute strmap.key?("alpha")
    assert_equal 2, strmap.size
  end

  def test_bytesmap
    bytesmap = Example::BytesMap.new
    assert_equal 0, bytesmap.size

    k1 = "\x00\x01\xFE\xFF".b
    k2 = "\xFF\xFE\x01\x00".b

    bytesmap[k1] = 42
    bytesmap[k2] = 84

    assert_equal 2, bytesmap.size
    assert_equal 42, bytesmap[k1]
    assert_equal 84, bytesmap[k2]

    assert bytesmap.key?(k1)
    assert bytesmap.delete(k1)
    refute bytesmap.key?(k1)
  end

  def test_blobmap
    blobmap = Example::BlobMap.new
    assert_equal 0, blobmap.size

    blobmap.set(100, "hello world", hot_meta: 1234)
    blobmap.set(200, "foo bar baz", hot_meta: 5678)

    assert_equal 2, blobmap.size
    val, meta = blobmap.get(100)
    assert_equal "hello world", val
    assert_equal 1234, meta

    assert blobmap.key?(100)
    assert blobmap.delete(100)
    refute blobmap.key?(100)
    assert_equal 1, blobmap.size
  end
end
"#;
        let pack = RubyPack;
        let facts = pack
            .extract(
                "bindings/ruby/test/test_example.rb",
                src,
                &AssertVocabulary::default(),
            )
            .expect("extract succeeds");

        assert_eq!(facts.tests.len(), 6);
        assert_eq!(facts.tests[0].name, "TestExample#test_version");
        assert_eq!(facts.tests[1].name, "TestExample#test_set");
        assert_eq!(facts.tests[2].name, "TestExample#test_map");
        assert_eq!(facts.tests[3].name, "TestExample#test_strmap");
        assert_eq!(facts.tests[4].name, "TestExample#test_bytesmap");
        assert_eq!(facts.tests[5].name, "TestExample#test_blobmap");

        // Verify none of the real tests are vacuous or ignored
        for t in &facts.tests {
            assert!(!t.is_vacuous(), "test {} should not be vacuous", t.name);
            assert!(!t.ignored, "test {} should not be ignored", t.name);
            assert!(
                t.total_asserts > 0,
                "test {} should have assertions",
                t.name
            );
        }

        // test_version has refute_nil (weak) and assert_match (strong)
        assert_eq!(facts.tests[0].total_asserts, 2);
        assert_eq!(facts.tests[0].strong_asserts, 1);
    }

    #[test]
    fn test_ruby_hierarchical_skips_propagate_down() {
        let src = r#"
xdescribe "skipped outer block" do
  it "nested test 1" do
    expect(1).to eq(1)
  end

  context "nested context" do
    it "nested test 2" do
      expect(2).to eq(2)
    end
  end
end

describe "active block" do
  it "active test" do
    expect(3).to eq(3)
  end
end
"#;
        let facts = RubyPack
            .extract("spec/foo_spec.rb", src, &AssertVocabulary::default())
            .unwrap();

        assert_eq!(facts.tests.len(), 3);
        let t1 = facts
            .tests
            .iter()
            .find(|t| t.name.contains("nested test 1"))
            .unwrap();
        let t2 = facts
            .tests
            .iter()
            .find(|t| t.name.contains("nested test 2"))
            .unwrap();
        let t3 = facts
            .tests
            .iter()
            .find(|t| t.name.contains("active test"))
            .unwrap();

        assert!(t1.ignored, "t1 must inherit skip from xdescribe");
        assert!(t2.ignored, "t2 must inherit skip from nested xdescribe");
        assert!(!t3.ignored, "t3 must be active");
    }

    #[test]
    fn test_ruby_minitest_lifecycle_and_helper_resolution() {
        let src = r#"
class MinitestLifecycleTest < Minitest::Test
  def setup
    @val = 42
  end

  def teardown
    @val = nil
  end

  def check_value(expected)
    assert_equal expected, @val
  end

  def test_real_thing
    check_value(42)
  end
end
"#;
        let facts = RubyPack
            .extract("test/lifecycle_test.rb", src, &AssertVocabulary::default())
            .unwrap();

        assert_eq!(
            facts.tests.len(),
            1,
            "only test_real_thing must be extracted; setup, teardown, and check_value are not tests"
        );
        let t = &facts.tests[0];
        assert_eq!(t.name, "MinitestLifecycleTest#test_real_thing");
        assert_eq!(t.total_asserts, 1);
        assert_eq!(t.strong_asserts, 1);
        assert!(!t.is_vacuous());
    }
}
