//! Compiler warning flags in build files, for `toolchain-config`.
//!
//! A change can lower the compiler's bar without touching a lint table: `-Wno-error` in a
//! `Makefile`, `add_compile_options(-w)` in a `CMakeLists.txt`, `extra_compile_args=["-w"]`
//! in a `setup.py`, `.warnings(false)` in a `build.rs`. Each recognised file is read, on the
//! base side and the head side, into the flag uses it contains; [`judge`] reports a lax flag
//! added and a strict flag lost, the way the data-file rules of `toolchain-config` do.
//!
//! Readers:
//! - `build.rs` (tree-sitter Rust) and `setup.py` (tree-sitter Python) read syntax nodes: a
//!   flag in a comment or in a string that is not passed to the builder is not a flag.
//! - `Makefile` and CMake have no grammar in this project, so each has a small lexer:
//!   comments are removed and line continuations joined before a word is looked at; a
//!   `$(...)` / `${...}` reference is one opaque word. Their limits are listed in
//!   `docs/GATES.md`.
//!
//! A flag use is keyed by the *context* that carries it (the variable, the CMake command,
//! `recipe`, `extra_compile_args`, `cc::Build`). A lax flag is new when its context holds
//! more of it than the base side did; a strict flag is lost when its context holds none of
//! it any more.

#[cfg(any(feature = "lang-rust", feature = "lang-python"))]
use tree_sitter::{Node, Parser};

/// Which reader a build file takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildKind {
    Make,
    CMake,
    SetupPy,
    BuildRs,
}

/// A file the readers apply to, by basename.
pub fn classify_build(path: &str) -> Option<BuildKind> {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name {
        "Makefile" | "makefile" | "GNUmakefile" => Some(BuildKind::Make),
        "CMakeLists.txt" => Some(BuildKind::CMake),
        "setup.py" => Some(BuildKind::SetupPy),
        "build.rs" => Some(BuildKind::BuildRs),
        _ if name.len() > 3 && name.ends_with(".mk") => Some(BuildKind::Make),
        _ if name.len() > 6 && name.ends_with(".cmake") => Some(BuildKind::CMake),
        _ => None,
    }
}

/// One warning flag read from a build file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagUse {
    /// Variable, command or call that carries it: `CFLAGS`, `CMAKE_CXX_FLAGS`,
    /// `add_compile_options`, `recipe`, `extra_compile_args`, `cc::Build`.
    pub context: String,
    /// The flag, normalised (`-D warnings`, `.warnings(false)`).
    pub flag: String,
    /// `true` for a flag whose loss loosens the build, `false` for one whose gain does.
    pub strict: bool,
    /// 1-based line in the file it was read from.
    pub line: usize,
}

/// Why a file could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractError {
    /// The text does not parse, so a flag in it could be missed.
    Unparsed,
    /// The grammar this reader needs is not compiled into this binary.
    GrammarAbsent,
}

/// A flag added or lost between the base and the head side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub context: String,
    pub flag: String,
    /// `true`: a lax flag gained. `false`: a strict flag lost.
    pub gained: bool,
    /// Head-side line, for a gained flag.
    pub line: Option<usize>,
}

/// Read every warning flag use in `src`.
pub fn extract(kind: BuildKind, src: &str) -> Result<Vec<FlagUse>, ExtractError> {
    match kind {
        BuildKind::Make => Ok(make_flags(src)),
        BuildKind::CMake => cmake_flags(src),
        BuildKind::SetupPy => setup_py_flags(src),
        BuildKind::BuildRs => build_rs_flags(src),
    }
}

/// Lax flags gained and strict flags lost going from `base` to `head`.
pub fn judge(base: &[FlagUse], head: &[FlagUse]) -> Vec<Change> {
    let count = |uses: &[FlagUse], u: &FlagUse| {
        uses.iter()
            .filter(|b| b.strict == u.strict && b.context == u.context && b.flag == u.flag)
            .count()
    };
    let mut out = Vec::new();
    let mut seen: Vec<(&str, &str, usize)> = Vec::new();
    for u in head.iter().filter(|u| !u.strict) {
        let n = match seen
            .iter_mut()
            .find(|(c, f, _)| *c == u.context && *f == u.flag)
        {
            Some(entry) => {
                entry.2 += 1;
                entry.2
            }
            None => {
                seen.push((&u.context, &u.flag, 1));
                1
            }
        };
        if n > count(base, u) {
            out.push(Change {
                context: u.context.clone(),
                flag: u.flag.clone(),
                gained: true,
                line: Some(u.line),
            });
        }
    }
    let mut lost: Vec<(&str, &str)> = Vec::new();
    for u in base.iter().filter(|u| u.strict) {
        if lost.contains(&(u.context.as_str(), u.flag.as_str())) {
            continue;
        }
        if count(head, u) == 0 {
            lost.push((&u.context, &u.flag));
            out.push(Change {
                context: u.context.clone(),
                flag: u.flag.clone(),
                gained: false,
                line: None,
            });
        }
    }
    out
}

// ---- flag vocabulary -------------------------------------------------------

/// Flags whose loss lowers the bar. `-Werror=<diag>` is matched by prefix, `-D warnings`
/// is two words.
const BUILD_STRICT: &[&str] = &["-Werror", "-Wall", "-Wextra", "-Wpedantic"];

