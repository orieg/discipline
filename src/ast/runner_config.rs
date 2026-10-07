//! Runner configuration that is written as code or as a command line, read only where
//! it is a literal: a Vitest configuration file whose default export is an object
//! literal, the words of a `package.json` script that runs Jest, and the
//! `collect_ignore` lists of a `conftest.py`. Anything computed is reported as such and
//! never guessed at: the caller counts the files, with a note.

use tree_sitter::Node;

/// What a `vitest.config.*` or `vite.config.*` file says about collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VitestConfig {
    /// A literal object with no `test` property.
    NoTestBlock,
    Literal(VitestLiteral),
    /// The export, the `test` block, or a key that decides collection is computed.
    Dynamic,
}

/// The collection keys of a literal Vitest `test` block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VitestLiteral {
    pub include: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
    /// `test.dir`, else `test.root`, else the top-level `root`: where the patterns
    /// resolve.
    pub base: Option<String>,
}

/// Keys of a `test` block that move collection in a way that is not read.
const VITEST_UNREAD_KEYS: &[&str] = &["projects", "workspace", "includeSource", "typecheck"];

fn text<'a>(node: Node, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

/// The value of a JavaScript string literal with no escape sequence in it.
fn js_string(node: Node, src: &[u8]) -> Option<String> {
    if node.kind() != "string" {
        return None;
    }
    let mut cursor = node.walk();
    let mut value = String::new();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "string_fragment" {
            return None;
        }
        value.push_str(text(child, src));
    }
    Some(value)
}

fn js_string_array(node: Node, src: &[u8]) -> Option<Vec<String>> {
    if node.kind() != "array" {
        return None;
    }
    let mut cursor = node.walk();
    let mut out = Vec::new();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "comment" {
            continue;
        }
        out.push(js_string(child, src)?);
    }
    Some(out)
}

/// The `key: value` pairs of an object literal, or `None` when it holds anything else:
/// a spread, a shorthand property, a method, a computed key.
fn js_object_pairs<'a>(node: Node<'a>, src: &[u8]) -> Option<Vec<(String, Node<'a>)>> {
    if node.kind() != "object" {
        return None;
    }
    let mut cursor = node.walk();
    let mut out = Vec::new();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "comment" => {}
            "pair" => {
                let key = child.child_by_field_name("key")?;
                let name = match key.kind() {
                    "property_identifier" => text(key, src).to_string(),
                    "string" => js_string(key, src)?,
                    _ => return None,
                };
                out.push((name, child.child_by_field_name("value")?));
            }
            _ => return None,
        }
    }
    Some(out)
}

/// The expression a configuration file exports: `export default <expr>` or
/// `module.exports = <expr>`. `None` unless there is exactly one.
fn js_default_export<'a>(root: Node<'a>, src: &[u8]) -> Option<Node<'a>> {
    let mut cursor = root.walk();
    let mut found = None;
    for child in root.named_children(&mut cursor) {
        let value = match child.kind() {
            "export_statement" => {
                let mut inner = child.walk();
                let is_default = child.children(&mut inner).any(|c| c.kind() == "default");
                if !is_default {
                    continue;
                }
                child.child_by_field_name("value")
            }
            "expression_statement" => {
                let Some(assignment) = child
                    .named_child(0)
                    .filter(|n| n.kind() == "assignment_expression")
                else {
                    continue;
                };
                let left = assignment.child_by_field_name("left")?;
                if text(left, src) != "module.exports" {
                    continue;
                }
                assignment.child_by_field_name("right")
            }
            _ => continue,
        };
        if found.is_some() {
            return None;
        }
        found = Some(value?);
    }
    found
}

/// Reads a Vitest or Vite configuration file. Only the literal case is read:
/// `export default defineConfig({ test: { include: ['...'], exclude: ['...'] } })`, the
/// same without `defineConfig`, or `module.exports = { ... }`, with `include` and
/// `exclude` arrays of string literals and `root` / `dir` string literals.
pub fn parse_vitest_config(file_name: &str, source: &str) -> VitestConfig {
    let typescript = [".ts", ".mts", ".cts"]
        .iter()
        .any(|ext| file_name.ends_with(ext));
    let language: tree_sitter::Language = if typescript {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
    } else {
        tree_sitter_javascript::LANGUAGE.into()
    };
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language).is_err() {
        return VitestConfig::Dynamic;
    }
    let Ok(tree) = crate::ast::source_text::parse(&mut parser, source) else {
        return VitestConfig::Dynamic;
    };
    let root = tree.root_node();
    if root.has_error() {
        return VitestConfig::Dynamic;
    }
    read_vitest_export(root, source.as_bytes()).unwrap_or(VitestConfig::Dynamic)
}

fn read_vitest_export(root: Node, src: &[u8]) -> Option<VitestConfig> {
    let mut export = js_default_export(root, src)?;
    if export.kind() == "call_expression" {
        let function = export.child_by_field_name("function")?;
        if function.kind() != "identifier" || text(function, src) != "defineConfig" {
            return None;
        }
        let arguments = export.child_by_field_name("arguments")?;
        if arguments.named_child_count() != 1 {
            return None;
        }
        export = arguments.named_child(0)?;
    }
    let top = js_object_pairs(export, src)?;
    let mut literal = VitestLiteral::default();
    let mut top_root = None;
    let mut test = None;
    for (key, value) in &top {
        match key.as_str() {
            "test" => test = Some(*value),
            "root" => top_root = Some(js_string(*value, src)?),
            _ => {}
        }
    }
    let Some(test) = test else {
        return Some(VitestConfig::NoTestBlock);
    };
    let mut dir = None;
    let mut test_root = None;
    for (key, value) in js_object_pairs(test, src)? {
        match key.as_str() {
            "include" => literal.include = Some(js_string_array(value, src)?),
            "exclude" => literal.exclude = Some(js_string_array(value, src)?),
            "dir" => dir = Some(js_string(value, src)?),
            "root" => test_root = Some(js_string(value, src)?),
            other if VITEST_UNREAD_KEYS.contains(&other) => return None,
            _ => {}
        }
    }
    literal.base = dir.or(test_root).or(top_root);
    Some(VitestConfig::Literal(literal))
}

/// How the `package.json` scripts run Jest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JestScripts {
    /// `--config` / `-c` of the invocation that decides.
    pub config: Option<String>,
    /// `--rootDir` of the invocation that decides.
    pub root_dir: Option<String>,
    /// Another script runs Jest with a different configuration, or in a way that is
    /// not read: a file the deciding configuration leaves out may be that one's.
    pub other: bool,
    /// The deciding invocation cannot be read, with the kind of problem.
    pub unreadable: Option<&'static str>,
}

