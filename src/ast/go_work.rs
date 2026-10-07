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
//! or a double-quoted string, read as a Go interpreted string literal (`"./\x61"` is
//! `./a`). The directives of a `go.work` are `go`, `toolchain`, `use`, `replace` and
//! `godebug`: the go tool refuses a file that holds any other (`unknown directive`), and
//! one with two `go` lines (`repeated go statement`).

/// What a `go.work` file says about the modules of its workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoWork {
    /// The `use` directories as written, in order.
    Uses(Vec<String>),
    /// The file is not one the go tool reads (an unterminated block, a block comment, a
    /// quoted path with an escape Go does not have, a `use` with no path, a directive
    /// that is not one of a `go.work`, a second `go` line): every go command below it
    /// fails.
    Unreadable,
}

/// The directives the go tool accepts in a `go.work`.
const DIRECTIVES: &[&str] = &["go", "toolchain", "use", "replace", "godebug"];

/// `line` without its `//` comment. A `//` inside a double-quoted string is kept, and a
/// `\"` inside one does not end it.
fn strip_comment(line: &str) -> &str {
    let mut quoted = false;
    let mut escaped = false;
    let bytes = line.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match b {
            b'\\' if quoted => escaped = true,
            b'"' => quoted = !quoted,
            b'/' if !quoted && bytes.get(i + 1) == Some(&b'/') => return &line[..i],
            _ => {}
        }
    }
    line
}

/// The value of the inside of a Go interpreted string literal, as `strconv.Unquote`
/// reads it: `\a \b \f \n \r \t \v \\ \"`, three octal digits, `\x` and two
/// hexadecimal digits, `\u` and four, `\U` and eight. `None` for any other escape, a
/// bare `"`, or a value that is not valid text.
fn unquote_interpreted(inner: &str) -> Option<String> {
    let mut out: Vec<u8> = Vec::with_capacity(inner.len());
    let mut chars = inner.chars();
    let digits = |chars: &mut std::str::Chars, n: usize, radix: u32| -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..n {
            value = value.checked_mul(radix)? + chars.next()?.to_digit(radix)?;
        }
        Some(value)
    };
    while let Some(c) = chars.next() {
        if c == '"' {
            return None;
        }
        if c != '\\' {
            out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
            continue;
        }
        match chars.next()? {
            'a' => out.push(0x07),
            'b' => out.push(0x08),
            'f' => out.push(0x0c),
            'n' => out.push(b'\n'),
            'r' => out.push(b'\r'),
            't' => out.push(b'\t'),
            'v' => out.push(0x0b),
            '\\' => out.push(b'\\'),
            '"' => out.push(b'"'),
            'x' => out.push(u8::try_from(digits(&mut chars, 2, 16)?).ok()?),
            first @ '0'..='7' => {
                let rest = digits(&mut chars, 2, 8)?;
                out.push(u8::try_from(first.to_digit(8)? * 64 + rest).ok()?);
            }
            'u' => {
                let c = char::from_u32(digits(&mut chars, 4, 16)?)?;
                out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
            }
            'U' => {
                let c = char::from_u32(digits(&mut chars, 8, 16)?)?;
                out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
            }
            _ => return None,
        }
    }
    String::from_utf8(out).ok()
}

/// One path argument: a bare word, or a double-quoted Go string.
fn path_argument(word: &str) -> Option<String> {
    if let Some(inner) = word.strip_prefix('"') {
        return unquote_interpreted(inner.strip_suffix('"')?);
    }
    (!word.is_empty() && !word.contains(['"', '`', '\'', '(', ')', ' ', '\t']))
        .then(|| word.to_string())
}

/// Reads the `use` directives of a `go.work` file.
pub fn parse_go_work(source: &str) -> GoWork {
    let mut uses = Vec::new();
    // The directive whose block is open.
    let mut block: Option<String> = None;
    let mut go_lines = 0;
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
        if !DIRECTIVES.contains(&verb) {
            return GoWork::Unreadable;
        }
        if rest == "(" {
            block = Some(verb.to_string());
            continue;
        }
        if verb == "go" {
            go_lines += 1;
            if go_lines > 1 {
                return GoWork::Unreadable;
            }
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

    /// `go list -m` (go 1.25) loaded the module each quoted path names: the string is a
    /// Go interpreted string literal. It refused `"./a\q"` with `invalid quoted string`.
    #[test]
    fn a_quoted_use_path_is_read_as_a_go_string() {
        for (source, expected) in [
            ("go 1.21\nuse \"./\\x61\"\n", uses(&["./a"])),
            ("go 1.21\nuse \"./\\141\"\n", uses(&["./a"])),
            ("go 1.21\nuse \"./\\u0061\"\n", uses(&["./a"])),
            ("go 1.21\nuse \"./sp ace\"\n", uses(&["./sp ace"])),
            ("go 1.21\nuse \"./sp\\x20ace\"\n", uses(&["./sp ace"])),
            ("go 1.21\nuse \"./sp\\tace\"\n", uses(&["./sp\tace"])),
            ("go 1.21\nuse \"./q\\\"uote\"\n", uses(&["./q\"uote"])),
            (
                "go 1.21\nuse \"./back\\\\slash\"\n",
                uses(&["./back\\slash"]),
            ),
            (
                "go 1.21\nuse (\n\t\"./\\x61\" // the first\n\t./sub\n)\n",
                uses(&["./a", "./sub"]),
            ),
            ("go 1.21\nuse \"./a\\q\"\n", GoWork::Unreadable),
            ("go 1.21\nuse \"./a\\x6\"\n", GoWork::Unreadable),
            ("go 1.21\nuse \"./a\\\"\n", GoWork::Unreadable),
        ] {
            assert_eq!(parse_go_work(source), expected, "{source:?}");
        }
    }

    /// `go list -m` (go 1.25) refused each of the first group with `unknown directive`
    /// or `unknown block type`, the two `go` lines with `repeated go statement`, and
    /// loaded the module with each directive of the last group.
    #[test]
    fn a_directive_the_go_tool_does_not_know_makes_the_file_unreadable() {
        for line in [
            "bogus x",
            "bogus (\n x\n)",
            "require example.test/x v1.0.0",
            "exclude example.test/x v1.0.0",
            "retract v1.0.0",
            "module example.test/w",
            "tool example.test/x",
            "ignore ./sub",
            "Use ./sub",
        ] {
            let source = format!("go 1.21\nuse ./a\n{line}\n");
            assert_eq!(parse_go_work(&source), GoWork::Unreadable, "{source:?}");
        }
        assert_eq!(
            parse_go_work("go 1.21\ngo 1.22\nuse ./a\n"),
            GoWork::Unreadable
        );
        for line in [
            "godebug default=go1.21",
            "toolchain go1.21.0",
            "replace example.test/x => ./sub",
        ] {
            let source = format!("go 1.21\nuse ./a\n{line}\n");
            assert_eq!(parse_go_work(&source), uses(&["./a"]), "{source:?}");
        }
    }
}
