//! The suppressions of a source file: the comments and annotations that turn a linter,
//! a type checker or a coverage tool off. Each pack names its spellings in one
//! [`Suppressions`] table and [`Suppressions::collect`] reads every pack's file with the
//! same walk.
//!
//! Two packs that differ in how they take a rule out of a comment or an annotation
//! differ here by a field of the table. A spelling is in a table when its tool honours
//! it; the table says where that rule of the tool was read.

use super::EscapeHatchSite;
use tree_sitter::Node;

/// How a pack's source suppresses a tool.
pub(crate) struct Suppressions {
    /// The node kinds that are comments. A comment's children are not walked.
    pub comments: &'static [&'static str],
    /// What a comment opens with: each is taken off the text as often as it repeats, in
    /// this order, to leave the comment's body.
    pub open: &'static [&'static str],
    /// What a comment closes with, taken off the same way before the body is trimmed.
    pub close: &'static [&'static str],
    /// The suppressing comments. The first rule a body matches is the one reported.
    pub rules: &'static [CommentRule],
    /// Whether the snippet of a comment is its text trimmed, and not its text as written.
    pub trimmed_snippet: bool,
    /// The suppressing annotations. An annotation's children are not walked, so one
    /// inside the arguments of another is not read.
    pub annotations: Option<&'static AnnotationSuppressions>,
    /// What the pack reads from a node that is neither.
    pub other: Option<OtherNode>,
    /// What opens a piece of a comment (each `#` in `# note # noqa`), for the rules that
    /// are `later`; empty when the pack reads no pieces. A piece runs from one of these
    /// to the end of the comment.
    pub piece: &'static str,
}

/// A pack's own reading of a node that is neither a comment nor an annotation: it
/// pushes the node's sites and says whether the node's children are walked.
pub(crate) type OtherNode = fn(Node, &[u8], &mut Vec<EscapeHatchSite>) -> bool;

impl Suppressions {
    /// `//` and `/* */` comments in one `comment` node kind, and nothing read yet.
    pub const SLASH_COMMENTS: Self = Self {
        comments: &["comment"],
        open: &["//", "/*"],
        close: &["*/"],
        rules: &[],
        trimmed_snippet: false,
        annotations: None,
        other: None,
        piece: "",
    };
}

/// One kind of suppressing comment.
pub(crate) struct CommentRule {
    /// What the comment's body opens with, or holds when `anywhere` is set.
    pub markers: &'static [&'static str],
    /// Whether a marker counts anywhere in the body and not only where it opens.
    pub anywhere: bool,
    /// Whether the ASCII letters of a marker match in either case.
    pub any_case: bool,
    /// Whether a space in a marker stands for any run of spaces and tabs, none included.
    pub loose_spaces: bool,
    /// Whether the rule is also read from the pieces of the comment (see
    /// [`Suppressions::piece`]) when the comment is not reported under this rule: the
    /// first piece that matches is one more site, whose snippet is the piece.
    pub later: bool,
    /// The comment node kinds the rule is read from; every kind when empty.
    pub kinds: &'static [&'static str],
    pub reports: Reports,
}

impl CommentRule {
    /// A comment whose body opens with one of `markers`.
    pub const fn opens(markers: &'static [&'static str], reports: Reports) -> Self {
        Self {
            markers,
            anywhere: false,
            any_case: false,
            loose_spaces: false,
            later: false,
            kinds: &[],
            reports,
        }
    }

    /// What is left of `body` after one of the markers, when one opens it.
    fn opened<'t>(&self, body: &'t str) -> Option<&'t str> {
        self.markers
            .iter()
            .find_map(|marker| self.after(body, marker))
    }

    /// What is left of `text` after `marker`, when `marker` opens it under this rule's
    /// letter-case and spacing.
    fn after<'t>(&self, text: &'t str, marker: &str) -> Option<&'t str> {
        let bytes = text.as_bytes();
        let mut at = 0;
        for expected in marker.bytes() {
            if self.loose_spaces && expected == b' ' {
                while matches!(bytes.get(at), Some(b' ' | b'\t')) {
                    at += 1;
                }
                continue;
            }
            let found = *bytes.get(at)?;
            let same = if self.any_case {
                found.eq_ignore_ascii_case(&expected)
            } else {
                found == expected
            };
            if !same {
                return None;
            }
            at += 1;
        }
        text.get(at..)
    }

    fn matches(&self, kind: &str, body: &str) -> bool {
        if !self.kinds.is_empty() && !self.kinds.contains(&kind) {
            return false;
        }
        if self.anywhere {
            self.markers.iter().any(|marker| body.contains(marker))
        } else {
            self.opened(body).is_some()
        }
    }
}