/// Read the flag words of one context into flag uses. Words that are neither strict nor
/// lax are dropped.
fn classify_words(
    context: &str,
    words: &[String],
    line_of: &dyn Fn(usize) -> usize,
    out: &mut Vec<FlagUse>,
) {
    let mut i = 0;
    while i < words.len() {
        let mut push = |flag: String, strict: bool| {
            out.push(FlagUse {
                context: context.to_string(),
                flag,
                strict,
                line: line_of(i),
            })
        };
        let w = words[i].as_str();
        let next = words.get(i + 1).map(String::as_str);
        match w {
            "-D" if next == Some("warnings") => {
                push("-D warnings".to_string(), true);
                i += 2;
                continue;
            }
            "-Dwarnings" => push("-D warnings".to_string(), true),
            "-A" if next == Some("warnings") => {
                push("-A warnings".to_string(), false);
                i += 2;
                continue;
            }
            "-Awarnings" => push("-A warnings".to_string(), false),
            "--cap-lints" if matches!(next, Some("allow" | "warn")) => {
                push(format!("--cap-lints {}", next.unwrap_or_default()), false);
                i += 2;
                continue;
            }
            "--cap-lints=allow" | "--cap-lints=warn" => push(w.replace('=', " "), false),
            "-w" => push("-w".to_string(), false),
            _ if BUILD_STRICT.contains(&w) || w.starts_with("-Werror=") => {
                push(w.to_string(), true)
            }
            _ if crate::guards::toolchain_config::LAX_FLAG_PREFIXES
                .iter()
                .any(|p| w.starts_with(p)) =>
            {
                push(w.to_string(), false)
            }
            _ => {}
        }
        i += 1;
    }
}

/// Split `s` into words the way a shell or `make` would, for what this reader needs: runs
/// of whitespace separate words, quotes group and are removed, a backslash escapes the next
/// character, and `$(...)` / `${...}` is part of one word whatever it contains. With
/// `shell`, a `#` that starts a word ends the line (a shell comment).
fn split_words(s: &str, shell: bool) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut depth = 0usize;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else if c == '\\' && q == '"' && i + 1 < chars.len() {
                cur.push(chars[i + 1]);
                i += 1;
            } else {
                cur.push(c);
            }
            i += 1;
            continue;
        }
        if depth > 0 {
            cur.push(c);
            match c {
                '(' | '{' => depth += 1,
                ')' | '}' => depth -= 1,
                _ => {}
            }
            i += 1;
            continue;
        }
        match c {
            '$' if matches!(chars.get(i + 1), Some('(' | '{')) => {
                cur.push('$');
                cur.push(chars[i + 1]);
                depth = 1;
                started = true;
                i += 2;
                continue;
            }
            '\'' | '"' => {
                quote = Some(c);
                started = true;
            }
            '\\' if i + 1 < chars.len() => {
                cur.push(chars[i + 1]);
                started = true;
                i += 1;
            }
            '#' if shell && !started => break,
            c if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            _ => {
                cur.push(c);
                started = true;
            }
        }
        i += 1;
    }
    if started {
        out.push(cur);
    }
    out
}

// ---- Makefile --------------------------------------------------------------

/// Logical lines of a makefile: a line ending in an odd number of backslashes continues on
/// the next one. Each carries the number of the physical line it starts on.
fn make_lines(src: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut pending: Option<(usize, String)> = None;
    for (idx, raw) in src.lines().enumerate() {
        let (start, mut text) = pending.take().unwrap_or((idx + 1, String::new()));
        text.push_str(raw);
        let slashes = text.chars().rev().take_while(|c| *c == '\\').count();
        if slashes % 2 == 1 {
            text.pop();
            text.push(' ');
            pending = Some((start, text));
        } else {
            out.push((start, text));
        }
    }
    if let Some(p) = pending {
        out.push(p);
    }
    out
}

/// `make` ends a non-recipe line at the first `#` not escaped by a backslash.
fn strip_make_comment(line: &str) -> &str {
    let mut prev = '\0';
    for (i, c) in line.char_indices() {
        if c == '#' && prev != '\\' {
            return &line[..i];
        }
        prev = c;
    }
    line
}

/// `[target:] [override|export|private] NAME op value` into `(NAME, value)`. `None` when the
/// line is not an assignment.
fn parse_assignment(line: &str) -> Option<(String, String)> {
    let eq = line.find('=')?;
    let bytes = line.as_bytes();
    let mut op_start = eq;
    while op_start > 0 && matches!(bytes[op_start - 1], b':' | b'+' | b'?' | b'!') {
        op_start -= 1;
    }
    let mut lhs = &line[..op_start];
    if let Some(colon) = lhs.find(':') {
        lhs = &lhs[colon + 1..];
    }
    let words: Vec<&str> = lhs
        .split_whitespace()
        .filter(|w| !matches!(*w, "override" | "export" | "private" | "unexport"))
        .collect();
    let [name] = words.as_slice() else {
        return None;
    };
    let valid = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '+'));
    valid.then(|| (name.to_string(), line[eq + 1..].to_string()))
}

