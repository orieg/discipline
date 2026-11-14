//! Invented names (`invented-names`): a flag or configuration key a document gives a tool
//! that the tool's declared surface does not list.
//!
//! Each `[[gates.invented-names.tools]]` entry names a tool and a tracked surface file,
//! one accepted name per line: a flag (`--name`, `-n`) or a configuration key
//! (`section.key`, `section.list[].field` for an entry of an array of tables). Blank
//! lines and `#` comments are skipped; any other line stops the run (exit 2), as does a
//! surface that is missing, untracked, a symlink, not UTF-8, or lists no name.
//!
//! Markdown documents are read with the line-based reader of `citation-anchors`
//! (fences, code spans and HTML comments, after CommonMark). Prose is never read:
//! - **Flags**: a code span, or a line of a shell fence (no info string, `sh`, `bash`,
//!   `shell`, `console`, `zsh`, `fish`, `shell-session`), split into commands at `|`,
//!   `||`, `&&`, `;` and `&`. A command whose program is the tool (its name, or a path
//!   ending in `/<name>`, after `VAR=value` assignments and `env`, `sudo`, `exec`,
//!   `time`) has each `--long` (value after `=` dropped) and single-letter `-x`
//!   argument checked, up to a `--`. Quoted words, grouped short flags (`-abc`) and
//!   placeholders (`--<flag>`) are not read. A line ending in `\` continues on the next.
//! - **Configuration keys**: a `toml` fence that is the tool's configuration: its info
//!   string or first comment names one of `config_files`, or the document matches
//!   `config_docs` and the block names no other `.toml` file. Each table header and
//!   each `key = value` (dotted keys joined) is checked as its full dotted path, an
//!   array-of-tables header adding `[]`. A name resolves when the surface lists it, or
//!   lists a name under it (`section` resolves against `section.key`).
//!
//! With `diff_only` (the default) only the lines the change adds are checked; a block's
//! table headers are read from the whole block either way.

use crate::config::{GateSettings, InventedNamesTool};
use crate::could_not_check::{tag, Reason};
use crate::guards::citation_anchors::links::{self, FencedBlock};
use crate::guards::{exempt_filter, Context, GateOutcome, PathFilter};
use anyhow::{anyhow, Result};
use std::collections::{BTreeMap, BTreeSet};

pub const GATE: &str = "invented-names";

/// Fence info words whose lines are shell commands.
const SHELL_INFOS: &[&str] = &[
    "",
    "sh",
    "bash",
    "shell",
    "console",
    "zsh",
    "fish",
    "shell-session",
];

/// Words that run the next word as the program.
const WRAPPERS: &[&str] = &["env", "sudo", "exec", "time", "command"];

/// The names one tool declares.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Surface {
    names: BTreeSet<String>,
}

impl Surface {
    /// Reads a surface file's text. `Err` names the first line that is neither a name,
    /// blank, nor a `#` comment, or says the file lists no name.
    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        let mut names = BTreeSet::new();
        for (idx, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let listed = if line.starts_with('-') {
                long_flag(line).is_some_and(|f| f == line) || short_flag(line).is_some()
            } else {
                is_config_key(line)
            };
            if listed {
                names.insert(line.to_string());
            } else {
                return Err(format!(
                    "line {} is neither a flag (`--name`, `-n`), a configuration key (`section.key`), blank, nor a `#` comment",
                    idx + 1
                ));
            }
        }
        if names.is_empty() {
            return Err("it lists no name".into());
        }
        Ok(Self { names })
    }

    /// Whether `name` is listed, or a listed name lies under it.
    pub fn resolves(&self, name: &str) -> bool {
        if self.names.contains(name) {
            return true;
        }
        let dotted = format!("{name}.");
        let entry = format!("{name}[].");
        self.names
            .range(name.to_string()..)
            .take_while(|n| n.starts_with(name))
            .any(|n| n.starts_with(&dotted) || n.starts_with(&entry))
    }
}