const JEST_COLLECTION_FLAGS: &[&str] = &[
    "--config",
    "--rootDir",
    "--roots",
    "--projects",
    "--selectProjects",
    "--ignoreProjects",
    "--testMatch",
    "--testRegex",
    "--testPathPattern",
    "--testPathPatterns",
    "--testPathIgnorePatterns",
    "--preset",
    "--runTestsByPath",
    "--findRelatedTests",
];

const SCRIPT_NOT_SIMPLE: &str =
    "a package script passes Jest a configuration flag in a command that is not a simple one";
const SCRIPT_UNREAD_FLAG: &str = "a package script passes Jest a flag that moves collection";
const SCRIPT_SEVERAL: &str = "package scripts run Jest with more than one configuration";

fn is_jest_word(word: &str) -> bool {
    let word = word.trim_matches(['"', '\'']);
    matches!(word.rsplit('/').next(), Some("jest" | "jest.js"))
}

/// Whether a word is, or starts, a flag that says where Jest collects.
fn is_collection_flag(word: &str) -> bool {
    let name = word.split('=').next().unwrap_or(word);
    name == "-c" || JEST_COLLECTION_FLAGS.contains(&name)
}

/// Splits a simple command into words, with single and double quotes. `None` when a
/// quote is not closed.
fn shell_words(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    for c in command.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => current.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                in_word = true;
            }
            None if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            None => {
                current.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if in_word {
        words.push(current);
    }
    Some(words)
}

/// `--config` and `--rootDir` of one Jest invocation.
type JestInvocation = (Option<String>, Option<String>);

/// One script's Jest invocation: `Ok(None)` when it does not run Jest.
fn read_jest_script(script: &str) -> Result<Option<JestInvocation>, &'static str> {
    if !script.split_whitespace().any(is_jest_word) {
        return Ok(None);
    }
    let not_simple = script.contains(['|', '&', ';', '$', '`', '(', ')', '<', '>', '\n', '\\']);
    if not_simple {
        // A command list is not split into its commands. It is left alone when no
        // word in it could carry a configuration to Jest.
        let carries =
            script.contains(['$', '`', '\\']) || script.split_whitespace().any(is_collection_flag);
        return if carries {
            Err(SCRIPT_NOT_SIMPLE)
        } else {
            Ok(Some((None, None)))
        };
    }
    let words = shell_words(script).ok_or(SCRIPT_NOT_SIMPLE)?;
    let start = words
        .iter()
        .position(|w| is_jest_word(w))
        .map_or(words.len(), |i| i + 1);
    let mut config = None;
    let mut root_dir = None;
    let mut rest = words[start..].iter();
    while let Some(word) = rest.next() {
        let (name, inline) = match word.split_once('=') {
            Some((name, value)) => (name, Some(value.to_string())),
            None => (word.as_str(), None),
        };
        match name {
            "--config" | "-c" => {
                config = Some(
                    inline
                        .or_else(|| rest.next().cloned())
                        .ok_or(SCRIPT_UNREAD_FLAG)?,
                );
            }
            "--rootDir" => {
                root_dir = Some(
                    inline
                        .or_else(|| rest.next().cloned())
                        .ok_or(SCRIPT_UNREAD_FLAG)?,
                );
            }
            other if is_collection_flag(other) => return Err(SCRIPT_UNREAD_FLAG),
            _ => {}
        }
    }
    Ok(Some((config, root_dir)))
}

/// Reads how the scripts of a parsed `package.json` run Jest. The `test` script
/// decides when it runs Jest; otherwise the scripts that do must agree.
pub fn read_jest_scripts(package: &serde_json::Value) -> JestScripts {
    let mut out = JestScripts::default();
    let Some(scripts) = package.get("scripts").and_then(|s| s.as_object()) else {
        return out;
    };
    let mut invocations = Vec::new();
    for (name, script) in scripts {
        let Some(script) = script.as_str() else {
            continue;
        };
        match read_jest_script(script) {
            Ok(None) => {}
            Ok(Some(invocation)) => invocations.push((name.as_str(), Ok(invocation))),
            Err(kind) => invocations.push((name.as_str(), Err(kind))),
        }
    }
    let primary = match invocations.iter().find(|(name, _)| *name == "test") {
        Some((_, primary)) => primary.clone(),
        None => {
            let Some((_, first)) = invocations.first() else {
                return out;
            };
            if invocations.iter().any(|(_, other)| other != first) {
                out.unreadable = Some(SCRIPT_SEVERAL);
                return out;
            }
            first.clone()
        }
    };
    out.other = invocations.iter().any(|(_, other)| *other != primary);
    match primary {
        Ok((config, root_dir)) => {
            out.config = config;
            out.root_dir = root_dir;
        }
        Err(kind) => out.unreadable = Some(kind),
    }
    out
}

/// The `include` and `exclude` lists `deno test` reads from a `deno.json` / `deno.jsonc`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DenoTest {
    /// `test.include`, when the key is set.
    pub include: Option<Vec<String>>,
    /// The top-level `exclude` then `test.exclude`, which both apply, in that order.
    /// The last entry that holds a path decides for it.
    pub exclude: Vec<DenoExclude>,
    /// A task passes `deno test` paths of its own, which replace `test.include`.
    pub task_paths: bool,
    /// `"vendor": true`: the root `vendor` directory is left out.
    pub vendor: bool,
    /// A `workspace` key: a member's own configuration applies below it.
    pub workspace: bool,
}

/// One `exclude` entry of a Deno configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DenoExclude {
    /// The path or glob, without the `!` of a negated entry.
    pub entry: String,
    /// `!path`: the entry brings back what an earlier one left out.
    pub negated: bool,
}

/// Why the lists of a Deno configuration cannot be told from the file.
pub const DENO_UNPARSED: &str =
    "the Deno configuration does not parse, or a list in it holds something that is not a string";
pub const DENO_OTHER_CONFIG: &str =
    "a task runs `deno test` with `--config` or `--no-config`, so the root Deno configuration is not the one in force";
pub const DENO_NEGATED_GLOB: &str =
    "a negated `exclude` entry of the Deno configuration is a glob, or a glob follows a negated entry, which is not evaluated";
pub const DENO_NEGATION_UNREACHED: &str =
    "a negated `exclude` entry of the Deno configuration comes before an entry that excludes it, and `deno test` refuses to run with that";

/// Whether a Deno list entry is a glob.
pub fn deno_entry_is_glob(entry: &str) -> bool {
    entry.contains(['*', '?', '[', '{'])
}