/// Variables that carry compiler flags: `CFLAGS`, `CXXFLAGS`, `LDFLAGS`, `RUSTFLAGS`,
/// `AM_CFLAGS`, `CFLAGS_main.o`.
fn is_flag_variable(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.ends_with("FLAGS") || upper.contains("FLAGS_")
}

/// Whether a shell word names a compiler: `cc`, `gcc-12`, `x86_64-linux-gnu-g++`, `clang`,
/// `rustc`, `$(CC)`, `${CXX}`, `$(CROSS_COMPILE)gcc`.
fn is_compiler_word(word: &str) -> bool {
    let mut rest = word;
    let mut only_reference = None;
    while let Some(after) = rest.strip_prefix("$(").or_else(|| rest.strip_prefix("${")) {
        let Some(end) = after.find([')', '}']) else {
            return false;
        };
        only_reference = Some(&after[..end]);
        rest = &after[end + 1..];
    }
    if rest.is_empty() {
        return only_reference.is_some_and(|v| {
            matches!(
                v,
                "CC" | "CXX"
                    | "CPP"
                    | "HOSTCC"
                    | "HOSTCXX"
                    | "CC_FOR_BUILD"
                    | "CXX_FOR_BUILD"
                    | "RUSTC"
                    | "CLANG"
            )
        });
    }
    let base = rest.rsplit('/').next().unwrap_or(rest);
    let versioned = base
        .rsplit_once('-')
        .is_some_and(|(_, v)| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.'));
    let base = if versioned {
        base.rsplit_once('-').map_or(base, |(b, _)| b)
    } else {
        base
    };
    const NAMES: &[&str] = &[
        "cc", "gcc", "g++", "c++", "clang", "clang++", "cpp", "rustc",
    ];
    NAMES.contains(&base)
        || NAMES
            .iter()
            .any(|n| base.strip_suffix(n).is_some_and(|p| p.ends_with('-')))
}

fn make_flags(src: &str) -> Vec<FlagUse> {
    let mut out = Vec::new();
    for (line, text) in make_lines(src) {
        if text.starts_with('\t') {
            let mut words = split_words(&text, true);
            if let Some(first) = words.first_mut() {
                *first = first.trim_start_matches(['@', '-', '+']).to_string();
            }
            if words.iter().any(|w| is_compiler_word(w)) {
                classify_words("recipe", &words, &|_| line, &mut out);
            }
            continue;
        }
        let Some((name, value)) = parse_assignment(strip_make_comment(&text)) else {
            continue;
        };
        if is_flag_variable(&name) {
            classify_words(&name, &split_words(&value, false), &|_| line, &mut out);
        }
    }
    out
}

// ---- CMake -----------------------------------------------------------------

struct Lexer {
    c: Vec<char>,
    i: usize,
    line: usize,
}

struct CmakeCommand {
    name: String,
    args: Vec<String>,
    line: usize,
}

impl Lexer {
    fn peek(&self) -> Option<char> {
        self.c.get(self.i).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.i += 1;
        if ch == '\n' {
            self.line += 1;
        }
        Some(ch)
    }

    /// At `[`, the `=` count of a bracket opener (`[[`, `[=[`), if one starts here.
    fn bracket_level(&self) -> Option<usize> {
        if self.peek() != Some('[') {
            return None;
        }
        let mut j = self.i + 1;
        let mut n = 0;
        while self.c.get(j) == Some(&'=') {
            n += 1;
            j += 1;
        }
        (self.c.get(j) == Some(&'[')).then_some(n)
    }

    fn at_close(&self, level: usize) -> bool {
        self.c.get(self.i) == Some(&']')
            && (1..=level).all(|k| self.c.get(self.i + k) == Some(&'='))
            && self.c.get(self.i + level + 1) == Some(&']')
    }

    /// Consume a bracket argument or comment; `None` when it never closes.
    fn read_bracket(&mut self, level: usize) -> Option<String> {
        for _ in 0..level + 2 {
            self.bump();
        }
        let mut s = String::new();
        loop {
            if self.at_close(level) {
                for _ in 0..level + 2 {
                    self.bump();
                }
                return Some(s);
            }
            s.push(self.bump()?);
        }
    }

    /// Consume a bracket comment, up to its close or the end of input.
    fn skip_bracket(&mut self, level: usize) {
        for _ in 0..level + 2 {
            self.bump();
        }
        while self.peek().is_some() && !self.at_close(level) {
            self.bump();
        }
        if self.at_close(level) {
            for _ in 0..level + 2 {
                self.bump();
            }
        }
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(ch) if ch.is_whitespace() => {
                    self.bump();
                }
                Some('#') => {
                    self.bump();
                    if let Some(level) = self.bracket_level() {
                        self.skip_bracket(level);
                    } else {
                        while let Some(ch) = self.peek() {
                            if ch == '\n' {
                                break;
                            }
                            self.bump();
                        }
                    }
                }
                _ => break,
            }
        }
    }

    /// Arguments up to the matching `)`; `None` at end of input first.
    fn read_args(&mut self) -> Option<Vec<String>> {
        let mut depth = 1usize;
        let mut args = Vec::new();
        loop {
            self.skip_trivia();
            match self.peek()? {
                ')' => {
                    self.bump();
                    depth -= 1;
                    if depth == 0 {
                        return Some(args);
                    }
                }
                '(' => {
                    self.bump();
                    depth += 1;
                }
                '"' => {
                    self.bump();
                    let mut s = String::new();
                    loop {
                        match self.bump()? {
                            '"' => break,
                            '\\' => {
                                if let Some(n) = self.bump() {
                                    s.push(n);
                                }
                            }
                            other => s.push(other),
                        }
                    }
                    args.push(s);
                }
                '[' if self.bracket_level().is_some() => {
                    let level = self.bracket_level().unwrap_or(0);
                    args.push(self.read_bracket(level)?);
                }
                _ => {
                    let mut s = String::new();
                    while let Some(c) = self.peek() {
                        if c.is_whitespace() || matches!(c, '(' | ')' | '#' | '"') {
                            break;
                        }
                        self.bump();
                        if c == '\\' {
                            if let Some(n) = self.bump() {
                                s.push(n);
                            }
                            continue;
                        }
                        s.push(c);
                    }
                    args.push(s);
                }
            }
        }
    }
}