/// `--name` from a `--name` or `--name=value` word, when `name` is a flag name.
fn long_flag(word: &str) -> Option<&str> {
    let flag = word.split('=').next().unwrap_or(word);
    let name = flag.strip_prefix("--")?;
    let first = name.chars().next()?;
    (first.is_ascii_alphanumeric()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    .then_some(flag)
}

/// `-x` when the word is exactly one dash and one letter or digit.
fn short_flag(word: &str) -> Option<&str> {
    let rest = word.strip_prefix('-')?;
    let mut chars = rest.chars();
    let c = chars.next()?;
    (c.is_ascii_alphanumeric() && chars.next().is_none()).then_some(word)
}

/// A bare TOML key segment: letters, digits, `-` and `_`.
fn is_bare_segment(seg: &str) -> bool {
    !seg.is_empty()
        && seg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `section.key` and `list[].field` forms: bare segments, each optionally ending in `[]`.
fn is_config_key(name: &str) -> bool {
    name.split('.')
        .all(|seg| is_bare_segment(seg.strip_suffix("[]").unwrap_or(seg)))
}

/// A word of a shell line, with the 1-based line it is on.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Word {
    text: String,
    /// The word was quoted in whole or part: a value, never a flag.
    quoted: bool,
    line: usize,
}

/// Splits shell text into words and control operators (`|`, `&&`, `;`, ...). A `#`
/// that starts a word starts a comment to the end of the line.
fn shell_words(parts: &[(usize, &str)]) -> Vec<Word> {
    let mut words = Vec::new();
    for (line, text) in parts {
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        let mut cur = String::new();
        let mut quoted = false;
        let mut in_word = false;
        let flush =
            |cur: &mut String, quoted: &mut bool, in_word: &mut bool, words: &mut Vec<Word>| {
                if *in_word {
                    words.push(Word {
                        text: std::mem::take(cur),
                        quoted: *quoted,
                        line: *line,
                    });
                }
                *quoted = false;
                *in_word = false;
            };
        while i < chars.len() {
            let c = chars[i];
            match c {
                ' ' | '\t' => {
                    flush(&mut cur, &mut quoted, &mut in_word, &mut words);
                    i += 1;
                }
                '#' if !in_word => break,
                '|' | '&' | ';' => {
                    flush(&mut cur, &mut quoted, &mut in_word, &mut words);
                    let doubled = chars.get(i + 1) == Some(&c) && c != ';';
                    words.push(Word {
                        text: if doubled {
                            format!("{c}{c}")
                        } else {
                            c.to_string()
                        },
                        quoted: false,
                        line: *line,
                    });
                    i += if doubled { 2 } else { 1 };
                }
                '\'' | '"' => {
                    in_word = true;
                    quoted = true;
                    let close = chars[i + 1..].iter().position(|q| *q == c);
                    match close {
                        Some(off) => {
                            cur.extend(&chars[i + 1..i + 1 + off]);
                            i += off + 2;
                        }
                        None => {
                            cur.extend(&chars[i + 1..]);
                            i = chars.len();
                        }
                    }
                }
                '\\' if i + 1 < chars.len() => {
                    in_word = true;
                    quoted = true;
                    cur.push(chars[i + 1]);
                    i += 2;
                }
                _ => {
                    in_word = true;
                    cur.push(c);
                    i += 1;
                }
            }
        }
        flush(&mut cur, &mut quoted, &mut in_word, &mut words);
    }
    words
}

/// Whether a word is a control operator that ends one command.
fn is_operator(word: &Word) -> bool {
    !word.quoted && matches!(word.text.as_str(), "|" | "||" | "&&" | ";" | "&")
}

/// Whether a program word runs `tool`: the bare name, or a path ending in `/<tool>`.
fn runs_tool(program: &str, tool: &str) -> bool {
    program == tool
        || program
            .rsplit_once('/')
            .is_some_and(|(_, base)| base == tool)
}

/// Every flag word of `tool`'s commands in a shell text: `(line, flag)`.
fn tool_flags(parts: &[(usize, &str)], tool: &str) -> Vec<(usize, String)> {
    let words = shell_words(parts);
    let mut out = Vec::new();
    for command in words.split(is_operator) {
        let mut rest = command.iter().skip_while(|w| {
            !w.quoted
                && (WRAPPERS.contains(&w.text.as_str())
                    || w.text.split_once('=').is_some_and(|(var, _)| {
                        !var.is_empty()
                            && var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    }))
        });
        let Some(program) = rest.next() else {
            continue;
        };
        if program.quoted || !runs_tool(&program.text, tool) {
            continue;
        }
        for word in rest {
            if word.quoted {
                continue;
            }
            if word.text == "--" {
                break;
            }
            if let Some(flag) = long_flag(&word.text).or_else(|| short_flag(&word.text)) {
                out.push((word.line, flag.to_string()));
            }
        }
    }
    out
}

/// The commands of a shell fence: physical lines joined at a trailing `\`, a leading
/// `$ ` or `% ` prompt removed. Each command keeps its parts' line numbers.
fn shell_commands(block: &FencedBlock) -> Vec<Vec<(usize, String)>> {
    let mut commands = Vec::new();
    let mut current: Vec<(usize, String)> = Vec::new();
    for (line, raw) in &block.lines {
        let mut text = raw.trim();
        if current.is_empty() {
            text = text
                .strip_prefix("$ ")
                .or_else(|| text.strip_prefix("% "))
                .unwrap_or(text);
        }
        match text.strip_suffix('\\') {
            Some(head) => current.push((*line, head.to_string())),
            None => {
                current.push((*line, text.to_string()));
                commands.push(std::mem::take(&mut current));
            }
        }
    }
    if !current.is_empty() {
        commands.push(current);
    }
    commands
}

/// The first word of a fence's info string, lowercased.
fn info_language(info: &str) -> String {
    info.split(|c: char| c.is_whitespace() || c == '{' || c == ',')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// `.toml` file names a block names: in its info string (`toml mytool.toml`,
/// `title="mytool.toml"`) or in its first non-blank line when that is a comment.
fn named_toml_files(block: &FencedBlock) -> Vec<String> {
    let mut text = block.info.clone();
    if let Some((_, first)) = block.lines.iter().find(|(_, l)| !l.trim().is_empty()) {
        if let Some(comment) = first.trim().strip_prefix('#') {
            text.push(' ');
            text.push_str(comment);
        }
    }
    text.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '=' | '`' | ','))
        .filter(|w| w.len() > ".toml".len() && w.to_ascii_lowercase().ends_with(".toml"))
        .map(|w| w.rsplit('/').next().unwrap_or(w).to_string())
        .collect()
}