/// What a suppressing comment is reported as.
pub(crate) enum Reports {
    /// A type-checker suppression of this tool.
    TypeIgnore(&'static str),
    /// A linter suppression of this rule.
    Rule(&'static str),
    /// A linter suppression whose rule is the comment's whole body.
    Body,
    /// A linter suppression whose rule is read from the body.
    Rest(RuleText),
}

/// How a rule is read from a comment's body.
pub(crate) struct RuleText {
    /// The first of these the body opens with is taken off once. When the list is not
    /// empty and the body opens with none of them, the rule is `all`.
    pub after: &'static [&'static str],
    /// Then each of these is taken off as often as it repeats, in this order.
    pub then: &'static [&'static str],
    /// Whether the rule is the first word of what is left (`all` when there is none),
    /// and not all of it, trimmed.
    pub first_word: bool,
    /// Whether nothing left reads as `all`, and not as an empty rule.
    pub empty_is_all: bool,
}

impl RuleText {
    /// The rule of `body`, with each marker matched as `rule` matches its own.
    fn read(&self, rule: &CommentRule, body: &str) -> String {
        let mut rest = body;
        if !self.after.is_empty() {
            match self.after.iter().find_map(|p| rule.after(rest, p)) {
                Some(stripped) => rest = stripped,
                None => return "all".to_string(),
            }
        }
        for prefix in self.then {
            while let Some(stripped) = rule.after(rest, prefix).filter(|s| s.len() < rest.len()) {
                rest = stripped;
            }
        }
        let rule = if self.first_word {
            rest.split_whitespace().next().unwrap_or("all")
        } else {
            rest.trim()
        };
        if rule.is_empty() && self.empty_is_all {
            return "all".to_string();
        }
        rule.to_string()
    }
}

/// The annotations that suppress a linter, and how the rule is read from one.
pub(crate) struct AnnotationSuppressions {
    /// The annotation node kinds.
    pub kinds: &'static [&'static str],
    pub name: AnnotationName,
    /// The annotation names that suppress.
    pub names: &'static [&'static str],
    pub rule: AnnotationRule,
}

/// Where an annotation's name is read from.
pub(crate) enum AnnotationName {
    /// The last segment of the node's `name` field: `SuppressWarnings` in
    /// `@java.lang.SuppressWarnings`. The package is not read, so an annotation of
    /// another package with one of the names is read as a suppression.
    LastSegment,
    /// The last `identifier` of the first `user_type` under the node, in source order:
    /// `Suppress` in `@kotlin.Suppress` and in `@file:Suppress`. The package is not
    /// read here either.
    UserTypeLast,
}

/// How the rule is read from a suppressing annotation. In each, an annotation with no
/// arguments suppresses `all`.
pub(crate) enum AnnotationRule {
    /// From the `arguments` field: the parentheses are taken off both ends, then one
    /// layer of surrounding quotes. `trimmed` trims between the two steps;
    /// `empty_is_all` reads an empty result as `all`.
    Arguments { trimmed: bool, empty_is_all: bool },
    /// From the annotation's text after its first `(`: the closing parentheses are
    /// taken off the end, the rest is trimmed, then one layer of surrounding quotes.
    AfterParen,
}

/// The first `identifier` under `node`, in source order; empty when there is none.
pub(crate) fn first_identifier<'s>(node: Node, src: &'s [u8]) -> &'s str {
    first_of_kind(node, "identifier").map_or("", |n| n.utf8_text(src).unwrap_or(""))
}

/// The first node of `kind` at or under `node`, in source order.
fn first_of_kind<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == kind {
            return Some(n);
        }
        let mut cursor = n.walk();
        let children: Vec<Node> = n.named_children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    None
}

/// The last segment of a name: the node itself, or the last named child of a qualified
/// name, followed down to a node with none.
fn last_segment(name: Node) -> Node {
    let mut node = name;
    while let Some(child) = node
        .named_child_count()
        .checked_sub(1)
        .and_then(|last| node.named_child(last))
    {
        node = child;
    }
    node
}