fn cmake_commands(src: &str) -> Option<Vec<CmakeCommand>> {
    let mut lx = Lexer {
        c: src.chars().collect(),
        i: 0,
        line: 1,
    };
    let mut out = Vec::new();
    loop {
        lx.skip_trivia();
        let Some(ch) = lx.peek() else { break };
        if !(ch.is_ascii_alphabetic() || ch == '_') {
            lx.bump();
            continue;
        }
        let line = lx.line;
        let mut name = String::new();
        while let Some(c) = lx.peek() {
            if !(c.is_ascii_alphanumeric() || c == '_') {
                break;
            }
            name.push(c);
            lx.bump();
        }
        while matches!(lx.peek(), Some(' ' | '\t')) {
            lx.bump();
        }
        if lx.peek() == Some('(') {
            lx.bump();
            let args = lx.read_args()?;
            out.push(CmakeCommand { name, args, line });
        }
    }
    Some(out)
}

/// `CMAKE_C_FLAGS`, `CMAKE_CXX_FLAGS_RELEASE`, `CMAKE_C_FLAGS_INIT`.
fn is_cmake_flag_variable(name: &str) -> bool {
    name.starts_with("CMAKE_") && (name.ends_with("_FLAGS") || name.contains("_FLAGS_"))
}

fn cmake_words(args: &[String]) -> Vec<String> {
    let mut words: Vec<String> = args.iter().flat_map(|a| split_words(a, false)).collect();
    for w in &mut words {
        if let Some(rest) = w.strip_prefix("SHELL:") {
            *w = rest.to_string();
        }
    }
    words
}

fn cmake_flags(src: &str) -> Result<Vec<FlagUse>, ExtractError> {
    let commands = cmake_commands(src).ok_or(ExtractError::Unparsed)?;
    let mut out = Vec::new();
    for cmd in &commands {
        let name = cmd.name.to_ascii_lowercase();
        let args = &cmd.args;
        match name.as_str() {
            "set" => {
                let Some(var) = args.first().filter(|v| is_cmake_flag_variable(v)) else {
                    continue;
                };
                let values: Vec<String> = args[1..]
                    .iter()
                    .take_while(|a| !matches!(a.as_str(), "CACHE" | "PARENT_SCOPE"))
                    .cloned()
                    .collect();
                classify_words(var, &cmake_words(&values), &|_| cmd.line, &mut out);
            }
            "string" => {
                let is_append = args.first().is_some_and(|a| {
                    a.eq_ignore_ascii_case("APPEND") || a.eq_ignore_ascii_case("PREPEND")
                });
                let Some(var) = args
                    .get(1)
                    .filter(|v| is_append && is_cmake_flag_variable(v))
                else {
                    continue;
                };
                classify_words(var, &cmake_words(&args[2..]), &|_| cmd.line, &mut out);
            }
            "add_compile_options" => {
                classify_words(&name, &cmake_words(args), &|_| cmd.line, &mut out);
            }
            "target_compile_options" => {
                let Some(target) = args.first() else { continue };
                let values: Vec<String> = args[1..]
                    .iter()
                    .filter(|a| {
                        !matches!(a.as_str(), "BEFORE" | "PUBLIC" | "PRIVATE" | "INTERFACE")
                    })
                    .cloned()
                    .collect();
                classify_words(
                    &format!("target_compile_options:{target}"),
                    &cmake_words(&values),
                    &|_| cmd.line,
                    &mut out,
                );
            }
            _ => {}
        }
    }
    Ok(out)
}

// ---- setup.py --------------------------------------------------------------

#[cfg(feature = "lang-python")]
fn setup_py_flags(src: &str) -> Result<Vec<FlagUse>, ExtractError> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .map_err(|_| ExtractError::Unparsed)?;
    let tree = crate::ast::source_text::parse(&mut parser, src).ok_or(ExtractError::Unparsed)?;
    let root = tree.root_node();
    if root.has_error() {
        return Err(ExtractError::Unparsed);
    }
    let mut out = Vec::new();
    walk_python(root, src.as_bytes(), &mut out);
    Ok(out)
}

#[cfg(not(feature = "lang-python"))]
fn setup_py_flags(_src: &str) -> Result<Vec<FlagUse>, ExtractError> {
    Err(ExtractError::GrammarAbsent)
}