/// A bare or quoted dotted TOML key as its segments; `None` for anything else (a
/// placeholder, a quoted segment holding a dot).
fn key_segments(text: &str) -> Option<Vec<String>> {
    let mut segs = Vec::new();
    for raw in text.split('.') {
        let seg = raw.trim();
        let seg = seg
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| seg.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
            .unwrap_or(seg);
        if !is_bare_segment(seg) {
            return None;
        }
        segs.push(seg.to_string());
    }
    Some(segs)
}

/// Where a multi-line TOML value is still open at the end of a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct ValueState {
    /// Unclosed `[` / `{`.
    depth: usize,
    /// Inside a `"""` or `'''` string.
    multiline: Option<char>,
}

impl ValueState {
    fn open(&self) -> bool {
        self.depth > 0 || self.multiline.is_some()
    }

    /// Reads one line of a value, carrying brackets and multi-line strings over.
    fn scan(&mut self, text: &str) {
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if let Some(q) = self.multiline {
                if chars[i] == q && chars.get(i + 1) == Some(&q) && chars.get(i + 2) == Some(&q) {
                    self.multiline = None;
                    i += 3;
                } else {
                    i += if q == '"' && chars[i] == '\\' { 2 } else { 1 };
                }
                continue;
            }
            match chars[i] {
                '#' => break,
                q @ ('"' | '\'') => {
                    if chars.get(i + 1) == Some(&q) && chars.get(i + 2) == Some(&q) {
                        self.multiline = Some(q);
                        i += 3;
                        continue;
                    }
                    i += 1;
                    while i < chars.len() && chars[i] != q {
                        i += if q == '"' && chars[i] == '\\' { 2 } else { 1 };
                    }
                    i += 1;
                }
                '[' | '{' => {
                    self.depth += 1;
                    i += 1;
                }
                ']' | '}' => {
                    self.depth = self.depth.saturating_sub(1);
                    i += 1;
                }
                _ => i += 1,
            }
        }
    }
}