/// JSON with comments and trailing commas (`deno.jsonc`, and `deno.json`, which Deno
/// reads the same way) as plain JSON. `None` when a string or a block comment is not
/// closed.
fn strip_jsonc(source: &str) -> Option<String> {
    // Comments first, so that a comma followed by a comment and a closing bracket is
    // seen as the trailing comma it is.
    let without_comments =
        rewrite_outside_strings(source, |bytes, i, out| match (bytes[i], bytes.get(i + 1)) {
            (b'/', Some(b'/')) => {
                let line = bytes[i..].iter().position(|b| *b == b'\n');
                Some(i + line.unwrap_or(bytes.len() - i))
            }
            (b'/', Some(b'*')) => {
                let end = bytes[i + 2..].windows(2).position(|w| w == b"*/")?;
                out.push(b' ');
                Some(i + end + 4)
            }
            (other, _) => {
                out.push(other);
                Some(i + 1)
            }
        })?;
    rewrite_outside_strings(&without_comments, |bytes, i, out| {
        let closes = || {
            bytes[i + 1..]
                .iter()
                .find(|b| !b.is_ascii_whitespace())
                .is_some_and(|b| matches!(b, b']' | b'}'))
        };
        if bytes[i] != b',' || !closes() {
            out.push(bytes[i]);
        }
        Some(i + 1)
    })
}

/// Copies `source`, keeping each JSON string as it is and passing every other byte to
/// `step`, which writes what stays and returns the index to go on from (`None` to give
/// up). `None` when a string is not closed.
fn rewrite_outside_strings(
    source: &str,
    step: impl Fn(&[u8], usize, &mut Vec<u8>) -> Option<usize>,
) -> Option<String> {
    let bytes = source.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let start = i;
            i += 1;
            loop {
                match bytes.get(i)? {
                    b'\\' => i += 2,
                    b'"' => break,
                    _ => i += 1,
                }
            }
            i += 1;
            out.extend_from_slice(bytes.get(start..i)?);
        } else {
            i = step(bytes, i, &mut out)?;
        }
    }
    String::from_utf8(out).ok()
}

/// A list of path or glob strings. `None` when the value is anything else.
fn json_strings(value: &serde_json::Value) -> Option<Vec<String>> {
    value
        .as_array()?
        .iter()
        .map(|entry| entry.as_str().map(str::to_string))
        .collect()
}

/// What the tasks of a Deno configuration say about `deno test`: whether one passes it
/// a configuration file of its own, and whether one passes it paths.
fn deno_test_tasks(config: &serde_json::Value) -> (bool, bool) {
    let (mut other_config, mut paths) = (false, false);
    let Some(tasks) = config.get("tasks").and_then(|t| t.as_object()) else {
        return (other_config, paths);
    };
    for task in tasks.values() {
        let command = task
            .as_str()
            .or_else(|| task.get("command").and_then(|c| c.as_str()))
            .unwrap_or("");
        let words: Vec<&str> = command.split_whitespace().collect();
        for (at, pair) in words.windows(2).enumerate() {
            if pair != ["deno", "test"] {
                continue;
            }
            for word in words[at + 2..]
                .iter()
                .take_while(|w| !matches!(**w, "&&" | "||" | ";" | "|"))
            {
                let flag = word.split('=').next().unwrap_or(word);
                if matches!(flag, "--config" | "-c" | "--no-config") {
                    other_config = true;
                } else if !word.starts_with('-') {
                    paths = true;
                }
            }
        }
    }
    (other_config, paths)
}

/// Reads the lists `deno test` takes from a `deno.json` / `deno.jsonc`: `test.include`,
/// `test.exclude` and the top-level `exclude`. `Err` with the reason when they cannot be
/// told from the file: it does not parse, a list holds something that is not a string,
/// a task runs `deno test` with a configuration file of its own, or the `exclude` list
/// holds a negation this does not evaluate. The top-level `include` does not limit
/// `deno test`.
///
/// A negated entry (`!path`) is read when it is a plain path and only plain paths
/// follow it: `deno test` (deno 2.6) then lets the last entry that holds a file decide.
/// It refuses to run (`Invalid exclude: The negation of '!a/b' is never reached`) when
/// a later plain entry holds the negated path.
pub fn parse_deno_config(source: &str) -> Result<DenoTest, &'static str> {
    let stripped = strip_jsonc(source).ok_or(DENO_UNPARSED)?;
    let config: serde_json::Value = serde_json::from_str(&stripped).map_err(|_| DENO_UNPARSED)?;
    let (other_config, task_paths) = deno_test_tasks(&config);
    if other_config {
        return Err(DENO_OTHER_CONFIG);
    }
    let test = config.get("test");
    let mut exclude: Vec<DenoExclude> = Vec::new();
    for list in [config.get("exclude"), test.and_then(|t| t.get("exclude"))]
        .into_iter()
        .flatten()
    {
        for raw in json_strings(list).ok_or(DENO_UNPARSED)? {
            exclude.push(match raw.strip_prefix('!') {
                Some(rest) => DenoExclude {
                    entry: rest.to_string(),
                    negated: true,
                },
                None => DenoExclude {
                    entry: raw,
                    negated: false,
                },
            });
        }
    }
    for (at, entry) in exclude.iter().enumerate() {
        if !entry.negated {
            continue;
        }
        // A negated glob does not bring a file back the way a negated path does, and
        // a path written with surrounding white space names another path.
        if deno_entry_is_glob(&entry.entry) || entry.entry.trim() != entry.entry {
            return Err(DENO_NEGATED_GLOB);
        }
        let negated = deno_path_words(&entry.entry);
        for later in exclude[at + 1..].iter().filter(|e| !e.negated) {
            if deno_entry_is_glob(&later.entry) {
                return Err(DENO_NEGATED_GLOB);
            }
            let holder = deno_path_words(&later.entry);
            if negated.len() >= holder.len() && negated[..holder.len()] == holder[..] {
                return Err(DENO_NEGATION_UNREACHED);
            }
        }
    }
    let include = match test.and_then(|t| t.get("include")) {
        Some(list) => Some(json_strings(list).ok_or(DENO_UNPARSED)?),
        None => None,
    };
    Ok(DenoTest {
        include,
        exclude,
        task_paths,
        vendor: config.get("vendor").and_then(|v| v.as_bool()) == Some(true),
        workspace: config.get("workspace").is_some_and(|w| !w.is_null()),
    })
}

/// The segments of a path entry, without `.` and empty ones.
fn deno_path_words(entry: &str) -> Vec<&str> {
    entry
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect()
}

/// Whether a script of a parsed `package.json` runs Node's own test runner: a command
/// whose program is `node` with the `--test` flag among its words (`node --test`,
/// `node --import tsx --test test/`). The words are split on white space, and a command
/// list is read command by command (`&&`, `||`, `;`, `|`).
pub fn scripts_run_node_test(package: &serde_json::Value) -> bool {
    let Some(scripts) = package.get("scripts").and_then(|s| s.as_object()) else {
        return false;
    };
    scripts
        .values()
        .filter_map(|script| script.as_str())
        .any(script_runs_node_test)
}