impl AnnotationSuppressions {
    fn site(&self, node: Node, src: &[u8]) -> Option<EscapeHatchSite> {
        let text_of = |n: Node| n.utf8_text(src).unwrap_or("");
        let name = match self.name {
            AnnotationName::LastSegment => node
                .child_by_field_name("name")
                .map(last_segment)
                .map_or("", text_of),
            AnnotationName::UserTypeLast => first_of_kind(node, "user_type")
                .and_then(|user_type| {
                    let mut cursor = user_type.walk();
                    let last = user_type
                        .named_children(&mut cursor)
                        .filter(|child| child.kind() == "identifier")
                        .last();
                    last
                })
                .map_or("", text_of),
        };
        if !self.names.contains(&name) {
            return None;
        }
        let text = text_of(node);
        let rule = match self.rule {
            AnnotationRule::Arguments {
                trimmed,
                empty_is_all,
            } => node
                .child_by_field_name("arguments")
                .map(|arguments| {
                    let mut inner = text_of(arguments).trim_matches(['(', ')']);
                    if trimmed {
                        inner = inner.trim();
                    }
                    inner.trim_matches('"')
                })
                .filter(|rule| !(empty_is_all && rule.is_empty())),
            AnnotationRule::AfterParen => text
                .split_once('(')
                .map(|(_, rest)| rest.trim_end_matches(')').trim().trim_matches('"')),
        };
        Some(EscapeHatchSite::LinterDisable {
            line: node.start_position().row + 1,
            rule: rule.unwrap_or("all").to_string(),
            snippet: text.to_string(),
        })
    }
}

impl Suppressions {
    /// The comment's text without what opens and closes a comment, trimmed.
    fn body<'t>(&self, text: &'t str) -> &'t str {
        let mut body = text;
        for marker in self.open {
            body = body.trim_start_matches(marker);
        }
        for marker in self.close {
            body = body.trim_end_matches(marker);
        }
        body.trim()
    }

    /// The site `rule` reports for a comment or a piece of one.
    fn site(&self, rule: &CommentRule, line: usize, text: &str, body: &str) -> EscapeHatchSite {
        let snippet = if self.trimmed_snippet {
            text.trim()
        } else {
            text
        }
        .to_string();
        let rule = match &rule.reports {
            Reports::TypeIgnore(tool) => {
                return EscapeHatchSite::TypeIgnore {
                    line,
                    tool: tool.to_string(),
                    snippet,
                }
            }
            Reports::Rule(rule) => rule.to_string(),
            Reports::Body => body.to_string(),
            Reports::Rest(text) => text.read(rule, body),
        };
        EscapeHatchSite::LinterDisable {
            line,
            rule,
            snippet,
        }
    }

    /// The sites of one comment: the first rule its body matches, then each other
    /// `later` rule at the first piece that matches it.
    fn comment_sites(&self, node: Node, src: &[u8], sites: &mut Vec<EscapeHatchSite>) {
        let text = node.utf8_text(src).unwrap_or("");
        let kind = node.kind();
        let line = node.start_position().row + 1;
        let body = self.body(text);
        let first = self.rules.iter().position(|rule| rule.matches(kind, body));
        if let Some(rule) = first.and_then(|at| self.rules.get(at)) {
            sites.push(self.site(rule, line, text, body));
        }
        if self.piece.is_empty() {
            return;
        }
        for (at, rule) in self.rules.iter().enumerate() {
            if !rule.later || first == Some(at) {
                continue;
            }
            let piece = text
                .match_indices(self.piece)
                .filter_map(|(offset, _)| text.get(offset..))
                .find(|piece| rule.matches(kind, self.body(piece)));
            if let Some(piece) = piece {
                sites.push(self.site(rule, line, piece, self.body(piece)));
            }
        }
    }
}

impl Suppressions {
    /// The suppressions under `root`, in source order, pushed to `sites`: what the table
    /// reads from each comment, from each annotation, and from any other node through
    /// `other`.
    pub(crate) fn collect(&self, root: Node, src: &[u8], sites: &mut Vec<EscapeHatchSite>) {
        super::bounds::walk(root, &mut |node| {
            let kind = node.kind();
            if self.comments.contains(&kind) {
                self.comment_sites(node, src, sites);
                return false;
            }
            if let Some(annotations) = self.annotations.filter(|a| a.kinds.contains(&kind)) {
                sites.extend(annotations.site(node, src));
                return false;
            }
            self.other.is_none_or(|other| other(node, src, sites))
        });
    }
}