/// The index of the first `=` outside quotes, if any.
fn assignment(text: &str) -> Option<usize> {
    let mut quote = None;
    for (i, c) in text.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), _) if c == q => quote = None,
            (None, '=') => return Some(i),
            (None, '#') => return None,
            _ => {}
        }
    }
    None
}

/// Every table header and key of a TOML block as a full dotted name: `(line, name)`.
fn toml_names(block: &FencedBlock) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut table = String::new();
    let mut arrays = BTreeSet::new();
    let mut state = ValueState::default();
    for (line, raw) in &block.lines {
        if state.open() {
            state.scan(raw);
            continue;
        }
        let text = raw.trim();
        if text.is_empty() || text.starts_with('#') {
            continue;
        }
        if text.starts_with('[') {
            let array = text.starts_with("[[");
            let inner = if array {
                text.strip_prefix("[[")
                    .and_then(|r| r.split_once("]]"))
                    .map(|(h, _)| h)
            } else {
                text.strip_prefix('[')
                    .and_then(|r| r.split_once(']'))
                    .map(|(h, _)| h)
            };
            table.clear();
            if let Some(segs) = inner.and_then(key_segments) {
                // A parent that is an array of tables (`[[a.groups]]` before
                // `[[a.groups.sources]]`) is an entry of it: `a.groups[].sources`.
                let mut name = String::new();
                for (i, seg) in segs.iter().enumerate() {
                    if i > 0 {
                        if arrays.contains(&segs[..i].join(".")) {
                            name.push_str("[]");
                        }
                        name.push('.');
                    }
                    name.push_str(seg);
                }
                out.push((*line, name.clone()));
                table = name;
                if array {
                    arrays.insert(segs.join("."));
                    table.push_str("[]");
                }
            }
            continue;
        }
        let Some(eq) = assignment(text) else {
            continue;
        };
        state.scan(&text[eq + 1..]);
        if let Some(segs) = key_segments(&text[..eq]) {
            let key = segs.join(".");
            out.push((
                *line,
                if table.is_empty() {
                    key
                } else {
                    format!("{table}.{key}")
                },
            ));
        }
    }
    out
}

/// What one document gives one tool: `(line, flag)` and `(line, key)` pairs.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DocNames {
    pub flags: Vec<(usize, String)>,
    pub keys: Vec<(usize, String)>,
}

/// Reads one Markdown document for one tool's flags and keys. `config_doc` says the
/// document matches the tool's `config_docs`.
pub fn doc_names(content: &str, tool: &InventedNamesTool, config_doc: bool) -> DocNames {
    let text = links::read_markdown(content);
    let mut names = DocNames::default();
    for span in &text.code_spans {
        names.flags.extend(tool_flags(
            &[(span.line_num, span.target.as_str())],
            &tool.name,
        ));
    }
    for block in &text.fences {
        let language = info_language(&block.info);
        if SHELL_INFOS.contains(&language.as_str()) {
            for command in shell_commands(block) {
                let parts: Vec<(usize, &str)> =
                    command.iter().map(|(l, t)| (*l, t.as_str())).collect();
                names.flags.extend(tool_flags(&parts, &tool.name));
            }
        } else if language == "toml" {
            let named = named_toml_files(block);
            let is_config = if named.is_empty() {
                config_doc
            } else {
                named.iter().any(|n| tool.config_files.contains(n))
            };
            if is_config {
                names.keys.extend(toml_names(block));
            }
        }
    }
    names.flags.sort();
    names.keys.sort();
    names
}

