//! The `use` directives of a `go.work` file.
//!
//! `go help work` and the Go modules reference ("Workspaces"): a `go.work` file lists
//! the modules of a workspace with `use <dir>` lines or a `use ( ... )` block, each
//! directory relative to the file. While a `go.work` is in force (the nearest one at or
//! above the working directory, unless `GOWORK` says otherwise) a go command run in a
//! module the file does not `use` fails: `go test ./...` there reports `directory prefix
//! . does not contain modules listed in go.work`. A `use` entry does not widen `./...`:
//! in a workspace that uses a module and one nested below it, `./...` at the outer
//! module still matches the outer module's packages only.
//!
//! The grammar is the one of `go.mod`: line oriented, `//` comments, a directive with
//! its arguments on one line or a block in parentheses, and a path that is a bare word
//! or a double-quoted string.

/// What a `go.work` file says about the modules of its workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoWork {
    /// The `use` directories as written, in order.
    Uses(Vec<String>),
    /// The file is not one the go tool reads (an unterminated block, a block comment, a
    /// quoted path with an escape, a `use` with no path): every go command below it
    /// fails, or the paths cannot be told.
    Unreadable,
}

/// `line` without its `//` comment. A `//` inside a double-quoted string is kept.
fn strip_comment(line: &str) -> &str {
    let mut quoted = false;
    let bytes = line.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        match b {
            b'"' => quoted = !quoted,
            b'/' if !quoted && bytes.get(i + 1) == Some(&b'/') => return &line[..i],
            _ => {}
        }
    }
    line
}

/// One path argument: a bare word, or a double-quoted string with no escape in it.
fn path_argument(word: &str) -> Option<String> {
    if let Some(inner) = word.strip_prefix('"') {
        let inner = inner.strip_suffix('"')?;
        return (!inner.contains(['"', '\\'])).then(|| inner.to_string());
    }
    (!word.is_empty() && !word.contains(['"', '`', '\'', '(', ')', ' ', '\t']))
        .then(|| word.to_string())
}

/// Reads the `use` directives of a `go.work` file.
pub fn parse_go_work(source: &str) -> GoWork {
    let mut uses = Vec::new();
    // The directive whose block is open.
    let mut block: Option<String> = None;
    for raw in source.lines() {
        if raw.contains("/*") {
            return GoWork::Unreadable;
        }
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(verb) = &block {
            if line == ")" {
                block = None;
                continue;
            }
            if verb == "use" {
                match path_argument(line) {
                    Some(path) => uses.push(path),
                    None => return GoWork::Unreadable,
                }
            }
            continue;
        }
        let (verb, rest) = match line.split_once([' ', '\t']) {
            Some((verb, rest)) => (verb, rest.trim()),
            None => match line.strip_suffix('(') {
                Some(verb) => (verb, "("),
                None => (line, ""),
            },
        };
        if rest == "(" {
            block = Some(verb.to_string());
            continue;
        }
        if verb == "use" {
            match path_argument(rest) {
                Some(path) => uses.push(path),
                None => return GoWork::Unreadable,
            }
        }
    }
    if block.is_some() {
        return GoWork::Unreadable;
    }
    GoWork::Uses(uses)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uses(paths: &[&str]) -> GoWork {
        GoWork::Uses(paths.iter().map(|p| p.to_string()).collect())
    }

    /// Each readable case was accepted by `go list ./...` (go 1.25) in a workspace of
    /// that shape, with the listed modules in force; each unreadable one was refused
    /// with `errors parsing go.work`.
    #[test]
    fn use_directives_are_read_as_the_go_tool_reads_them() {
        for (source, expected) in [
            ("go 1.21\n\nuse ./a\n", uses(&["./a"])),
            ("go 1.21\n\nuse (\n\t.\n\t./sub\n)\n", uses(&[".", "./sub"])),
            (
                "go 1.21\n// use ./other\nuse (\n\t\"./sub\" // trailing\n\t./\n)\n",
                uses(&["./sub", "./"]),
            ),
            ("go 1.21\nuse (\n\t./sub\n)\nuse .\n", uses(&["./sub", "."])),
            (
                "go 1.21\ntoolchain go1.21.0\nuse .\nuse ./sub\nreplace example.test/x => ./other\n",
                uses(&[".", "./sub"]),
            ),
            (
                "go 1.21\nreplace (\n\texample.test/x => ./use\n)\nuse .\n",
                uses(&["."]),
            ),
            ("go 1.21\n", uses(&[])),
            ("go 1.21\nuse (\n . \n", GoWork::Unreadable),
            ("go 1.21\n/* use . */\nuse ./sub\n", GoWork::Unreadable),
            ("go 1.21\nuse `./sub`\n", GoWork::Unreadable),
            ("go 1.21\nuse\n", GoWork::Unreadable),
            ("go 1.21\nuse . ./sub\n", GoWork::Unreadable),
            ("go 1.21\nuse(\n\t.\n\t./sub\n)\n", uses(&[".", "./sub"])),
        ] {
            assert_eq!(parse_go_work(source), expected, "{source:?}");
        }
    }
}
