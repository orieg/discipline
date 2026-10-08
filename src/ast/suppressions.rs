//! The suppressions of a source file: the comments and annotations that turn a linter,
//! a type checker or a coverage tool off. Each pack names its spellings in one
//! [`Suppressions`] table and [`Suppressions::collect`] reads every pack's file with the
//! same walk.
//!
//! The tables record what each pack read before it had a table, so two packs that
//! differ in how they take a rule out of a comment or an annotation still differ here,
//! by a field of the table.

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
    };
}

/// One kind of suppressing comment.
pub(crate) struct CommentRule {
    /// What the comment's body opens with, or holds when `anywhere` is set.
    pub markers: &'static [&'static str],
    /// Whether a marker counts anywhere in the body and not only where it opens.
    pub anywhere: bool,
    pub reports: Reports,
}

impl CommentRule {
    /// A comment whose body opens with one of `markers`.
    pub const fn opens(markers: &'static [&'static str], reports: Reports) -> Self {
        Self {
            markers,
            anywhere: false,
            reports,
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
    fn read(&self, body: &str) -> String {
        let mut rest = body;
        if !self.after.is_empty() {
            match self.after.iter().find_map(|p| rest.strip_prefix(p)) {
                Some(stripped) => rest = stripped,
                None => return "all".to_string(),
            }
        }
        for prefix in self.then {
            rest = rest.trim_start_matches(prefix);
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
    /// The node's `name` field, whole: a qualified name is not one of the names.
    Field,
    /// The first `identifier` under the node, in source order.
    FirstIdentifier,
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
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "identifier" {
            return n.utf8_text(src).unwrap_or("");
        }
        let mut cursor = n.walk();
        let children: Vec<Node> = n.named_children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    ""
}

impl AnnotationSuppressions {
    fn site(&self, node: Node, src: &[u8]) -> Option<EscapeHatchSite> {
        let text_of = |n: Node| n.utf8_text(src).unwrap_or("");
        let name = match self.name {
            AnnotationName::Field => node.child_by_field_name("name").map_or("", text_of),
            AnnotationName::FirstIdentifier => first_identifier(node, src),
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
    fn comment_site(&self, node: Node, src: &[u8]) -> Option<EscapeHatchSite> {
        let text = node.utf8_text(src).unwrap_or("");
        let mut body = text;
        for marker in self.open {
            body = body.trim_start_matches(marker);
        }
        for marker in self.close {
            body = body.trim_end_matches(marker);
        }
        let body = body.trim();
        let rule = self.rules.iter().find(|rule| {
            rule.markers.iter().any(|marker| {
                if rule.anywhere {
                    body.contains(marker)
                } else {
                    body.starts_with(marker)
                }
            })
        })?;
        let line = node.start_position().row + 1;
        let snippet = if self.trimmed_snippet {
            text.trim()
        } else {
            text
        }
        .to_string();
        let rule = match &rule.reports {
            Reports::TypeIgnore(tool) => {
                return Some(EscapeHatchSite::TypeIgnore {
                    line,
                    tool: tool.to_string(),
                    snippet,
                })
            }
            Reports::Rule(rule) => rule.to_string(),
            Reports::Body => body.to_string(),
            Reports::Rest(text) => text.read(body),
        };
        Some(EscapeHatchSite::LinterDisable {
            line,
            rule,
            snippet,
        })
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
                sites.extend(self.comment_site(node, src));
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