/// A tool entry checked and compiled.
struct Tool<'a> {
    config: &'a InventedNamesTool,
    surface: Surface,
    config_docs: PathFilter,
}

/// Refuses an entry that could not be read: a blank or duplicate name, a name holding
/// whitespace or `/`, a surface path that is not repository-relative, a blank
/// `config_files` entry, a `config_docs` glob that does not compile.
fn check_entry(tool: &InventedNamesTool, index: usize) -> Result<PathFilter> {
    let key = |field: &str| format!("gates.{GATE}.tools[{index}].{field}");
    let config_err = |msg: String| tag(Reason::Configuration, anyhow!(msg));
    if tool.name.trim().is_empty() || tool.name.contains(|c: char| c.is_whitespace() || c == '/') {
        return Err(config_err(format!(
            "`{}` must be a command name: not blank, no whitespace, no `/`",
            key("name")
        )));
    }
    let surface = tool.surface.trim();
    if surface.is_empty()
        || surface.starts_with('/')
        || surface.split(['/', '\\']).any(|seg| seg == "..")
    {
        return Err(config_err(format!(
            "`{}` must be a repository-relative path inside the repository",
            key("surface")
        )));
    }
    if tool.config_files.iter().any(|f| f.trim().is_empty()) {
        return Err(config_err(format!(
            "`{}` holds a blank entry",
            key("config_files")
        )));
    }
    PathFilter::new(&tool.config_docs)
        .map_err(|e| config_err(format!("`{}`: {e:#}", key("config_docs"))))
}

/// Reads a tool's surface from the head side, failing closed (reason `gate`) on a file
/// that is missing, untracked, a symlink, not UTF-8, or not a list of names.
fn read_surface(ctx: &Context, path: &str) -> Result<Surface> {
    let gate_err = |msg: String| tag(Reason::Gate, anyhow!(msg));
    if ctx.git.is_symlink(path)? {
        return Err(gate_err(format!(
            "the surface `{path}` is a symlink; name the file it points to"
        )));
    }
    if !ctx.git.is_tracked(path)? {
        return Err(gate_err(format!(
            "the surface `{path}` is not a tracked file: the list of names is reviewed content"
        )));
    }
    let Some(bytes) = ctx.git.head_bytes(path)? else {
        return Err(gate_err(format!("the surface `{path}` could not be read")));
    };
    let text = String::from_utf8(bytes)
        .map_err(|_| gate_err(format!("the surface `{path}` is not UTF-8")))?;
    Surface::parse(&text).map_err(|why| gate_err(format!("the surface `{path}`: {why}")))
}