fn script_runs_node_test(script: &str) -> bool {
    let mut in_node = false;
    for raw in script.split_whitespace() {
        if matches!(raw, "&&" | "||" | ";" | "|") {
            in_node = false;
            continue;
        }
        let ends_command = raw.ends_with(';');
        let word = raw.trim_end_matches(';').trim_matches(['"', '\'']);
        if in_node && word == "--test" {
            return true;
        }
        if matches!(word.rsplit('/').next(), Some("node" | "node.exe")) {
            in_node = true;
        }
        if ends_command {
            in_node = false;
        }
    }
    false
}

/// Whether a JavaScript or TypeScript file imports Node's test runner: an `import`
/// declaration from `node:test`, or a `require('node:test')` call, anywhere in the
/// file. The words in a comment or in another string are not an import. `false` for a
/// file that does not parse.
pub fn imports_node_test(file_name: &str, source: &str) -> bool {
    // The module name is in the text of every file that imports it.
    if !source.contains(NODE_TEST_MODULE) {
        return false;
    }
    let lower = file_name.to_ascii_lowercase();
    let language: tree_sitter::Language = if lower.ends_with(".tsx") {
        tree_sitter_typescript::LANGUAGE_TSX.into()
    } else if [".ts", ".mts", ".cts"]
        .iter()
        .any(|ext| lower.ends_with(ext))
    {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
    } else {
        tree_sitter_javascript::LANGUAGE.into()
    };
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language).is_err() {
        return false;
    }
    let Ok(tree) = crate::ast::source_text::parse(&mut parser, source) else {
        return false;
    };
    let src = source.as_bytes();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "import_statement" => {
                let from = node.child_by_field_name("source");
                if from.and_then(|n| js_string(n, src)).as_deref() == Some(NODE_TEST_MODULE) {
                    return true;
                }
                continue;
            }
            "call_expression" => {
                let callee = node.child_by_field_name("function");
                let is_require =
                    callee.is_some_and(|f| f.kind() == "identifier" && text(f, src) == "require");
                let argument = node
                    .child_by_field_name("arguments")
                    .filter(|a| a.named_child_count() == 1)
                    .and_then(|a| a.named_child(0));
                if is_require
                    && argument.and_then(|n| js_string(n, src)).as_deref() == Some(NODE_TEST_MODULE)
                {
                    return true;
                }
            }
            _ => {}
        }
        let mut cursor = node.walk();
        stack.extend(node.children(&mut cursor));
    }
    false
}

const NODE_TEST_MODULE: &str = "node:test";

/// The major version a dependency range pins, when the range is a plain one: an
/// optional `^`, `~`, `=` or `v`, then `<major>` and optional `.<minor>.<patch>` parts
/// (`^29.7.0`, `~30.0`, `29.x`, `30`). Any other range (`*`, `>=29`, `29 || 30`, a tag,
/// a URL, `workspace:*`) pins none.
pub fn plain_semver_major(range: &str) -> Option<u32> {
    let range = range.trim();
    let range = range
        .strip_prefix(['^', '~', '='])
        .unwrap_or(range)
        .trim_start();
    let range = range.strip_prefix('v').unwrap_or(range);
    let mut parts = range.split('.');
    let major = parts.next()?;
    if major.is_empty() || !major.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    for part in parts {
        let numeric = !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
        let prerelease_free = numeric || matches!(part, "x" | "X" | "*");
        if !prerelease_free {
            return None;
        }
    }
    major.parse().ok()
}

/// What a `conftest.py` says pytest must not collect below its directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConftestIgnores {
    /// Module-level `collect_ignore` / `collect_ignore_glob` lists of string literals
    /// (both empty when the file sets neither).
    Literal {
        paths: Vec<String>,
        globs: Vec<String>,
    },
    /// One of the two names is assigned something else, built up, or the file defines
    /// a `pytest_ignore_collect` hook.
    Dynamic,
}

const CONFTEST_NAMES: &[&str] = &["collect_ignore", "collect_ignore_glob"];

/// The value of a Python string literal with no prefix, escape or interpolation.
pub(crate) fn python_string(node: Node, src: &[u8]) -> Option<String> {
    if node.kind() != "string" {
        return None;
    }
    let mut cursor = node.walk();
    let mut value = String::new();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "string_start" => {
                let start = text(child, src);
                if !matches!(start, "\"" | "'" | "\"\"\"" | "'''") {
                    return None;
                }
            }
            "string_content" => {
                if child.named_child_count() > 0 {
                    return None;
                }
                value.push_str(text(child, src));
            }
            "string_end" => {}
            _ => return None,
        }
    }
    Some(value)
}