#[cfg(feature = "lang-python")]
const PY_ARGS: &str = "extra_compile_args";

#[cfg(feature = "lang-python")]
fn walk_python(node: Node, src: &[u8], out: &mut Vec<FlagUse>) {
    let text = |n: Node| n.utf8_text(src).unwrap_or("");
    let value = match node.kind() {
        "keyword_argument" => node
            .child_by_field_name("name")
            .filter(|n| text(*n) == PY_ARGS)
            .and_then(|_| node.child_by_field_name("value")),
        "assignment" | "augmented_assignment" => node
            .child_by_field_name("left")
            .filter(|n| text(*n) == PY_ARGS)
            .and_then(|_| node.child_by_field_name("right")),
        "pair" => node
            .child_by_field_name("key")
            .filter(|n| n.kind() == "string" && python_string(*n, src).as_deref() == Some(PY_ARGS))
            .and_then(|_| node.child_by_field_name("value")),
        _ => None,
    };
    if let Some(value) = value {
        let mut strings = Vec::new();
        python_strings(value, src, &mut strings);
        let mut words = Vec::new();
        let mut rows = Vec::new();
        for (row, s) in &strings {
            for w in split_words(s, false) {
                words.push(w);
                rows.push(*row);
            }
        }
        classify_words(PY_ARGS, &words, &|i| rows[i], out);
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_python(child, src, out);
    }
}

/// The text of a string literal without its quotes; interpolated parts are dropped.
#[cfg(feature = "lang-python")]
fn python_string(node: Node, src: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let mut s = String::new();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_content" {
            s.push_str(child.utf8_text(src).ok()?);
        }
    }
    Some(s)
}

/// Every string literal under `node`, with the 1-based line it starts on.
#[cfg(feature = "lang-python")]
fn python_strings(node: Node, src: &[u8], out: &mut Vec<(usize, String)>) {
    if node.kind() == "string" {
        if let Some(s) = python_string(node, src) {
            out.push((node.start_position().row + 1, s));
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        python_strings(child, src, out);
    }
}

// ---- build.rs --------------------------------------------------------------

#[cfg(feature = "lang-rust")]
fn build_rs_flags(src: &str) -> Result<Vec<FlagUse>, ExtractError> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .map_err(|_| ExtractError::Unparsed)?;
    let tree = crate::ast::source_text::parse(&mut parser, src).ok_or(ExtractError::Unparsed)?;
    let root = tree.root_node();
    if root.has_error() {
        return Err(ExtractError::Unparsed);
    }
    let mut out = Vec::new();
    walk_rust(root, src.as_bytes(), &mut out);
    Ok(out)
}

#[cfg(not(feature = "lang-rust"))]
fn build_rs_flags(_src: &str) -> Result<Vec<FlagUse>, ExtractError> {
    Err(ExtractError::GrammarAbsent)
}

/// The text of a string literal without its quotes; `None` for anything else.
#[cfg(feature = "lang-rust")]
fn rust_string(node: Node, src: &[u8]) -> Option<String> {
    let raw = node.utf8_text(src).ok()?;
    match node.kind() {
        "string_literal" => Some(raw.trim_matches('"').to_string()),
        "raw_string_literal" => Some(
            raw.trim_start_matches('r')
                .trim_matches('#')
                .trim_matches('"')
                .to_string(),
        ),
        _ => None,
    }
}

