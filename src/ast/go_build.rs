//! The build constraint at the head of a Go file, and whether a default `go test`
//! builds the file.
//!
//! `go help buildconstraint`: a constraint is a `//go:build` line among the comments
//! and blank lines before the package clause, or, with no such line, the legacy
//! `// +build` lines of the leading `//` comments that a blank line follows. The
//! expression combines tags with `||`, `&&`, `!` and parentheses. A default build
//! satisfies the tags of its platform (operating system, architecture, `unix`, `cgo`,
//! the compiler, the `go1.N` release tags) and no other: any other tag is set only by
//! `-tags`. `ignore` is the conventional tag no build sets.

/// Whether a default `go test` builds a file, as far as its build constraint says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoBuild {
    /// No constraint, or one a default build satisfies on some platform.
    Built,
    /// The constraint holds under no assignment of tags, or needs the `ignore` tag.
    Never,
    /// The constraint holds only with these tags, which `-tags` alone sets.
    NeedsTags(Vec<String>),
    /// The constraint cannot be read: the go tool rejects it, or it is larger than
    /// this evaluation goes.
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Expr {
    Tag(String),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

const KNOWN_OS: &[&str] = &[
    "aix",
    "android",
    "darwin",
    "dragonfly",
    "freebsd",
    "hurd",
    "illumos",
    "ios",
    "js",
    "linux",
    "nacl",
    "netbsd",
    "openbsd",
    "plan9",
    "solaris",
    "wasip1",
    "windows",
    "zos",
];

const KNOWN_ARCH: &[&str] = &[
    "386",
    "amd64",
    "amd64p32",
    "arm",
    "armbe",
    "arm64",
    "arm64be",
    "loong64",
    "mips",
    "mipsle",
    "mips64",
    "mips64le",
    "mips64p32",
    "mips64p32le",
    "ppc",
    "ppc64",
    "ppc64le",
    "riscv",
    "riscv64",
    "s390",
    "s390x",
    "sparc",
    "sparc64",
    "wasm",
];

/// More distinct tags than this in one constraint and it is not evaluated.
const MAX_TAGS: usize = 12;

/// Whether a default build may satisfy `tag` on some platform or toolchain.
fn is_platform_tag(tag: &str) -> bool {
    if KNOWN_OS.contains(&tag)
        || KNOWN_ARCH.contains(&tag)
        || matches!(tag, "unix" | "cgo" | "gc" | "gccgo")
    {
        return true;
    }
    // A release tag: `go1.21`.
    if let Some(minor) = tag.strip_prefix("go1.") {
        return !minor.is_empty() && minor.bytes().all(|b| b.is_ascii_digit());
    }
    // An architecture feature: `amd64.v3`, `arm.7`.
    tag.split_once('.')
        .is_some_and(|(arch, feature)| KNOWN_ARCH.contains(&arch) && !feature.is_empty())
}

/// The 1-based line of the constraint [`go_build_constraint`] reads: the `//go:build`
/// line, or with none the first legacy `// +build` line that counts. `None` when the
/// file has no constraint, or its header cannot be read.
pub fn go_build_constraint_line(source: &str) -> Option<usize> {
    let header = read_header(source).ok()?;
    header
        .go_build
        .map(|(index, _)| index + 1)
        .or_else(|| header.plus_build.first().map(|(index, _)| index + 1))
}

/// Reads the build constraint of a Go source file.
pub fn go_build_constraint(source: &str) -> GoBuild {
    let header = match read_header(source) {
        Ok(header) => header,
        Err(()) => return GoBuild::Unreadable,
    };
    let expr = match header.go_build {
        Some((_, line)) => match parse_go_build(line) {
            Some(expr) => Some(expr),
            None => return GoBuild::Unreadable,
        },
        None => {
            let mut combined: Option<Expr> = None;
            for (_, line) in header.plus_build {
                let Some(expr) = parse_plus_build(line) else {
                    return GoBuild::Unreadable;
                };
                combined = Some(match combined {
                    Some(prev) => Expr::And(Box::new(prev), Box::new(expr)),
                    None => expr,
                });
            }
            combined
        }
    };
    match expr {
        Some(expr) => evaluate(&expr),
        None => GoBuild::Built,
    }
}

struct Header<'a> {
    /// What follows `//go:build` on the one such line, with the line's index.
    go_build: Option<(usize, &'a str)>,
    /// What follows `+build` on each legacy line that counts, with the line's index.
    plus_build: Vec<(usize, &'a str)>,
}

/// What follows `//go:build` when `line` is such a comment.
fn go_build_text(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("//go:build")?;
    (rest.is_empty() || rest.starts_with([' ', '\t'])).then_some(rest)
}

/// What follows `+build` when `line` is a legacy constraint comment.
fn plus_build_text(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("//")?.trim_start_matches([' ', '\t']);
    let rest = rest.strip_prefix("+build")?;
    (rest.is_empty() || rest.starts_with([' ', '\t'])).then_some(rest)
}

/// The constraint lines of the file's header, as `go/build` finds them: the header is
/// the run of blank lines and comments before the first other text; a `//go:build`
/// line counts anywhere in it outside a block comment, and a `+build` line only in
/// the leading `//` comments above the last blank line of that leading run. Two
/// `//go:build` lines are an error.
fn read_header(source: &str) -> Result<Header<'_>, ()> {
    let mut go_build = None;
    let mut candidates: Vec<(usize, &str)> = Vec::new();
    // Line index after the most recent blank line of the leading `//` run.
    let mut end = 0;
    let mut ended = false;
    let mut in_block = false;
    'lines: for (index, raw) in source.lines().enumerate() {
        let mut line = raw.trim();
        if line.is_empty() && !ended {
            end = index + 1;
            continue;
        }
        if !line.starts_with("//") {
            ended = true;
        }
        if !in_block {
            if let Some(text) = go_build_text(line) {
                if go_build.is_some() {
                    return Err(());
                }
                go_build = Some((index, text));
            }
            if !ended {
                if let Some(text) = plus_build_text(line) {
                    candidates.push((index, text));
                }
            }
        }
        loop {
            if line.is_empty() {
                continue 'lines;
            }
            if in_block {
                match line.find("*/") {
                    Some(i) => {
                        in_block = false;
                        line = line[i + 2..].trim();
                        continue;
                    }
                    None => continue 'lines,
                }
            }
            if line.starts_with("//") {
                continue 'lines;
            }
            if let Some(rest) = line.strip_prefix("/*") {
                in_block = true;
                line = rest.trim();
                continue;
            }
            // Text that is not a comment: the header is over.
            break 'lines;
        }
    }
    Ok(Header {
        go_build,
        plus_build: candidates
            .into_iter()
            .filter(|(index, _)| *index < end)
            .collect(),
    })
}

fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.'
}

/// Parses a `//go:build` expression. `None` when it is not one.
fn parse_go_build(text: &str) -> Option<Expr> {
    let mut tokens = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        match c {
            ' ' | '\t' => {}
            '(' | ')' | '!' => tokens.push(&text[start..start + 1]),
            '&' | '|' => {
                chars.next_if(|(_, next)| *next == c)?;
                tokens.push(&text[start..start + 2]);
            }
            c if is_tag_char(c) => {
                let mut end = start + c.len_utf8();
                while let Some((i, next)) = chars.next_if(|(_, next)| is_tag_char(*next)) {
                    end = i + next.len_utf8();
                }
                tokens.push(&text[start..end]);
            }
            _ => return None,
        }
    }
    let mut at = 0;
    let expr = parse_or(&tokens, &mut at, 0)?;
    (at == tokens.len()).then_some(expr)
}

/// Nesting deeper than this is not read.
const MAX_DEPTH: usize = 64;

fn parse_or(tokens: &[&str], at: &mut usize, depth: usize) -> Option<Expr> {
    let mut left = parse_and(tokens, at, depth)?;
    while tokens.get(*at) == Some(&"||") {
        *at += 1;
        let right = parse_and(tokens, at, depth)?;
        left = Expr::Or(Box::new(left), Box::new(right));
    }
    Some(left)
}

fn parse_and(tokens: &[&str], at: &mut usize, depth: usize) -> Option<Expr> {
    let mut left = parse_not(tokens, at, depth)?;
    while tokens.get(*at) == Some(&"&&") {
        *at += 1;
        let right = parse_not(tokens, at, depth)?;
        left = Expr::And(Box::new(left), Box::new(right));
    }
    Some(left)
}

