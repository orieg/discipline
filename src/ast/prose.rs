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

#[cfg(all(
    test,
    feature = "lang-php",
    feature = "lang-ruby",
    feature = "lang-c",
    feature = "lang-cpp"
))]
mod pack_tests {
    use crate::ast::{default_registry, AssertVocabulary, Fact};

    fn spans(path: &str, src: &str) -> Vec<(usize, String)> {
        let reg = default_registry();
        let pack = reg.find_pack(path).unwrap();
        assert!(pack.supplies(Fact::Prose));
        pack.extract(path, src, &AssertVocabulary::default())
            .unwrap()
            .prose
            .into_iter()
            .map(|p| (p.line, p.text))
            .collect()
    }

    #[test]
    fn php_ruby_and_c_cpp_prose_is_comments_strings_and_heredocs() {
        let php = spans(
            "src/a.php",
            "<?php\n// one\n$a = 'two';\n$b = \"th$x\";\n$c = <<<EOT\nfour\nEOT;\n# five\n",
        );
        let texts: Vec<&str> = php.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(
            texts,
            vec!["// one", "'two'", "\"th$x\"", "<<<EOT\nfour\nEOT", "# five"]
        );
        let rb = spans("lib/a.rb", "# one\na = \"two\"\nb = <<~EOT\n  three\nEOT\n");
        let texts: Vec<&str> = rb.iter().map(|(_, t)| t.as_str()).collect();
        // A heredoc body spans from the line after the opener to its terminator.
        assert_eq!(texts, vec!["# one", "\"two\"", "\n  three\nEOT"]);
        let c = spans("src/a.c", "/* one */\nconst char *s = \"two\"; // three\n");
        let texts: Vec<&str> = c.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(texts, vec!["/* one */", "\"two\"", "// three"]);
        let cpp = spans("src/a.cpp", "const char *s = R\"(one)\" \"two\";\n");
        let texts: Vec<&str> = cpp.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(texts, vec!["R\"(one)\"", "\"two\""]);
    }
}