fn count_python_names(node: Node, src: &[u8], uses: &mut usize, hook: &mut bool) {
    match node.kind() {
        "identifier" if CONFTEST_NAMES.contains(&text(node, src)) => *uses += 1,
        "function_definition" => {
            let name = node.child_by_field_name("name").map(|n| text(n, src));
            if name == Some("pytest_ignore_collect") {
                *hook = true;
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();
    for child in children {
        count_python_names(child, src, uses, hook);
    }
}

/// Reads the ignore lists of a `conftest.py`.
pub fn parse_conftest(source: &str) -> ConftestIgnores {
    let none = ConftestIgnores::Literal {
        paths: Vec::new(),
        globs: Vec::new(),
    };
    // Most `conftest.py` files name neither: no need to parse them.
    if !source.contains("collect_ignore") && !source.contains("pytest_ignore_collect") {
        return none;
    }
    // The Python scanner overflows its serialization buffer on a deep enough indentation
    // stack (#636); refuse before the parser sees it, as the pack does.
    if crate::ast::scanner_limits::python_indent_nesting(source).is_err() {
        return ConftestIgnores::Dynamic;
    }
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .is_err()
    {
        return ConftestIgnores::Dynamic;
    }
    let Ok(tree) = crate::ast::source_text::parse(&mut parser, source) else {
        return ConftestIgnores::Dynamic;
    };
    let root = tree.root_node();
    let src = source.as_bytes();
    if root.has_error() {
        return ConftestIgnores::Dynamic;
    }
    let (mut uses, mut hook) = (0, false);
    count_python_names(root, src, &mut uses, &mut hook);
    if hook {
        return ConftestIgnores::Dynamic;
    }
    let mut lists: [Option<Vec<String>>; 2] = [None, None];
    let mut read = 0;
    let mut cursor = root.walk();
    for statement in root.named_children(&mut cursor) {
        if statement.kind() != "expression_statement" {
            continue;
        }
        let Some(assignment) = statement
            .named_child(0)
            .filter(|n| n.kind() == "assignment")
        else {
            continue;
        };
        let (Some(left), Some(right)) = (
            assignment.child_by_field_name("left"),
            assignment.child_by_field_name("right"),
        ) else {
            continue;
        };
        if left.kind() != "identifier" {
            continue;
        }
        let Some(index) = CONFTEST_NAMES.iter().position(|n| *n == text(left, src)) else {
            continue;
        };
        if right.kind() != "list" || lists[index].is_some() {
            return ConftestIgnores::Dynamic;
        }
        let mut items = Vec::new();
        let mut inner = right.walk();
        for item in right.named_children(&mut inner) {
            if item.kind() == "comment" {
                continue;
            }
            match python_string(item, src) {
                Some(value) => items.push(value),
                None => return ConftestIgnores::Dynamic,
            }
        }
        lists[index] = Some(items);
        read += 1;
    }
    // Every mention of either name must be one of the literal assignments just read.
    if uses != read {
        return ConftestIgnores::Dynamic;
    }
    let [paths, globs] = lists;
    ConftestIgnores::Literal {
        paths: paths.unwrap_or_default(),
        globs: globs.unwrap_or_default(),
    }
}

/// The files a Jest or Vitest configuration has the runner load around the tests: set-up
/// files, and global set-up and tear-down modules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupFiles {
    /// Each entry as written. `root` is Jest's `rootDir` or Vitest's `root` as written:
    /// the directory, relative to the configuration's own, that entries resolve against.
    Literal {
        root: Option<String>,
        entries: Vec<String>,
    },
    /// The configuration, or one of the keys that name these files, is computed.
    Dynamic,
}

/// The Jest keys that name a file the runner loads around the tests.
const JEST_SETUP_KEYS: &[&str] = &[
    "setupFiles",
    "setupFilesAfterEnv",
    "globalSetup",
    "globalTeardown",
];
/// The same for a Vitest `test` block.
const VITEST_SETUP_KEYS: &[&str] = &["setupFiles", "globalSetup"];

/// Reads the set-up files of a Jest configuration given as JSON: `jest.config.json`, or
/// the `jest` key of a `package.json`. A configuration with `projects` is not read: each
/// project has its own lists.
pub fn jest_setup_files(config: &serde_json::Value) -> SetupFiles {
    let Some(table) = config.as_object() else {
        return SetupFiles::Dynamic;
    };
    if table.get("projects").is_some_and(|p| !p.is_null()) {
        return SetupFiles::Dynamic;
    }
    let root = match table.get("rootDir") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(dir)) => Some(dir.clone()),
        Some(_) => return SetupFiles::Dynamic,
    };
    let mut entries = Vec::new();
    for key in JEST_SETUP_KEYS {
        match table.get(*key) {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::String(entry)) => entries.push(entry.clone()),
            Some(list @ serde_json::Value::Array(_)) => match json_strings(list) {
                Some(listed) => entries.extend(listed),
                None => return SetupFiles::Dynamic,
            },
            Some(_) => return SetupFiles::Dynamic,
        }
    }
    SetupFiles::Literal { root, entries }
}

/// The object literal a configuration file exports, through what commonly wraps it:
/// `defineConfig(...)`, a type assertion (`as`, `satisfies`), parentheses, and a name
/// bound once by `const` at the top level and used nowhere else (`const config = {...};
/// export default config`).
fn js_exported_object<'a>(root: Node<'a>, src: &[u8]) -> Option<Node<'a>> {
    let mut export = js_default_export(root, src)?;
    // Each step moves to a strictly smaller node or to one declaration, so this ends.
    let mut resolved_name = false;
    loop {
        match export.kind() {
            "object" => return Some(export),
            "parenthesized_expression" | "as_expression" | "satisfies_expression" => {
                export = export.named_child(0)?;
            }
            "call_expression" => {
                let function = export.child_by_field_name("function")?;
                if function.kind() != "identifier" || text(function, src) != "defineConfig" {
                    return None;
                }
                let arguments = export.child_by_field_name("arguments")?;
                if arguments.named_child_count() != 1 {
                    return None;
                }
                export = arguments.named_child(0)?;
            }
            "identifier" if !resolved_name => {
                resolved_name = true;
                let name = text(export, src);
                // The name is bound once, by `const`, and used nowhere but in the
                // export: an object that is assigned to or built up afterwards is not
                // the literal it was declared as.
                let mut uses = 0;
                let mut stack = vec![root];
                while let Some(node) = stack.pop() {
                    if node.kind() == "identifier" && text(node, src) == name {
                        uses += 1;
                    }
                    let mut cursor = node.walk();
                    stack.extend(node.children(&mut cursor));
                }
                if uses != 2 {
                    return None;
                }
                let mut bound = None;
                let mut cursor = root.walk();
                for statement in root.named_children(&mut cursor) {
                    let is_const = statement.kind() == "lexical_declaration"
                        && statement
                            .child_by_field_name("kind")
                            .is_some_and(|kind| text(kind, src) == "const");
                    if !is_const {
                        continue;
                    }
                    let mut inner = statement.walk();
                    for declarator in statement.named_children(&mut inner) {
                        let named = declarator
                            .child_by_field_name("name")
                            .is_some_and(|n| n.kind() == "identifier" && text(n, src) == name);
                        if named {
                            bound = Some(declarator.child_by_field_name("value")?);
                        }
                    }
                }
                export = bound?;
            }
            _ => return None,
        }
    }
}

/// Reads the set-up files of a configuration written as code: `jest.config.*`, whose
/// keys are at the top of the exported object, or `vitest.config.*` / `vite.config.*`,
/// whose keys are in its `test` block. Only the literal case is read: an exported
/// object literal (see [`js_exported_object`]) whose set-up keys are string literals or
/// arrays of them.
pub fn script_setup_files(file_name: &str, source: &str) -> SetupFiles {
    let typescript = [".ts", ".mts", ".cts"]
        .iter()
        .any(|ext| file_name.ends_with(ext));
    let language: tree_sitter::Language = if typescript {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
    } else {
        tree_sitter_javascript::LANGUAGE.into()
    };
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language).is_err() {
        return SetupFiles::Dynamic;
    }
    let Ok(tree) = crate::ast::source_text::parse(&mut parser, source) else {
        return SetupFiles::Dynamic;
    };
    let root = tree.root_node();
    if root.has_error() {
        return SetupFiles::Dynamic;
    }
    let jest = file_name.starts_with("jest.config.");
    read_script_setup_files(root, source.as_bytes(), jest).unwrap_or(SetupFiles::Dynamic)
}