pub fn evaluate_invented_names(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.invented_names;
    let mut out = GateOutcome::new(GATE);
    if settings.tools.is_empty() {
        return Err(tag(
            Reason::Configuration,
            anyhow!("`gates.{GATE}` is enabled but no tools are configured"),
        ));
    }
    let mut seen = BTreeSet::new();
    let mut tools = Vec::new();
    for (index, tool) in settings.tools.iter().enumerate() {
        let config_docs = check_entry(tool, index)?;
        if !seen.insert(tool.name.as_str()) {
            return Err(tag(
                Reason::Configuration,
                anyhow!(
                    "`gates.{GATE}.tools[{index}].name`: `{}` is configured twice",
                    tool.name
                ),
            ));
        }
        tools.push(Tool {
            config: tool,
            surface: read_surface(ctx, tool.surface.trim())?,
            config_docs,
        });
    }
    let exempt = exempt_filter(settings)?;

    // Each Markdown document with the lines read: the added ones under `diff_only`.
    let docs: BTreeMap<String, Option<BTreeSet<usize>>> = if settings.diff_only {
        ctx.git
            .changed_files()?
            .into_iter()
            .filter(|f| !f.is_deleted())
            .map(|f| (f.path, Some(f.added_lines)))
            .collect()
    } else {
        ctx.git
            .tracked_files()?
            .into_iter()
            .map(|p| (p, None))
            .collect()
    };
    let severity = ctx.overridable(settings.severity());
    for (doc, lines) in &docs {
        if !links::is_markdown_path(doc) || exempt.matches(doc) {
            continue;
        }
        let Some(content) = ctx.git.head_content(doc)? else {
            continue;
        };
        out.examined += 1;
        let read = |line: &usize| lines.as_ref().is_none_or(|l| l.contains(line));
        for tool in &tools {
            let names = doc_names(&content, tool.config, tool.config_docs.matches(doc));
            let name = &tool.config.name;
            let surface = &tool.config.surface;
            for (line, flag) in names.flags.iter().filter(|(l, _)| read(l)) {
                if !tool.surface.resolves(flag) {
                    out.add_violation(
                        severity,
                        &crate::findings::UNKNOWN_FLAG,
                        doc,
                        *line,
                        format!("`{name} {flag}`: `{surface}` does not list the flag `{flag}`"),
                        format!(
                            "Write a flag `{name}` accepts, or add `{flag}` to `{surface}` if the tool now has it."
                        ),
                    );
                }
            }
            for (line, key) in names.keys.iter().filter(|(l, _)| read(l)) {
                if !tool.surface.resolves(key) {
                    out.add_violation(
                        severity,
                        &crate::findings::UNKNOWN_CONFIG_KEY,
                        doc,
                        *line,
                        format!(
                            "configuration key `{key}` of `{name}`: `{surface}` does not list it"
                        ),
                        format!(
                            "Write a key `{name}` accepts, or add `{key}` to `{surface}` if the tool now has it."
                        ),
                    );
                }
            }
        }
    }
    if out.examined == 0 {
        out.notes.push(if settings.diff_only {
            "the change adds no line to a Markdown document outside `exempt_paths`".into()
        } else {
            "no tracked Markdown document outside `exempt_paths`".into()
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, config_files: &[&str]) -> InventedNamesTool {
        InventedNamesTool {
            name: name.into(),
            surface: "docs/surface.txt".into(),
            config_files: config_files.iter().map(|s| s.to_string()).collect(),
            config_docs: Vec::new(),
        }
    }

    fn flags(md: &str) -> Vec<(usize, String)> {
        doc_names(md, &tool("mytool", &[]), false).flags
    }

    fn keys(md: &str, config_doc: bool) -> Vec<(usize, String)> {
        doc_names(md, &tool("mytool", &["mytool.toml"]), config_doc).keys
    }

    fn pairs(items: &[(usize, &str)]) -> Vec<(usize, String)> {
        items.iter().map(|(l, s)| (*l, s.to_string())).collect()
    }

    #[test]
    fn the_surface_lists_flags_and_keys_and_refuses_anything_else() {
        let s = Surface::parse(
            "# names\n--dry-run\n-n\n\ngates.pii.enabled\ngates.command.commands[].name\n",
        )
        .unwrap();
        assert!(s.resolves("--dry-run"));
        assert!(s.resolves("-n"));
        assert!(s.resolves("gates.pii.enabled"));
        // A name under a listed one resolves its parents.
        assert!(s.resolves("gates.pii"));
        assert!(s.resolves("gates.command.commands"));
        assert!(s.resolves("gates.command.commands[].name"));
        // Negative controls.
        assert!(!s.resolves("--dry"));
        assert!(!s.resolves("gates.pi"));
        assert!(!s.resolves("gates.pii.strict"));
        assert!(!s.resolves("-x"));
        for bad in [
            "--dry-run extra",
            "dry run",
            "--=x",
            "-abc",
            "gates..pii",
            "--flag=value",
        ] {
            assert!(Surface::parse(bad).is_err(), "{bad} should be refused");
        }
        assert!(Surface::parse("# only a comment\n\n").is_err());
    }

    #[test]
    fn flags_are_read_from_code_spans_and_shell_fences_of_the_tool_only() {
        let md = "\
Run `mytool check --base main -q` or `other --nope`.
In prose --invented is not read, nor `--alone`.

```bash
$ mytool run --fast \\
    --out=x.json -- --after
FOO=1 env ./bin/mytool scan --deep | grep --count x && mytool -v
mytool \"--quoted\" -abc --<flag> # mytool --comment
```

```rust
mytool --in-rust
```

<!-- `mytool --in-comment` -->
";
        assert_eq!(
            flags(md),
            pairs(&[
                (1, "--base"),
                (1, "-q"),
                (5, "--fast"),
                (6, "--out"),
                (7, "--deep"),
                (7, "-v"),
            ])
        );
    }

    #[test]
    fn toml_blocks_are_read_when_marked_as_the_tools_configuration() {
        let marked = "\
```toml mytool.toml
[gates.pii]
enabled = true
diff_only.strict = false
exempt_paths = [
  \"a\",
  \"b = c\",
]
note = \"\"\"
fake = 1
\"\"\"

[[gates.command.commands]]
name = \"x\" # trailing = comment
<key> = 1
```
";
        assert_eq!(
            keys(marked, false),
            pairs(&[
                (2, "gates.pii"),
                (3, "gates.pii.enabled"),
                (4, "gates.pii.diff_only.strict"),
                (5, "gates.pii.exempt_paths"),
                (9, "gates.pii.note"),
                (13, "gates.command.commands"),
                (14, "gates.command.commands[].name"),
            ])
        );
        let nested = "```toml mytool.toml\n[[a.groups]]\nname = \"g\"\n[[a.groups.sources]]\npath = \"p\"\n```\n";
        assert_eq!(
            keys(nested, false),
            pairs(&[
                (2, "a.groups"),
                (3, "a.groups[].name"),
                (4, "a.groups[].sources"),
                (5, "a.groups[].sources[].path"),
            ])
        );
        let commented = "```toml\n# mytool.toml\n[meta]\nname = \"x\"\n```\n";
        assert_eq!(
            keys(commented, false),
            pairs(&[(3, "meta"), (4, "meta.name")])
        );
        let unmarked = "```toml\n[meta]\nname = \"x\"\n```\n";
        // An unmarked block is read only in a `config_docs` document.
        assert!(keys(unmarked, false).is_empty());
        assert_eq!(keys(unmarked, true).len(), 2);
        // A block naming another file is never the tool's.
        let other = "```toml\n# Cargo.toml\n[package]\nname = \"x\"\n```\n";
        assert!(keys(other, true).is_empty());
        let titled = "```toml title=\"pyproject.toml\"\n[tool.x]\na = 1\n```\n";
        assert!(keys(titled, true).is_empty());
    }

    #[test]
    fn an_entry_that_cannot_be_read_is_a_configuration_error() {
        let mut t = tool("mytool", &[]);
        assert!(check_entry(&t, 0).is_ok());
        for name in ["", " ", "my tool", "bin/mytool"] {
            t.name = name.into();
            assert!(check_entry(&t, 0).is_err(), "name {name:?}");
        }
        t = tool("mytool", &[]);
        for surface in ["", "/etc/x", "../x", "docs/../../x"] {
            t.surface = surface.into();
            assert!(check_entry(&t, 0).is_err(), "surface {surface:?}");
        }
        t = tool("mytool", &[" "]);
        assert!(check_entry(&t, 0).is_err());
        t = tool("mytool", &[]);
        t.config_docs = vec!["docs/[".into()];
        assert!(check_entry(&t, 0).is_err());
    }
}
