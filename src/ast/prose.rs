//! Comment, docstring and string-literal spans of a source file: the places a
//! change can carry text meant for a reader other than the compiler.
//!
//! The `instruction-smuggling` gate scans these (and whole lines of prose files) for
//! text aimed at an agent. One walker; packs pass their node kinds.

use tree_sitter::Node;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProseSpan {
    pub line: usize,
    pub end_line: usize,
    pub text: String,
}

/// Comment and string node kinds, collected without descending into a match.
pub fn extract(root: Node, src: &str, kinds: &[&str]) -> Vec<ProseSpan> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if kinds.contains(&node.kind()) {
            out.push(ProseSpan {
                line: node.start_position().row + 1,
                end_line: node.end_position().row + 1,
                text: node.utf8_text(src.as_bytes()).unwrap_or("").to_string(),
            });
            continue;
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
    out
}