fn read_script_setup_files(root: Node, src: &[u8], jest: bool) -> Option<SetupFiles> {
    let top = js_object_pairs(js_exported_object(root, src)?, src)?;
    let (keys, table, root_key) = if jest {
        (JEST_SETUP_KEYS, top, "rootDir")
    } else {
        let mut test = None;
        let mut top_root = None;
        for (key, value) in &top {
            match key.as_str() {
                "test" => test = Some(*value),
                "root" => top_root = Some(*value),
                _ => {}
            }
        }
        let Some(test) = test else {
            return Some(SetupFiles::Literal {
                root: None,
                entries: Vec::new(),
            });
        };
        let mut table = js_object_pairs(test, src)?;
        if !table.iter().any(|(key, _)| key == "root") {
            table.extend(top_root.map(|value| ("root".to_string(), value)));
        }
        (VITEST_SETUP_KEYS, table, "root")
    };
    let mut dir = None;
    let mut entries = Vec::new();
    for (key, value) in table {
        if key == root_key {
            dir = Some(js_string(value, src)?);
        } else if key == "projects" || key == "workspace" {
            return None;
        } else if keys.contains(&key.as_str()) {
            match value.kind() {
                "string" => entries.push(js_string(value, src)?),
                "array" => entries.extend(js_string_array(value, src)?),
                _ => return None,
            }
        }
    }
    Some(SetupFiles::Literal { root: dir, entries })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(root: Option<&str>, entries: &[&str]) -> SetupFiles {
        SetupFiles::Literal {
            root: root.map(str::to_string),
            entries: entries.iter().map(|e| e.to_string()).collect(),
        }
    }

    #[test]
    fn jest_setup_files_are_read_from_json_and_refused_when_not_literal() {
        let read = |json: &str| jest_setup_files(&serde_json::from_str(json).unwrap());
        assert_eq!(
            read(
                r#"{"rootDir": "web", "setupFiles": ["./a.js", "<rootDir>/b.js"], "setupFilesAfterEnv": ["c"], "globalSetup": "./up.js", "globalTeardown": "./down.js", "testMatch": ["**/*.js"]}"#
            ),
            setup(
                Some("web"),
                &["./a.js", "<rootDir>/b.js", "c", "./up.js", "./down.js"]
            )
        );
        assert_eq!(read(r#"{"testEnvironment": "node"}"#), setup(None, &[]));
        assert_eq!(read(r#"{"globalSetup": null}"#), setup(None, &[]));
        for dynamic in [
            r#"{"setupFiles": [1]}"#,
            r#"{"globalSetup": {"path": "x"}}"#,
            r#"{"rootDir": 3, "globalSetup": "./x.js"}"#,
            r#"{"projects": ["a", "b"], "globalSetup": "./x.js"}"#,
            r#"["not", "a", "table"]"#,
        ] {
            assert_eq!(read(dynamic), SetupFiles::Dynamic, "{dynamic}");
        }
    }

    #[test]
    fn script_setup_files_are_read_only_from_a_literal_export() {
        for (name, source, expected) in [
            (
                "jest.config.js",
                "module.exports = {\n  rootDir: 'web',\n  globalTeardown: './down.js',\n  setupFiles: ['./a.js', \"./b.js\"],\n};\n",
                setup(Some("web"), &["./down.js", "./a.js", "./b.js"]),
            ),
            (
                "jest.config.ts",
                "import type { Config } from 'jest';\n\nconst config: Config = {\n  setupFilesAfterEnv: ['./jest.setup'],\n};\n\nexport default config;\n",
                setup(None, &["./jest.setup"]),
            ),
            (
                "jest.config.ts",
                "export default { globalSetup: './up.ts' } satisfies Config;\n",
                setup(None, &["./up.ts"]),
            ),
            (
                "jest.config.mjs",
                "export default { testEnvironment: 'node' };\n",
                setup(None, &[]),
            ),
            (
                "vitest.config.ts",
                "import { defineConfig } from 'vitest/config';\n\nexport default defineConfig({\n  root: 'web',\n  test: { setupFiles: './setup.ts', globalSetup: ['./global.ts'] },\n});\n",
                setup(Some("web"), &["./setup.ts", "./global.ts"]),
            ),
            (
                "vitest.config.js",
                "export default { test: { root: 'inner', setupFiles: ['a'] }, root: 'outer' };\n",
                setup(Some("inner"), &["a"]),
            ),
            (
                "vite.config.ts",
                "export default defineConfig({ plugins: [] });\n",
                setup(None, &[]),
            ),
            // A Jest key at the top of a Vitest configuration names nothing Vitest loads.
            (
                "vitest.config.js",
                "export default { globalSetup: './x.js', test: {} };\n",
                setup(None, &[]),
            ),
        ] {
            assert_eq!(script_setup_files(name, source), expected, "{name}: {source}");
        }
        for (name, source) in [
            (
                "jest.config.js",
                "module.exports = { ...base, globalSetup: './x.js' };\n",
            ),
            (
                "jest.config.js",
                "module.exports = { globalSetup: require.resolve('./x') };\n",
            ),
            (
                "jest.config.js",
                "module.exports = { setupFiles: [path.join(__dirname, 'x.js')] };\n",
            ),
            (
                "jest.config.js",
                "module.exports = async () => ({ globalSetup: './x.js' });\n",
            ),
            (
                "jest.config.js",
                "module.exports = createJestConfig({ globalSetup: './x.js' });\n",
            ),
            (
                "jest.config.js",
                "module.exports = { projects: [{ globalSetup: './x.js' }] };\n",
            ),
            (
                "jest.config.js",
                "module.exports = { rootDir: dir, globalSetup: './x.js' };\n",
            ),
            (
                "jest.config.ts",
                "let config = {};\nconfig = { globalSetup: './x.js' };\nexport default config;\n",
            ),
            ("jest.config.ts", "export default config;\n"),
            (
                "jest.config.ts",
                "const config = {};\nconfig.globalSetup = './x.js';\nexport default config;\n",
            ),
            (
                "jest.config.js",
                "var config = { globalSetup: './x.js' };\nmodule.exports = config;\n",
            ),
            (
                "vitest.config.ts",
                "export default mergeConfig(base, { test: { setupFiles: ['a'] } });\n",
            ),
            (
                "vitest.config.ts",
                "export default defineConfig({ test: { setupFiles: files } });\n",
            ),
            ("jest.config.js", "module.exports = {\n"),
            ("jest.config.js", "const x = 1;\n"),
        ] {
            assert_eq!(
                script_setup_files(name, source),
                SetupFiles::Dynamic,
                "{name}: {source}"
            );
        }
    }

    const DEFINE_CONFIG: &str = "\
import { defineConfig } from 'vitest/config';

export default defineConfig({
  // where the app lives
  root: 'web',
  plugins: [react()],
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.ts', \"checks/**\"],
    exclude: ['**/legacy/**'],
  },
});
";
    const PLAIN_OBJECT: &str = "export default { test: { dir: 'checks', root: 'ignored' } };\n";
    const COMMON_JS: &str = "module.exports = { test: { include: ['a/**'] } };\n";
    const NO_TEST: &str = "export default defineConfig({ plugins: [] });\n";
    const TYPED: &str = "\
import type { UserConfig } from 'vitest/config';

const shared: string[] = [];
export default { test: { include: ['a/**'] } };
";

    fn literal(
        include: Option<&[&str]>,
        exclude: Option<&[&str]>,
        base: Option<&str>,
    ) -> VitestConfig {
        let list = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        VitestConfig::Literal(VitestLiteral {
            include: include.map(list),
            exclude: exclude.map(list),
            base: base.map(str::to_string),
        })
    }

    #[test]
    fn a_literal_vitest_configuration_is_read() {
        assert_eq!(
            parse_vitest_config("vitest.config.ts", DEFINE_CONFIG),
            literal(
                Some(&["src/**/*.test.ts", "checks/**"]),
                Some(&["**/legacy/**"]),
                Some("web")
            )
        );
        // `dir` wins over `root`.
        assert_eq!(
            parse_vitest_config("vitest.config.js", PLAIN_OBJECT),
            literal(None, None, Some("checks"))
        );
        assert_eq!(
            parse_vitest_config("vitest.config.cjs", COMMON_JS),
            literal(Some(&["a/**"]), None, None)
        );
        assert_eq!(
            parse_vitest_config("vite.config.ts", NO_TEST),
            VitestConfig::NoTestBlock
        );
        // TypeScript syntax elsewhere in the file needs the TypeScript grammar.
        assert_eq!(
            parse_vitest_config("vitest.config.ts", TYPED),
            literal(Some(&["a/**"]), None, None)
        );
        assert_eq!(
            parse_vitest_config("vitest.config.js", TYPED),
            VitestConfig::Dynamic
        );
    }

    #[test]
    fn a_computed_vitest_configuration_is_not_read() {
        for source in [
            "export default defineConfig(({ mode }) => ({ test: {} }));\n",
            "export default defineConfig({ ...base, test: {} });\n",
            "export default defineConfig({ test });\n",
            "export default defineConfig({ test: shared });\n",
            "export default defineConfig({ test: { ...shared } });\n",
            "export default defineConfig({ test: { include } });\n",
            "export default defineConfig({ test: { include: patterns } });\n",
            "export default defineConfig({ test: { include: [...a, 'b'] } });\n",
            "export default defineConfig({ test: { include: [`a/${b}`] } });\n",
            "export default defineConfig({ test: { include: ['a\\u002fb'] } });\n",
            "export default defineConfig({ test: { exclude: [...configDefaults.exclude] } });\n",
            "export default defineConfig({ test: { root: process.cwd() } });\n",
            "export default defineConfig({ root: dir, test: {} });\n",
            "export default defineConfig({ test: { projects: ['packages/*'] } });\n",
            "export default defineConfig({ test: { includeSource: ['src/**'] } });\n",
            "export default defineConfig({ ['te' + 'st']: {} });\n",
            "export default mergeConfig(base, { test: {} });\n",
            "export default defineConfig({ test: {} }, extra);\n",
            "const config = { test: {} };\nexport default config;\n",
            "export default { test: {} };\nmodule.exports = { test: {} };\n",
            "export const other = 1;\n",
            "export default defineConfig({ test: { include: ['a' });\n",
        ] {
            assert_eq!(
                parse_vitest_config("vitest.config.ts", source),
                VitestConfig::Dynamic,
                "{source}"
            );
        }
    }

    fn scripts(json: &str) -> JestScripts {
        read_jest_scripts(&serde_json::from_str(json).unwrap())
    }

    fn read(config: Option<&str>, root_dir: Option<&str>, other: bool) -> JestScripts {
        JestScripts {
            config: config.map(str::to_string),
            root_dir: root_dir.map(str::to_string),
            other,
            unreadable: None,
        }
    }

    #[test]
    fn jest_script_words_are_read() {
        for (json, expected) in [
            (r#"{}"#, read(None, None, false)),
            (
                r#"{"scripts": {"build": "tsc -c x"}}"#,
                read(None, None, false),
            ),
            (r#"{"scripts": {"test": "jest"}}"#, read(None, None, false)),
            (
                r#"{"scripts": {"test": "jest --config config/jest.json --ci"}}"#,
                read(Some("config/jest.json"), None, false),
            ),
            (
                r#"{"scripts": {"test": "npx jest -c 'my config.json'"}}"#,
                read(Some("my config.json"), None, false),
            ),
            (
                r#"{"scripts": {"test": "node node_modules/jest/bin/jest.js --config=a.json --rootDir=web"}}"#,
                read(Some("a.json"), Some("web"), false),
            ),
            (
                r#"{"scripts": {"test": "NODE_ENV=test jest --rootDir web"}}"#,
                read(None, Some("web"), false),
            ),
            // A flag before the Jest word belongs to another program.
            (
                r#"{"scripts": {"test": "cross-env -c x jest"}}"#,
                read(None, None, false),
            ),
            // The `test` script decides; another configuration is noted.
            (
                r#"{"scripts": {"test": "jest", "test:e2e": "jest -c e2e.json"}}"#,
                read(None, None, true),
            ),
            (
                r#"{"scripts": {"test": "jest", "test:watch": "jest --watch"}}"#,
                read(None, None, false),
            ),
            // With no `test` script, the scripts that run Jest must agree.
            (
                r#"{"scripts": {"unit": "jest -c a.json", "ci": "jest --ci -c a.json"}}"#,
                read(Some("a.json"), None, false),
            ),
            // A command list that carries no configuration to Jest changes nothing.
            (
                r#"{"scripts": {"test": "tsc -p . && jest --coverage"}}"#,
                read(None, None, false),
            ),
        ] {
            assert_eq!(scripts(json), expected, "{json}");
        }
    }

    #[test]
    fn jest_scripts_that_are_not_read_say_so() {
        for (json, kind) in [
            (
                r#"{"scripts": {"test": "tsc && jest --config a.json"}}"#,
                SCRIPT_NOT_SIMPLE,
            ),
            (
                r#"{"scripts": {"test": "jest $JEST_ARGS"}}"#,
                SCRIPT_NOT_SIMPLE,
            ),
            (
                r#"{"scripts": {"test": "jest --config 'a.json"}}"#,
                SCRIPT_NOT_SIMPLE,
            ),
            (
                r#"{"scripts": {"test": "jest --testPathPattern unit"}}"#,
                SCRIPT_UNREAD_FLAG,
            ),
            (
                r#"{"scripts": {"test": "jest --projects a b"}}"#,
                SCRIPT_UNREAD_FLAG,
            ),
            (
                r#"{"scripts": {"test": "jest --config"}}"#,
                SCRIPT_UNREAD_FLAG,
            ),
            (
                r#"{"scripts": {"unit": "jest -c a.json", "e2e": "jest -c b.json"}}"#,
                SCRIPT_SEVERAL,
            ),
        ] {
            assert_eq!(scripts(json).unreadable, Some(kind), "{json}");
        }
        // An unreadable script beside a readable `test` script is another configuration.
        let mixed = scripts(r#"{"scripts": {"test": "jest", "e2e": "jest $ARGS"}}"#);
        assert_eq!(mixed, read(None, None, true));
    }

    #[test]
    fn a_plain_semver_range_pins_a_major() {
        for (range, major) in [
            ("^29.7.0", Some(29)),
            ("~30.0.2", Some(30)),
            ("29.x", Some(29)),
            ("30", Some(30)),
            ("=28.1.3", Some(28)),
            ("v27.0.0", Some(27)),
            ("*", None),
            ("latest", None),
            (">=29", None),
            ("29 || 30", None),
            ("^29.0.0-alpha.1", None),
            ("workspace:*", None),
            ("npm:jest@29", None),
            ("", None),
        ] {
            assert_eq!(plain_semver_major(range), major, "{range:?}");
        }
    }

    fn ignores(paths: &[&str], globs: &[&str]) -> ConftestIgnores {
        ConftestIgnores::Literal {
            paths: paths.iter().map(|s| s.to_string()).collect(),
            globs: globs.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn literal_conftest_ignore_lists_are_read() {
        for (source, expected) in [
            ("import pytest\n", ignores(&[], &[])),
            (
                "collect_ignore = [\"setup.py\", 'legacy']\n",
                ignores(&["setup.py", "legacy"], &[]),
            ),
            (
                "collect_ignore = [\n    \"a.py\",  # parked\n]\ncollect_ignore_glob = [\"*_skip.py\"]\n",
                ignores(&["a.py"], &["*_skip.py"]),
            ),
            ("collect_ignore: list[str] = []\n", ignores(&[], &[])),
        ] {
            assert_eq!(parse_conftest(source), expected, "{source}");
        }
    }

    #[test]
    fn conftest_ignores_that_are_computed_are_not_read() {
        for source in [
            "collect_ignore = [\"a.py\"]\ncollect_ignore.append(\"b.py\")\n",
            "collect_ignore = [\"a.py\"]\ncollect_ignore += [\"b.py\"]\n",
            "collect_ignore = [\"a.py\"]\ncollect_ignore = [\"b.py\"]\n",
            "collect_ignore = [name]\n",
            "collect_ignore = [f\"{name}.py\"]\n",
            "collect_ignore = [r\"a\\b.py\"]\n",
            "collect_ignore = [\"a\" \"b.py\"]\n",
            "collect_ignore = (\"a.py\",)\n",
            "collect_ignore = glob.glob(\"legacy/*.py\")\n",
            "if sys.platform == \"win32\":\n    collect_ignore = [\"a.py\"]\n",
            "collect_ignore_glob = [\"*.py\" if OLD else \"x\"]\n",
            "def pytest_ignore_collect(collection_path, config):\n    return True\n",
            "collect_ignore = [\"a.py\"\n",
        ] {
            assert_eq!(parse_conftest(source), ConftestIgnores::Dynamic, "{source}");
        }
    }

    fn deno_excludes(config: &str) -> Vec<(String, bool)> {
        parse_deno_config(config)
            .unwrap()
            .exclude
            .into_iter()
            .map(|e| (e.entry, e.negated))
            .collect()
    }

    /// Each list was run with `deno test` (deno 2.6): the first group runs, with the
    /// last entry that holds a file deciding; the others are refused by Deno or leave a
    /// file out by a rule that is not read.
    #[test]
    fn a_negated_deno_exclude_entry_is_read_when_it_is_a_plain_path() {
        assert_eq!(
            deno_excludes(r#"{"exclude": ["a"], "test": {"exclude": ["!a/keep", "b"]}}"#),
            [
                ("a".to_string(), false),
                ("a/keep".to_string(), true),
                ("b".to_string(), false)
            ]
        );
        for config in [
            r#"{"test": {"exclude": ["a", "!a/keep"]}}"#,
            r#"{"test": {"exclude": ["a", "!./a/keep/"]}}"#,
            r#"{"test": {"exclude": ["a", "!a/keep", "a/keep/x_test.ts"]}}"#,
            r#"{"test": {"exclude": ["a/keep", "!a/keep"]}}"#,
            r#"{"test": {"exclude": ["a*", "!a/keep"]}}"#,
            r#"{"test": {"exclude": ["!a/keep", "b"]}}"#,
        ] {
            assert!(parse_deno_config(config).is_ok(), "{config}");
        }
        for (config, reason) in [
            (
                r#"{"test": {"exclude": ["!a/keep", "a"]}}"#,
                DENO_NEGATION_UNREACHED,
            ),
            (
                r#"{"test": {"exclude": ["!a/keep", "a/keep"]}}"#,
                DENO_NEGATION_UNREACHED,
            ),
            (
                r#"{"test": {"exclude": ["a"]}, "exclude": ["!a/keep"]}"#,
                DENO_NEGATION_UNREACHED,
            ),
            (
                r#"{"test": {"exclude": ["a/**", "!a/keep/**"]}}"#,
                DENO_NEGATED_GLOB,
            ),
            (
                r#"{"test": {"exclude": ["!a/keep", "**/x"]}}"#,
                DENO_NEGATED_GLOB,
            ),
            (
                r#"{"test": {"exclude": ["a", "! a/keep"]}}"#,
                DENO_NEGATED_GLOB,
            ),
            (r#"{"test": {"exclude": ["a", 1]}}"#, DENO_UNPARSED),
            (r#"{"test": {"include": "a"}}"#, DENO_UNPARSED),
            (r#"{"test": "#, DENO_UNPARSED),
            (
                r#"{"tasks": {"t": "deno test --no-config"}}"#,
                DENO_OTHER_CONFIG,
            ),
        ] {
            assert_eq!(parse_deno_config(config), Err(reason), "{config}");
        }
    }

    #[test]
    fn deno_keys_beside_the_lists_are_read() {
        let read = parse_deno_config(
            r#"{"vendor": true, "workspace": ["./a"], "tasks": {"t": "deno test src/"}, "test": {"include": ["src"]}}"#,
        )
        .unwrap();
        assert!(read.vendor && read.workspace && read.task_paths);
        assert_eq!(read.include, Some(vec!["src".to_string()]));
        let plain = parse_deno_config("{}").unwrap();
        assert!(!plain.vendor && !plain.workspace && !plain.task_paths);
        assert_eq!(plain.include, None);
    }
}