fn parse_not(tokens: &[&str], at: &mut usize, depth: usize) -> Option<Expr> {
    if depth > MAX_DEPTH {
        return None;
    }
    match *tokens.get(*at)? {
        "!" => {
            *at += 1;
            // `!!x` is an error to the go tool.
            if tokens.get(*at) == Some(&"!") {
                return None;
            }
            Some(Expr::Not(Box::new(parse_not(tokens, at, depth + 1)?)))
        }
        "(" => {
            *at += 1;
            let inner = parse_or(tokens, at, depth + 1)?;
            if tokens.get(*at) != Some(&")") {
                return None;
            }
            *at += 1;
            Some(inner)
        }
        ")" | "&&" | "||" => None,
        tag => {
            *at += 1;
            Some(Expr::Tag(tag.to_string()))
        }
    }
}

/// Parses what follows `+build` on a legacy line: options separated by spaces are
/// alternatives, the comma-separated terms of an option all hold, and `!` negates a
/// term. `None` when a term is not a tag.
fn parse_plus_build(text: &str) -> Option<Expr> {
    let mut any: Option<Expr> = None;
    for option in text.split([' ', '\t']).filter(|o| !o.is_empty()) {
        let mut all: Option<Expr> = None;
        for term in option.split(',') {
            let (negated, tag) = match term.strip_prefix('!') {
                Some(tag) => (true, tag),
                None => (false, term),
            };
            if tag.is_empty() || !tag.chars().all(is_tag_char) {
                return None;
            }
            let mut expr = Expr::Tag(tag.to_string());
            if negated {
                expr = Expr::Not(Box::new(expr));
            }
            all = Some(match all {
                Some(prev) => Expr::And(Box::new(prev), Box::new(expr)),
                None => expr,
            });
        }
        let all = all?;
        any = Some(match any {
            Some(prev) => Expr::Or(Box::new(prev), Box::new(all)),
            None => all,
        });
    }
    // A `+build` line with no option holds for no build.
    Some(any.unwrap_or_else(|| Expr::Tag("ignore".to_string())))
}

fn collect_tags(expr: &Expr, tags: &mut Vec<String>) {
    match expr {
        Expr::Tag(tag) => {
            if !tags.contains(tag) {
                tags.push(tag.clone());
            }
        }
        Expr::Not(inner) => collect_tags(inner, tags),
        Expr::And(a, b) | Expr::Or(a, b) => {
            collect_tags(a, tags);
            collect_tags(b, tags);
        }
    }
}

fn holds(expr: &Expr, set: &dyn Fn(&str) -> bool) -> bool {
    match expr {
        Expr::Tag(tag) => set(tag),
        Expr::Not(inner) => !holds(inner, set),
        Expr::And(a, b) => holds(a, set) && holds(b, set),
        Expr::Or(a, b) => holds(a, set) || holds(b, set),
    }
}

/// Whether some assignment of `free` satisfies `expr`, every other tag being unset.
fn satisfiable(expr: &Expr, free: &[&String]) -> bool {
    (0u32..(1 << free.len())).any(|bits| {
        holds(expr, &|tag| {
            free.iter()
                .position(|f| f.as_str() == tag)
                .is_some_and(|i| bits & (1 << i) != 0)
        })
    })
}

