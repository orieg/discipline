//! Relative Markdown links whose target is not in the head tree
//! (`citation-anchors/link-target-missing`).
//!
//! There is no Markdown grammar among the parsers, so the reader is line-based, after
//! CommonMark: fenced code blocks (`` ``` `` / `~~~`, closed by a fence of the same
//! character at least as long), indented code blocks (four columns after a blank line,
//! outside a list), code spans and HTML comments are not read. It reads inline links
//! `[text](target "title")`, images `![alt](target)` and reference definitions
//! `[label]: target`. A link or code span that wraps onto a second line is not read.

use std::collections::HashSet;

/// A link, image or reference-definition target, on a 1-based line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownLink {
    pub line_num: usize,
    pub target: String,
}

/// Whether `path` names a Markdown file (`.md`, `.markdown`, any case).
pub fn is_markdown_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty() && (ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
    })
}

/// The run of `ch` a fence line opens with (up to three spaces of indentation), and its
/// length, when it is at least three.
fn fence_run(line: &str) -> Option<(char, usize, &str)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let ch = rest.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let count = rest.chars().take_while(|c| *c == ch).count();
    (count >= 3).then(|| (ch, count, &rest[count..]))
}

/// Columns of leading whitespace, a tab counting as four.
fn indent_columns(line: &str) -> usize {
    line.chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

/// Whether a line (indentation removed) starts a list item.
fn is_list_item(trimmed: &str) -> bool {
    if let Some(rest) = trimmed.strip_prefix(['-', '*', '+']) {
        return rest.is_empty() || rest.starts_with([' ', '\t']);
    }
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    (1..=9).contains(&digits)
        && trimmed[digits..]
            .strip_prefix(['.', ')'])
            .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t']))
}

/// `line` with code spans and HTML comments replaced by spaces. `in_comment` carries an
/// HTML comment that is still open at the end of the line into the next one.
fn mask_line(line: &str, in_comment: &mut bool) -> Vec<char> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    let starts_with = |at: usize, pat: &str| {
        pat.chars()
            .enumerate()
            .all(|(k, p)| chars.get(at + k) == Some(&p))
    };
    while i < chars.len() {
        if *in_comment {
            if starts_with(i, "-->") {
                out.extend([' ', ' ', ' ']);
                i += 3;
                *in_comment = false;
            } else {
                out.push(' ');
                i += 1;
            }
            continue;
        }
        match chars[i] {
            '\\' if i + 1 < chars.len() => {
                out.extend([chars[i], chars[i + 1]]);
                i += 2;
            }
            '<' if starts_with(i, "<!--") => {
                out.extend([' ', ' ', ' ', ' ']);
                i += 4;
                *in_comment = true;
            }
            '`' => {
                let run = chars[i..].iter().take_while(|c| **c == '`').count();
                // The closing run is a run of exactly the same length.
                let mut j = i + run;
                let mut close = None;
                while j < chars.len() {
                    if chars[j] == '`' {
                        let len = chars[j..].iter().take_while(|c| **c == '`').count();
                        if len == run {
                            close = Some(j + len);
                            break;
                        }
                        j += len;
                    } else {
                        j += 1;
                    }
                }
                match close {
                    Some(end) => {
                        out.extend(std::iter::repeat_n(' ', end - i));
                        i = end;
                    }
                    None => {
                        out.extend(std::iter::repeat_n('`', run));
                        i += run;
                    }
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// The index of the `]` closing the `[` at `open`, brackets nested and escapes skipped.
fn closing_bracket(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 1,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The destination starting at `start` (after the `(`): the target with backslash
/// escapes removed, and the index of the `)` that closes the link. `None` when the text
/// is not a link destination (CommonMark reads it as plain text then).
fn inline_destination(chars: &[char], start: usize) -> Option<(String, usize)> {
    let skip_ws = |mut k: usize| {
        while chars.get(k).is_some_and(|c| *c == ' ' || *c == '\t') {
            k += 1;
        }
        k
    };
    let mut k = skip_ws(start);
    let mut target = String::new();
    if chars.get(k) == Some(&'<') {
        k += 1;
        loop {
            match chars.get(k)? {
                '>' => break,
                '<' => return None,
                '\\' if chars.get(k + 1).is_some_and(char::is_ascii_punctuation) => {
                    target.push(chars[k + 1]);
                    k += 2;
                    continue;
                }
                c => target.push(*c),
            }
            k += 1;
        }
        k += 1;
    } else {
        let mut depth = 0usize;
        while let Some(&c) = chars.get(k) {
            match c {
                '\\' if chars.get(k + 1).is_some_and(char::is_ascii_punctuation) => {
                    target.push(chars[k + 1]);
                    k += 2;
                    continue;
                }
                '(' => depth += 1,
                ')' if depth == 0 => break,
                ')' => depth -= 1,
                c if c.is_whitespace() || c.is_control() => break,
                _ => {}
            }
            target.push(c);
            k += 1;
        }
    }
    k = skip_ws(k);
    if let Some(&open) = chars.get(k).filter(|c| matches!(c, '"' | '\'' | '(')) {
        let close = if open == '(' { ')' } else { open };
        k += 1;
        loop {
            match chars.get(k)? {
                '\\' => k += 1,
                c if *c == close => break,
                _ => {}
            }
            k += 1;
        }
        k = skip_ws(k + 1);
    }
    (chars.get(k) == Some(&')')).then_some((target, k))
}

/// The target of a reference definition `[label]: target` (up to three spaces of
/// indentation; a footnote `[^1]: text` is not one).
fn reference_definition(chars: &[char]) -> Option<String> {
    let indent = chars.iter().take_while(|c| **c == ' ').count();
    if indent > 3 || chars.get(indent) != Some(&'[') || chars.get(indent + 1) == Some(&'^') {
        return None;
    }
    let close = closing_bracket(chars, indent)?;
    if close == indent + 1 || chars.get(close + 1) != Some(&':') {
        return None;
    }
    let mut k = close + 2;
    while chars.get(k).is_some_and(|c| *c == ' ' || *c == '\t') {
        k += 1;
    }
    let rest: String = chars[k..].iter().collect();
    let target = match rest.strip_prefix('<') {
        Some(inner) => inner.split_once('>')?.0.to_string(),
        None => rest.split_whitespace().next()?.to_string(),
    };
    Some(target)
}

/// The inline link and image targets of one masked line.
fn inline_targets(chars: &[char]) -> Vec<String> {
    let mut found = Vec::new();
    // Destinations already read: a `[` inside one opens nothing.
    let mut read: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 1,
            '[' if !read.iter().any(|(s, e)| (*s..=*e).contains(&i)) => {
                if let Some(close) = closing_bracket(chars, i) {
                    if chars.get(close + 1) == Some(&'(') {
                        if let Some((target, end)) = inline_destination(chars, close + 2) {
                            found.push(target);
                            read.push((close + 1, end));
                        }
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    found
}

/// Every link, image and reference-definition target of a Markdown text, outside code
/// and HTML comments, in order.
pub fn extract_markdown_links(content: &str) -> Vec<MarkdownLink> {
    let mut links = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    let mut in_comment = false;
    let mut prev_blank = true;
    let mut in_list = false;
    let mut in_indented_code = false;
    for (idx, line) in content.lines().enumerate() {
        if let Some((open_ch, open_len)) = fence {
            if let Some((ch, len, rest)) = fence_run(line) {
                if ch == open_ch && len >= open_len && rest.trim().is_empty() {
                    fence = None;
                }
            }
            continue;
        }
        let blank = line.trim().is_empty();
        if !in_comment {
            if let Some((ch, len, info)) = fence_run(line) {
                // A backtick fence's info string holds no backtick.
                if ch == '~' || !info.contains('`') {
                    fence = Some((ch, len));
                    prev_blank = false;
                    continue;
                }
            }
            if !blank && indent_columns(line) >= 4 && (in_indented_code || (prev_blank && !in_list))
            {
                in_indented_code = true;
                prev_blank = false;
                continue;
            }
        }
        if blank {
            prev_blank = true;
            continue;
        }
        in_indented_code = false;
        let trimmed = line.trim_start();
        if is_list_item(trimmed) {
            in_list = true;
        } else if prev_blank && indent_columns(line) == 0 {
            in_list = false;
        }
        prev_blank = false;

        let masked = mask_line(line, &mut in_comment);
        let line_num = idx + 1;
        if let Some(target) = reference_definition(&masked) {
            links.push(MarkdownLink { line_num, target });
            continue;
        }
        for target in inline_targets(&masked) {
            links.push(MarkdownLink { line_num, target });
        }
    }
    links
}

/// Where a link target leads, for [`resolve_link_target`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkResolution {
    /// Not a relative path: a URL (any scheme), an absolute or protocol-relative path,
    /// a same-page anchor, a template expression, or nothing.
    NotChecked,
    /// A repository path (empty for the root), and whether the target named a directory
    /// (a trailing `/`).
    Path { path: String, directory: bool },
    /// More `..` than the document's directory is deep.
    OutsideRepository,
}

/// Whether `target` starts with a URI scheme (`[a-z][a-z0-9+.-]*:`).
fn has_scheme(target: &str) -> bool {
    let mut chars = target.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    for c in chars {
        if c == ':' {
            return true;
        }
        if !(c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-')) {
            return false;
        }
    }
    false
}

/// `%XX` sequences decoded; the text as written when the result is not UTF-8.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

/// Resolves a link `target` written in the Markdown file `doc_path` against the
/// document's directory: the query and fragment dropped, percent-encoding decoded, `.`
/// and `..` segments applied.
pub fn resolve_link_target(doc_path: &str, target: &str) -> LinkResolution {
    let target = target.trim();
    if target.is_empty()
        || target.starts_with(['#', '/', '\\'])
        || ["{{", "{%", "<%", "${"].iter().any(|t| target.contains(t))
        || has_scheme(target)
    {
        return LinkResolution::NotChecked;
    }
    let path_part = target.split(['?', '#']).next().unwrap_or("");
    if path_part.is_empty() {
        return LinkResolution::NotChecked;
    }
    let decoded = percent_decode(path_part);
    let directory = decoded.ends_with('/');
    let dir = doc_path.rsplit_once('/').map_or("", |(d, _)| d);
    let mut segments: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
    for seg in decoded.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if segments.pop().is_none() {
                    return LinkResolution::OutsideRepository;
                }
            }
            s => segments.push(s),
        }
    }
    LinkResolution::Path {
        path: segments.join("/"),
        directory,
    }
}

/// Tracked files on the head side and the directories that hold them.
#[derive(Debug, Default)]
pub struct HeadTree {
    files: HashSet<String>,
    dirs: HashSet<String>,
}

impl HeadTree {
    pub fn new(files: impl IntoIterator<Item = String>) -> Self {
        let mut tree = HeadTree::default();
        for file in files {
            let mut at = file.as_str();
            while let Some((parent, _)) = at.rsplit_once('/') {
                if !tree.dirs.insert(parent.to_string()) {
                    break;
                }
                at = parent;
            }
            tree.files.insert(file);
        }
        tree
    }

    /// Whether `path` is a tracked file (unless a directory was asked for) or a
    /// directory holding one. Compared byte for byte: case matters.
    pub fn holds(&self, path: &str, directory: bool) -> bool {
        path.is_empty() || self.dirs.contains(path) || (!directory && self.files.contains(path))
    }
}

/// `target` cut to at most 120 characters for a message.
pub fn shown_target(target: &str) -> String {
    const MAX: usize = 120;
    if target.chars().count() <= MAX {
        target.to_string()
    } else {
        let cut: String = target.chars().take(MAX - 3).collect();
        format!("{cut}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(content: &str) -> Vec<(usize, String)> {
        extract_markdown_links(content)
            .into_iter()
            .map(|l| (l.line_num, l.target))
            .collect()
    }

    fn one(line: usize, t: &str) -> Vec<(usize, String)> {
        vec![(line, t.to_string())]
    }

    #[test]
    fn each_link_form_is_read() {
        assert_eq!(
            targets("See [the guide](docs/guide.md).\n"),
            one(1, "docs/guide.md")
        );
        assert_eq!(
            targets("See [the guide](docs/guide.md \"The guide\").\n"),
            one(1, "docs/guide.md")
        );
        assert_eq!(targets("![logo](img/logo.png)\n"), one(1, "img/logo.png"));
        assert_eq!(targets("[api]: docs/api.md\n"), one(1, "docs/api.md"));
        assert_eq!(
            targets("[api]: <docs/my api.md> \"API\"\n"),
            one(1, "docs/my api.md")
        );
        assert_eq!(targets("[x](<docs/a b.md>)\n"), one(1, "docs/a b.md"));
        // A badge: the image inside the link and the link itself.
        assert_eq!(
            targets("[![ci](badge.svg)](ci.md)\n"),
            vec![(1, "ci.md".to_string()), (1, "badge.svg".to_string())]
        );
        // Balanced parentheses belong to the destination.
        assert_eq!(targets("[w](docs/a_(b).md)\n"), one(1, "docs/a_(b).md"));
        // Negative controls: prose brackets, a reference link, a footnote definition.
        assert!(targets("an [aside] (not a link)\n").is_empty());
        assert!(targets("see [the guide][g]\n").is_empty());
        assert!(targets("[^1]: a footnote, docs/x.md\n").is_empty());
        assert!(targets("[]: empty-label.md\n").is_empty());
        assert!(targets("\\[not](a-link.md)\n").is_empty());
    }

    #[test]
    fn code_and_comments_are_not_read() {
        let fenced = "```md\n[a](missing.md)\n```\n[b](b.md)\n";
        assert_eq!(targets(fenced), one(4, "b.md"));
        let tilde = "~~~\n[a](missing.md)\n```\n[c](c.md)\n~~~\n[b](b.md)\n";
        assert_eq!(targets(tilde), one(6, "b.md"));
        // A shorter fence quoted inside a longer one stays code.
        let long = "````\n```\n[a](missing.md)\n```\n````\n[b](b.md)\n";
        assert_eq!(targets(long), one(6, "b.md"));
        assert!(targets("Run `[a](missing.md)` to see.\n").is_empty());
        assert!(targets("Run ``[a](`x`.md)`` to see.\n").is_empty());
        assert!(targets("<!-- [a](missing.md) -->\n").is_empty());
        assert_eq!(
            targets("<!--\n[a](missing.md)\n--> [b](b.md)\n"),
            one(3, "b.md")
        );
        assert!(targets("text\n\n    [a](missing.md)\n").is_empty());
        // Positive controls: indentation inside a list item is prose, and an unclosed
        // backtick is a backtick.
        assert_eq!(targets("- item\n\n    [a](a.md)\n"), one(3, "a.md"));
        assert_eq!(targets("a ` b [a](a.md)\n"), one(1, "a.md"));
    }

    fn path(p: &str) -> LinkResolution {
        LinkResolution::Path {
            path: p.to_string(),
            directory: false,
        }
    }

    #[test]
    fn targets_are_resolved_against_the_document_directory() {
        assert_eq!(
            resolve_link_target("README.md", "docs/guide.md"),
            path("docs/guide.md")
        );
        assert_eq!(
            resolve_link_target("docs/a/b.md", "../c.md"),
            path("docs/c.md")
        );
        assert_eq!(
            resolve_link_target("docs/a/b.md", "./c.md"),
            path("docs/a/c.md")
        );
        assert_eq!(
            resolve_link_target("docs/a/b.md", "../../src/./lib.rs"),
            path("src/lib.rs")
        );
        assert_eq!(
            resolve_link_target("docs/b.md", "../docs/"),
            LinkResolution::Path {
                path: "docs".to_string(),
                directory: true
            }
        );
        assert_eq!(
            resolve_link_target("docs/b.md", "../"),
            LinkResolution::Path {
                path: String::new(),
                directory: true
            }
        );
        // Fragment and query dropped, percent-encoding decoded.
        assert_eq!(resolve_link_target("README.md", "a.md#usage"), path("a.md"));
        assert_eq!(
            resolve_link_target("README.md", "a.md?plain=1#L3"),
            path("a.md")
        );
        assert_eq!(
            resolve_link_target("README.md", "docs/my%20guide.md"),
            path("docs/my guide.md")
        );
        assert_eq!(resolve_link_target("README.md", "a%2.md"), path("a%2.md"));
        assert_eq!(resolve_link_target("README.md", "a%"), path("a%"));
        // More `..` than the document is deep.
        assert_eq!(
            resolve_link_target("docs/b.md", "../../x.md"),
            LinkResolution::OutsideRepository
        );
        assert_eq!(
            resolve_link_target("README.md", "../sibling/README.md"),
            LinkResolution::OutsideRepository
        );
    }

    #[test]
    fn urls_absolute_paths_anchors_and_templates_are_not_checked() {
        for t in [
            "https://example.com/x.md",
            "http://example.com",
            "mailto:a@example.com",
            "ftp://example.com/f",
            "vscode://file/x",
            "git+ssh://host/repo",
            "/docs/guide.md",
            "//example.com/x",
            "#usage",
            "?tab=readme",
            "{{ site.baseurl }}/x.md",
            "{% link x.md %}",
            "<%= path %>",
            "${BASE}/x.md",
            "",
            "   ",
        ] {
            assert_eq!(
                resolve_link_target("README.md", t),
                LinkResolution::NotChecked,
                "`{t}`"
            );
        }
        // Control: a relative path that only looks like one of them is checked.
        assert_eq!(
            resolve_link_target("README.md", "docs/http.md"),
            path("docs/http.md")
        );
        assert_eq!(resolve_link_target("README.md", "1http:x"), path("1http:x"));
    }

    #[test]
    fn the_head_tree_holds_files_and_their_directories_case_sensitively() {
        let tree = HeadTree::new(["docs/guide.md".to_string(), "README.md".to_string()]);
        assert!(tree.holds("docs/guide.md", false));
        assert!(tree.holds("docs", false));
        assert!(tree.holds("docs", true));
        assert!(tree.holds("", true));
        assert!(!tree.holds("docs/Guide.md", false));
        assert!(!tree.holds("Docs", true));
        assert!(!tree.holds("docs/guide.md", true));
        assert!(!tree.holds("docs/api-reference.md", false));
    }

    #[test]
    fn markdown_paths_and_shown_targets() {
        for p in ["README.md", "docs/a.markdown", "x/NOTES.MD"] {
            assert!(is_markdown_path(p), "{p}");
        }
        for p in ["notes.txt", "md", ".md", "docs/md/readme", "a.mdx"] {
            assert!(!is_markdown_path(p), "{p}");
        }
        assert_eq!(shown_target("a.md"), "a.md");
        let long = "x".repeat(200);
        assert_eq!(shown_target(&long).chars().count(), 120);
        assert!(shown_target(&long).ends_with("..."));
    }
}