/// `.flag("-w")`, `.flag_if_supported("-Wno-x")`, `.warnings(false)`,
/// `.warnings_into_errors(..)`, `.extra_warnings(false)`: a method call by that name whose
/// first argument is a literal of the kind the method takes.
#[cfg(feature = "lang-rust")]
fn walk_rust(node: Node, src: &[u8], out: &mut Vec<FlagUse>) {
    const CONTEXT: &str = "cc::Build";
    // Children first: in a builder chain the receiver is the inner call, which reads first.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_rust(child, src, out);
    }
    if node.kind() == "call_expression" {
        let method = node
            .child_by_field_name("function")
            .filter(|f| f.kind() == "field_expression")
            .and_then(|f| f.child_by_field_name("field"))
            .and_then(|f| f.utf8_text(src).ok());
        let arg = node
            .child_by_field_name("arguments")
            .and_then(|a| a.named_child(0));
        if let (Some(method), Some(arg)) = (method, arg) {
            let line = arg.start_position().row + 1;
            let boolean = (arg.kind() == "boolean_literal")
                .then(|| arg.utf8_text(src).ok())
                .flatten();
            let found: Vec<(String, bool)> = match (method, boolean) {
                ("flag" | "flag_if_supported", _) => {
                    let mut flags = Vec::new();
                    if let Some(s) = rust_string(arg, src) {
                        classify_words(CONTEXT, &split_words(&s, false), &|_| line, &mut flags);
                    }
                    flags.into_iter().map(|u| (u.flag, u.strict)).collect()
                }
                ("warnings", Some("false")) => vec![(".warnings(false)".to_string(), false)],
                ("extra_warnings", Some("false")) => {
                    vec![(".extra_warnings(false)".to_string(), false)]
                }
                ("warnings_into_errors", Some("false")) => {
                    vec![(".warnings_into_errors(false)".to_string(), false)]
                }
                ("warnings_into_errors", Some("true")) => {
                    vec![(".warnings_into_errors(true)".to_string(), true)]
                }
                _ => Vec::new(),
            };
            for (flag, strict) in found {
                out.push(FlagUse {
                    context: CONTEXT.to_string(),
                    flag,
                    strict,
                    line,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags(kind: BuildKind, src: &str) -> Vec<(String, String, bool)> {
        extract(kind, src)
            .expect("reads")
            .into_iter()
            .map(|u| (u.context, u.flag, u.strict))
            .collect()
    }

    fn changes(kind: BuildKind, base: &str, head: &str) -> Vec<(String, String, bool)> {
        let b = extract(kind, base).expect("base reads");
        let h = extract(kind, head).expect("head reads");
        judge(&b, &h)
            .into_iter()
            .map(|c| (c.context, c.flag, c.gained))
            .collect()
    }

    fn c(ctx: &str, flag: &str, gained: bool) -> (String, String, bool) {
        (ctx.to_string(), flag.to_string(), gained)
    }

    #[test]
    fn classification_covers_the_build_files_and_nothing_else() {
        assert_eq!(classify_build("Makefile"), Some(BuildKind::Make));
        assert_eq!(classify_build("sub/GNUmakefile"), Some(BuildKind::Make));
        assert_eq!(classify_build("rules/common.mk"), Some(BuildKind::Make));
        assert_eq!(classify_build("CMakeLists.txt"), Some(BuildKind::CMake));
        assert_eq!(
            classify_build("cmake/Warnings.cmake"),
            Some(BuildKind::CMake)
        );
        assert_eq!(classify_build("pkg/setup.py"), Some(BuildKind::SetupPy));
        assert_eq!(
            classify_build("crates/a/build.rs"),
            Some(BuildKind::BuildRs)
        );
        assert_eq!(classify_build("docs/Makefile.md"), None);
        assert_eq!(classify_build(".mk"), None);
        assert_eq!(classify_build("src/build.rs.bak"), None);
        assert_eq!(classify_build("setup.cfg"), None);
    }

    // ---- Makefile ----

    #[test]
    fn makefile_added_lax_flags_and_lost_strict_flags_are_reported() {
        let base = "CFLAGS = -O2 -Wall -Werror\nLDFLAGS = -lm\n";
        // `-Wno-error` added.
        assert_eq!(
            changes(
                BuildKind::Make,
                base,
                "CFLAGS = -O2 -Wall -Werror -Wno-error\nLDFLAGS = -lm\n"
            ),
            vec![c("CFLAGS", "-Wno-error", true)]
        );
        // `-w` added by `+=`.
        assert_eq!(
            changes(BuildKind::Make, base, &format!("{base}CFLAGS += -w\n")),
            vec![c("CFLAGS", "-w", true)]
        );
        // `-Werror` removed.
        assert_eq!(
            changes(BuildKind::Make, base, "CFLAGS = -O2 -Wall\nLDFLAGS = -lm\n"),
            vec![c("CFLAGS", "-Werror", false)]
        );
        // Other flag variables, and the assignment forms, are read.
        for assign in [
            "CXXFLAGS := -Wno-unused",
            "CPPFLAGS ?= -Wno-unused",
            "AM_CFLAGS=-Wno-unused",
        ] {
            let got = changes(BuildKind::Make, "", &format!("{assign}\n"));
            assert_eq!(got.len(), 1, "{assign}: {got:?}");
            assert_eq!(got[0].1, "-Wno-unused");
        }
        // A target-specific and an `override`/`export` assignment.
        assert_eq!(
            changes(BuildKind::Make, "", "legacy.o: CFLAGS += -w\n"),
            vec![c("CFLAGS", "-w", true)]
        );
        assert_eq!(
            changes(
                BuildKind::Make,
                "",
                "override export CFLAGS = -Wno-error=format\n"
            ),
            vec![c("CFLAGS", "-Wno-error=format", true)]
        );
        // `RUSTFLAGS` loses `-D warnings`, gains `-A warnings`.
        assert_eq!(
            changes(
                BuildKind::Make,
                "RUSTFLAGS = -D warnings\n",
                "RUSTFLAGS = -A warnings\n"
            ),
            vec![
                c("RUSTFLAGS", "-A warnings", true),
                c("RUSTFLAGS", "-D warnings", false)
            ]
        );
    }

    #[test]
    fn makefile_recipe_lines_are_read_only_when_they_invoke_a_compiler() {
        let head = "all:\n\t@$(CC) -Wno-error -c a.c\n\tgcc-12 -w b.c\n\tx86_64-linux-gnu-g++ -Wno-unused c.cc\n\techo -w -Wno-nothing\n\trustc -A warnings x.rs\n";
        let got = changes(BuildKind::Make, "all:\n", head);
        assert_eq!(
            got,
            vec![
                c("recipe", "-Wno-error", true),
                c("recipe", "-w", true),
                c("recipe", "-Wno-unused", true),
                c("recipe", "-A warnings", true),
            ]
        );
        // A recipe line's `-Werror` is lost when no compiler line carries it any more.
        assert_eq!(
            changes(
                BuildKind::Make,
                "all:\n\tcc -Werror a.c\n",
                "all:\n\tcc a.c\n"
            ),
            vec![c("recipe", "-Werror", false)]
        );
        assert!(!is_compiler_word("echo") && !is_compiler_word("$(MAKE)"));
        assert!(is_compiler_word("$(CROSS_COMPILE)gcc") && is_compiler_word("${CXX}"));
    }

    #[test]
    fn makefile_negative_controls() {
        let base = "CFLAGS = -O2 -w\nall:\n\tcc $(CFLAGS) a.c\n";
        // An unchanged pre-existing `-w` is not reported, even when the file is reformatted.
        assert!(changes(
            BuildKind::Make,
            base,
            "# build\nCFLAGS := -w -O2\nall:\n\tcc $(CFLAGS) a.c\n"
        )
        .is_empty());
        // A strict flag added, and one moved between lines, are not weakenings.
        assert!(changes(BuildKind::Make, base, "CFLAGS = -O2 -w -Wall -Werror\n").is_empty());
        // `-Wno-` in a comment, on a continued comment line, or escaped.
        let commented =
            "CFLAGS = -O2 # was -Wno-error\n# CFLAGS += -w\nLDFLAGS = -lm \\\n  # -Wno-x\n";
        assert!(
            flags(BuildKind::Make, commented).is_empty(),
            "{:?}",
            flags(BuildKind::Make, commented)
        );
        // A variable that does not carry flags, a function call, and a quoted word.
        let other = "NAME = -w\nCFLAGS += $(shell echo -w)\nCFLAGS += $(filter -Wno-x,$(Y))\n";
        assert!(
            flags(BuildKind::Make, other).is_empty(),
            "{:?}",
            flags(BuildKind::Make, other)
        );
        // A `-D` macro is not `-D warnings`.
        assert!(flags(BuildKind::Make, "CFLAGS = -D FOO -DBAR=1\n").is_empty());
        // A recipe that is not a compiler call.
        assert!(flags(BuildKind::Make, "x:\n\tstrip -w a.out\n").is_empty());
    }

    #[test]
    fn makefile_continuations_join_and_a_second_use_counts() {
        let head = "CFLAGS = -O2 \\\n    -Wno-error \\\n    -Wall\n";
        let got = extract(BuildKind::Make, head).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].flag.as_str(), got[0].line), ("-Wno-error", 1));
        // A second `-w` in a context that already had one is new.
        let base = "CFLAGS = -w\n";
        assert_eq!(
            changes(BuildKind::Make, base, "CFLAGS = -w\nCFLAGS += -w\n"),
            vec![c("CFLAGS", "-w", true)]
        );
    }

    // ---- CMake ----

    #[test]
    fn cmake_flags_in_variables_and_options_commands() {
        let base = "set(CMAKE_C_FLAGS \"${CMAKE_C_FLAGS} -Wall -Werror\")\n";
        let head = "set(CMAKE_C_FLAGS \"${CMAKE_C_FLAGS} -Wall\")\nset(CMAKE_CXX_FLAGS \"${CMAKE_CXX_FLAGS} -Wno-error\" CACHE STRING \"\")\n\
                    string(APPEND CMAKE_CXX_FLAGS_RELEASE \" -w\")\nadd_compile_options(-Wno-unused -Wall)\n\
                    target_compile_options(app PRIVATE -Wno-deprecated-declarations)\n";
        assert_eq!(
            changes(BuildKind::CMake, base, head),
            vec![
                c("CMAKE_CXX_FLAGS", "-Wno-error", true),
                c("CMAKE_CXX_FLAGS_RELEASE", "-w", true),
                c("add_compile_options", "-Wno-unused", true),
                c(
                    "target_compile_options:app",
                    "-Wno-deprecated-declarations",
                    true
                ),
                c("CMAKE_C_FLAGS", "-Werror", false),
            ]
        );
        // `add_compile_options(-Wno-unused)` alone, unquoted separate arguments, lower-case command.
        assert_eq!(
            changes(
                BuildKind::CMake,
                "",
                "ADD_COMPILE_OPTIONS(-Wno-unused)\nset(CMAKE_C_FLAGS ${CMAKE_C_FLAGS} -w)\n"
            ),
            vec![
                c("add_compile_options", "-Wno-unused", true),
                c("CMAKE_C_FLAGS", "-w", true)
            ]
        );
    }

    #[test]
    fn cmake_negative_controls() {
        let base = "add_compile_options(-w)\n";
        // Unchanged, and reformatted across lines.
        assert!(changes(
            BuildKind::CMake,
            base,
            "add_compile_options(\n  -w\n)\nadd_compile_options(-Wall -Werror)\n"
        )
        .is_empty());
        // Comments (line and bracket), a variable that is not a flag variable, a generator
        // expression (documented as not read), and an unrelated command.
        let quiet = "# add_compile_options(-Wno-error)\n#[[ set(CMAKE_C_FLAGS -w) ]]\n\
                     set(MY_FLAGS -w)\nadd_compile_options($<$<CXX_COMPILER_ID:GNU>:-Wno-unused>)\n\
                     message(STATUS \"-Wno-error\")\nadd_definitions(-DFOO)\n";
        assert!(
            flags(BuildKind::CMake, quiet).is_empty(),
            "{:?}",
            flags(BuildKind::CMake, quiet)
        );
        // A quoted list of flags splits into words.
        assert_eq!(
            flags(
                BuildKind::CMake,
                "add_compile_options(\"-Wall -Wno-error\")\n"
            ),
            vec![
                ("add_compile_options".to_string(), "-Wall".to_string(), true),
                (
                    "add_compile_options".to_string(),
                    "-Wno-error".to_string(),
                    false
                ),
            ]
        );
        // An unterminated command cannot be read.
        assert_eq!(
            extract(BuildKind::CMake, "add_compile_options(-w\n").unwrap_err(),
            ExtractError::Unparsed
        );
    }

    // ---- setup.py ----

    #[cfg(feature = "lang-python")]
    #[test]
    fn setup_py_extra_compile_args() {
        let base = "from setuptools import Extension, setup\next = Extension('a', ['a.c'], extra_compile_args=['-O2', '-Werror'])\nsetup(ext_modules=[ext])\n";
        let head = "from setuptools import Extension, setup\next = Extension('a', ['a.c'], extra_compile_args=['-O2', '-Wno-error'])\nsetup(ext_modules=[ext])\n";
        assert_eq!(
            changes(BuildKind::SetupPy, base, head),
            vec![
                c("extra_compile_args", "-Wno-error", true),
                c("extra_compile_args", "-Werror", false),
            ]
        );
        // A variable, an augmented assignment and a per-compiler dict are read.
        let more = "extra_compile_args = ['-O2']\nextra_compile_args += ['-w']\nopts = {'extra_compile_args': ['-Wno-unused']}\n";
        assert_eq!(
            changes(BuildKind::SetupPy, "", more),
            vec![
                c("extra_compile_args", "-w", true),
                c("extra_compile_args", "-Wno-unused", true),
            ]
        );
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn setup_py_negative_controls() {
        let base = "e = Extension('a', ['a.c'], extra_compile_args=['-w'])\n";
        // Unchanged, and a strict flag added.
        assert!(changes(
            BuildKind::SetupPy,
            base,
            "e = Extension('a', ['a.c'], extra_compile_args=['-w', '-Wall'])\n"
        )
        .is_empty());
        // A flag in a comment, in a docstring, in another argument, and in another variable.
        let quiet = "# extra_compile_args=['-w']\n\"\"\"extra_compile_args=['-Wno-error']\"\"\"\n\
                     e = Extension('a', ['a.c'], extra_link_args=['-w'], define_macros=[('X', '-w')])\nother = ['-w']\n";
        assert!(
            flags(BuildKind::SetupPy, quiet).is_empty(),
            "{:?}",
            flags(BuildKind::SetupPy, quiet)
        );
        // Python 2 syntax is not readable.
        assert_eq!(
            extract(BuildKind::SetupPy, "def f(:\nextra_compile_args=['-w']\n").unwrap_err(),
            ExtractError::Unparsed
        );
    }

    // ---- build.rs ----

    #[cfg(feature = "lang-rust")]
    #[test]
    fn build_rs_cc_builder_flags() {
        let base = "fn main() {\n    cc::Build::new().file(\"a.c\").flag(\"-Werror\").warnings_into_errors(true).compile(\"a\");\n}\n";
        let head = "fn main() {\n    cc::Build::new()\n        .file(\"a.c\")\n        .flag(\"-Wno-error\")\n        .flag_if_supported(\"-w\")\n        .warnings(false)\n        .extra_warnings(false)\n        .warnings_into_errors(false)\n        .compile(\"a\");\n}\n";
        assert_eq!(
            changes(BuildKind::BuildRs, base, head),
            vec![
                c("cc::Build", "-Wno-error", true),
                c("cc::Build", "-w", true),
                c("cc::Build", ".warnings(false)", true),
                c("cc::Build", ".extra_warnings(false)", true),
                c("cc::Build", ".warnings_into_errors(false)", true),
                c("cc::Build", "-Werror", false),
                c("cc::Build", ".warnings_into_errors(true)", false),
            ]
        );
        let got = extract(BuildKind::BuildRs, head).unwrap();
        assert_eq!(got[0].line, 4);
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn build_rs_negative_controls() {
        let base = "fn main() { cc::Build::new().flag(\"-w\").compile(\"a\"); }\n";
        // Unchanged; a strict flag added; `.warnings(true)` is the default.
        assert!(changes(BuildKind::BuildRs, base, "fn main() { cc::Build::new().flag(\"-w\").flag(\"-Wall\").warnings(true).compile(\"a\"); }\n").is_empty());
        // A flag in a comment, in a string that is not passed to `.flag`, built at runtime, or a `println!`.
        let quiet = "fn main() {\n    // cc::Build::new().flag(\"-Wno-error\");\n    /* .warnings(false) */\n    let s = \"-w\";\n    println!(\"cargo:warning=-Wno-error\");\n    let f = format!(\"-Wno-{}\", \"x\");\n    cc::Build::new().flag(&f).define(\"-w\", None).file(\"-Wno-error\").compile(\"a\");\n}\n";
        assert!(
            flags(BuildKind::BuildRs, quiet).is_empty(),
            "{:?}",
            flags(BuildKind::BuildRs, quiet)
        );
        // Source that does not parse cannot be read.
        assert_eq!(
            extract(BuildKind::BuildRs, "fn main( {").unwrap_err(),
            ExtractError::Unparsed
        );
    }
}