fn evaluate(expr: &Expr) -> GoBuild {
    let mut tags = Vec::new();
    collect_tags(expr, &mut tags);
    if tags.len() > MAX_TAGS {
        return GoBuild::Unreadable;
    }
    let platform: Vec<&String> = tags.iter().filter(|t| is_platform_tag(t)).collect();
    if satisfiable(expr, &platform) {
        return GoBuild::Built;
    }
    // `ignore` is the tag no build sets; every other one `-tags` may set.
    let settable: Vec<&String> = tags.iter().filter(|t| t.as_str() != "ignore").collect();
    if !satisfiable(expr, &settable) {
        return GoBuild::Never;
    }
    let mut custom: Vec<String> = settable
        .into_iter()
        .filter(|t| !is_platform_tag(t))
        .cloned()
        .collect();
    custom.sort();
    GoBuild::NeedsTags(custom)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "package a\n\nimport \"testing\"\n\nfunc TestA(t *testing.T) {}\n";

    fn build(header: &str) -> GoBuild {
        go_build_constraint(&format!("{header}{BODY}"))
    }

    fn needs(tags: &[&str]) -> GoBuild {
        GoBuild::NeedsTags(tags.iter().map(|t| t.to_string()).collect())
    }

    /// Each case was compared with `go list -e -f '{{.TestGoFiles}} {{.IgnoredGoFiles}}
    /// {{.InvalidGoFiles}}'` (go 1.25, darwin/arm64): a `Built` file is a test file there
    /// (or an ignored one when its platform is another), a `Never` or `NeedsTags` file is
    /// ignored, and an `Unreadable` one is invalid, except the malformed `+build` line,
    /// which the go tool ignores the file for.
    #[test]
    fn constraints_are_read_as_the_go_tool_reads_them() {
        for (header, expected) in [
            ("", GoBuild::Built),
            ("//go:build ignore\n\n", GoBuild::Never),
            ("//go:build linux && !linux\n\n", GoBuild::Never),
            ("//go:build integration\n\n", needs(&["integration"])),
            ("//go:build !integration\n\n", GoBuild::Built),
            ("//go:build linux || darwin || windows\n\n", GoBuild::Built),
            ("//go:build cgo\n\n", GoBuild::Built),
            ("//go:build go1.21\n\n", GoBuild::Built),
            ("//go:build unix\n\n", GoBuild::Built),
            (
                "//go:build amd64.v3 && (linux || freebsd)\n\n",
                GoBuild::Built,
            ),
            (
                "//go:build darwin && integration || never2\n\n",
                needs(&["integration", "never2"]),
            ),
            ("//go:build (linux && slow) || ignore\n\n", needs(&["slow"])),
            // The legacy form: a blank line must follow the leading comments.
            ("// +build e2e\n\n", needs(&["e2e"])),
            ("// +build e2e\n", GoBuild::Built),
            ("// +build ignore\n\n", GoBuild::Never),
            ("// +build linux darwin\n\n", GoBuild::Built),
            ("// +build linux,!cgo darwin\n\n", GoBuild::Built),
            (
                "// +build linux darwin\n// +build slowtag\n\n",
                needs(&["slowtag"]),
            ),
            ("// +build !e2e\n\n", GoBuild::Built),
            // Comments and blank lines may come first; a block comment ends the
            // `+build` region and not the `//go:build` one.
            (
                "// Copyright\n\n/* block */\n//go:build smoke\n\n",
                needs(&["smoke"]),
            ),
            ("/* block */\n\n// +build smoke\n\n", GoBuild::Built),
            // `//go:build` decides when both forms are present.
            (
                "//go:build integration\n// +build ignore\n\n",
                needs(&["integration"]),
            ),
            // Not constraints.
            ("//go:buildx ignore\n\n", GoBuild::Built),
            ("// go:build ignore\n\n", GoBuild::Built),
            ("/*\n//go:build ignore\n*/\n\n", GoBuild::Built),
            // Rejected by the go tool.
            (
                "//go:build ignore\n//go:build linux\n\n",
                GoBuild::Unreadable,
            ),
            ("//go:build linux &&\n\n", GoBuild::Unreadable),
            ("//go:build !!linux\n\n", GoBuild::Unreadable),
            ("//go:build (linux\n\n", GoBuild::Unreadable),
            ("//go:build linux & darwin\n\n", GoBuild::Unreadable),
            ("// +build !!e2e\n\n", GoBuild::Unreadable),
        ] {
            assert_eq!(build(header), expected, "{header:?}");
        }
        // After the package clause it is an ordinary comment.
        assert_eq!(
            go_build_constraint(&format!("{BODY}\n//go:build ignore\n")),
            GoBuild::Built
        );
    }

    #[test]
    fn a_constraint_with_too_many_tags_is_not_evaluated() {
        let tags: Vec<String> = (0..=MAX_TAGS).map(|i| format!("tag{i}")).collect();
        let header = format!("//go:build {}\n\n", tags.join(" || "));
        assert_eq!(build(&header), GoBuild::Unreadable);
    }
}
