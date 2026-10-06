//! Function-body facts shared by the language packs: which functions a file defines and
//! whether each body does anything.
//!
//! One walker, one classifier; per-language tables name the function node kinds, where
//! the body sits, what counts as a stub marker (`todo!()`, `raise NotImplementedError`,
//! `throw new UnsupportedOperationException`) and what counts as a trivial body
//! (`return null`, `pass`). The `stub-bodies` gate compares these facts base against head.

use tree_sitter::Node;

/// What a function body amounts to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyShape {
    /// The whole body is a not-implemented marker; `String` is the marker text.
    Stub(String),
    /// No statements at all.
    Empty,
    /// One statement that returns a constant or nothing (`return null`, `pass`).
    Trivial(String),
    /// Anything else.
    Substantive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionFacts {
    pub name: String,
    pub line: usize,
    pub end_line: usize,
    pub shape: BodyShape,
    /// A test function or a test-file member: judged by the test gates, not here.
    pub is_test: bool,
}

/// Per-language description of function nodes.
pub struct FunctionSpec {
    /// Node kinds that define a function or method with a body.
    pub function_kinds: &'static [&'static str],
    /// Field holding the name, tried in order; the first present wins.
    pub name_fields: &'static [&'static str],
    /// Field holding the body, tried in order (`body`, or a kind name for a child).
    pub body_fields: &'static [&'static str],
    /// Node kinds ignored when counting statements (comments, docstrings).
    pub ignored_kinds: &'static [&'static str],
    /// Whether the node is an abstract, overload or interface member: no body to judge.
    pub skip: fn(Node, &str) -> bool,
    /// Whether the function is a test.
    pub is_test: fn(Node, &str, &str) -> bool,
    /// Classify a single statement's text. `None` = substantive.
    pub classify: fn(&str) -> Option<BodyShape>,
}

fn text<'a>(node: Node, src: &'a str) -> &'a str {
    node.utf8_text(src.as_bytes()).unwrap_or("")
}

/// Walk `root` and collect every function `spec` describes.
pub fn extract(root: Node, src: &str, path: &str, spec: &FunctionSpec) -> Vec<FunctionFacts> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if spec.function_kinds.contains(&node.kind()) {
            if let Some(f) = describe(node, src, path, spec) {
                out.push(f);
            }
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        // Push in reverse so the walk visits in source order.
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
    out
}

fn describe(node: Node, src: &str, path: &str, spec: &FunctionSpec) -> Option<FunctionFacts> {
    if (spec.skip)(node, src) {
        return None;
    }
    let name = spec
        .name_fields
        .iter()
        // A field, or failing that a child of that kind (an Objective-C method's selector
        // is its first `identifier`).
        .find_map(|f| {
            node.child_by_field_name(f).or_else(|| {
                let mut cursor = node.walk();
                let found = node.named_children(&mut cursor).find(|c| c.kind() == *f);
                found
            })
        })
        .map(unwrap_declarator)
        .map(|n| text(n, src).trim().to_string())
        .filter(|n| !n.is_empty())
        .or_else(|| declarator_name(node, src))?;
    let body = spec.body_fields.iter().find_map(|f| {
        node.child_by_field_name(f).or_else(|| {
            let mut cursor = node.walk();
            let found = node.children(&mut cursor).find(|c| c.kind() == *f);
            found
        })
    })?;
    // Kotlin wraps a block body in `function_body`; an expression body is the
    // `function_body`'s own child. Judge the block itself when there is one.
    let body = match body.named_child(0) {
        Some(inner)
            if body.kind() == "function_body"
                && body.named_child_count() == 1
                && inner.kind() == "block" =>
        {
            inner
        }
        _ => body,
    };
    let shape = classify_body(body, src, spec);
    Some(FunctionFacts {
        name,
        line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        shape,
        is_test: (spec.is_test)(node, src, path),
    })
}

/// C and C++ name a function through its declarator: `static int *f(int a)` is a
/// `pointer_declarator` around a `function_declarator` around the identifier. Descend to
/// the innermost declarator so the name is `f`, not `*f(int a)`.
fn unwrap_declarator(node: Node) -> Node {
    let mut n = node;
    while n.kind().ends_with("_declarator") {
        match n
            .child_by_field_name("declarator")
            .or_else(|| n.named_child(0))
        {
            Some(inner) => n = inner,
            None => break,
        }
    }
    n
}

/// `const f = () => {}` and `let g = function() {}`: the name is on the declarator.
fn declarator_name(node: Node, src: &str) -> Option<String> {
    let parent = node.parent()?;
    if parent.kind() != "variable_declarator" && parent.kind() != "pair" {
        return None;
    }
    parent
        .child_by_field_name("name")
        .or_else(|| parent.child_by_field_name("key"))
        .map(|n| text(n, src).trim().trim_matches(['"', '\'']).to_string())
}

/// Statements of a body, ignoring comments, docstrings and structural punctuation.
fn statements<'a>(body: Node<'a>, spec: &FunctionSpec) -> Vec<Node<'a>> {
    let mut cursor = body.walk();
    body.named_children(&mut cursor)
        .filter(|c| !spec.ignored_kinds.contains(&c.kind()))
        // A docstring: an expression statement that is only a string literal.
        .filter(|c| {
            !(c.kind() == "expression_statement"
                && c.named_child_count() == 1
                && c.named_child(0).is_some_and(|s| s.kind() == "string"))
        })
        .collect()
}

pub fn classify_body(body: Node, src: &str, spec: &FunctionSpec) -> BodyShape {
    // An expression body (`=> throw new NotImplementedException()`, Ruby's last
    // expression) has no statement list: judge its text directly.
    let stmts = statements(body, spec);
    match stmts.len() {
        0 => {
            let t = text(body, src).trim();
            let inner = t
                .trim_start_matches(['{', '='])
                .trim_start_matches('>')
                .trim_end_matches('}')
                .trim();
            if inner.is_empty() {
                BodyShape::Empty
            } else {
                (spec.classify)(inner).unwrap_or(BodyShape::Substantive)
            }
        }
        1 => {
            let t = text(stmts[0], src).trim();
            (spec.classify)(t).unwrap_or(BodyShape::Substantive)
        }
        // A stub padded with statements that cannot affect the result (a log line, a bare
        // assignment) is still a stub; one preceded by a call is not.
        _ => {
            let last = text(stmts[stmts.len() - 1], src).trim();
            match (spec.classify)(last) {
                Some(BodyShape::Stub(marker))
                    if stmts[..stmts.len() - 1]
                        .iter()
                        .all(|st| inert_statement(text(*st, src))) =>
                {
                    BodyShape::Stub(marker)
                }
                _ => BodyShape::Substantive,
            }
        }
    }
}

/// A statement that records or assigns and calls nothing: a logging line, or an
/// assignment whose right-hand side has no call.
fn inert_statement(t: &str) -> bool {
    let t = t.trim().trim_end_matches(';').trim();
    if super::handlers::is_logging_statement(t) {
        return true;
    }
    let assignment = t.starts_with("let ")
        || t.starts_with("var ")
        || t.starts_with("val ")
        || t.starts_with("const ")
        || t.starts_with("my ")
        || t.starts_with('$')
        || t.starts_with("int ")
        || t.starts_with("auto ")
        || t.contains(" = ")
        || t.contains(" := ");
    assignment
        && t.split_once(['=', ':'])
            .is_some_and(|(_, rhs)| !rhs.contains('('))
}

fn has_word(hay: &str, needles: &[&str]) -> bool {
    let h = hay.to_ascii_lowercase();
    needles.iter().any(|n| h.contains(n))
}

const NOT_IMPLEMENTED_WORDS: &[&str] = &[
    "not implemented",
    "notimplemented",
    "unimplemented",
    "not yet implemented",
    "implement me",
    "todo",
    "tbd",
    "stub",
];

fn strip_semicolon(t: &str) -> &str {
    t.trim().trim_end_matches(';').trim()
}

fn trivial_return(t: &str, constants: &[&str]) -> Option<BodyShape> {
    let t = strip_semicolon(t);
    let rest = t.strip_prefix("return")?.trim();
    if rest.is_empty() || constants.contains(&rest) {
        return Some(BodyShape::Trivial(t.to_string()));
    }
    None
}

pub fn classify_rust(t: &str) -> Option<BodyShape> {
    let t = strip_semicolon(t);
    if t.starts_with("todo!") || t.starts_with("unimplemented!") {
        return Some(BodyShape::Stub(t.to_string()));
    }
    if t.starts_with("panic!") && has_word(t, NOT_IMPLEMENTED_WORDS) {
        return Some(BodyShape::Stub(t.to_string()));
    }
    const TRIVIAL: &[&str] = &[
        "None",
        "Ok(())",
        "Default::default()",
        "Vec::new()",
        "vec![]",
        "String::new()",
        "0",
        "false",
        "true",
        "\"\"",
        "()",
    ];
    if TRIVIAL.contains(&t) {
        return Some(BodyShape::Trivial(t.to_string()));
    }
    trivial_return(t, TRIVIAL)
}

pub fn classify_python(t: &str) -> Option<BodyShape> {
    let t = t.trim();
    if t == "pass" || t == "..." || t.starts_with("raise NotImplemented") {
        return Some(BodyShape::Stub(t.to_string()));
    }
    trivial_return(
        t,
        &["None", "{}", "[]", "\"\"", "''", "0", "False", "True", "()"],
    )
}

pub fn classify_javascript(t: &str) -> Option<BodyShape> {
    let t = strip_semicolon(t);
    if t.starts_with("throw ") && has_word(t, NOT_IMPLEMENTED_WORDS) {
        return Some(BodyShape::Stub(t.to_string()));
    }
    trivial_return(
        t,
        &[
            "null",
            "undefined",
            "{}",
            "[]",
            "\"\"",
            "''",
            "``",
            "0",
            "false",
            "true",
        ],
    )
}

pub fn classify_go(t: &str) -> Option<BodyShape> {
    let t = strip_semicolon(t);
    if t.starts_with("panic(") && has_word(t, NOT_IMPLEMENTED_WORDS) {
        return Some(BodyShape::Stub(t.to_string()));
    }
    trivial_return(
        t,
        &[
            "nil",
            "nil, nil",
            "0",
            "\"\"",
            "false",
            "true",
            "0, nil",
            "\"\", nil",
            "false, nil",
        ],
    )
}

pub fn classify_jvm(t: &str) -> Option<BodyShape> {
    let t = strip_semicolon(t);
    if t.starts_with("throw ")
        && (t.contains("UnsupportedOperationException")
            || t.contains("NotImplementedException")
            || t.contains("NotSupportedException")
            || has_word(t, NOT_IMPLEMENTED_WORDS))
    {
        return Some(BodyShape::Stub(t.to_string()));
    }
    trivial_return(
        t,
        &[
            "null",
            "0",
            "0L",
            "0.0",
            "false",
            "true",
            "\"\"",
            "default",
            "Optional.empty()",
        ],
    )
}

pub fn classify_kotlin(t: &str) -> Option<BodyShape> {
    let t = strip_semicolon(t);
    if t.starts_with("TODO(") || t == "TODO" {
        return Some(BodyShape::Stub(t.to_string()));
    }
    if t.starts_with("throw ")
        && (t.contains("NotImplementedError")
            || t.contains("UnsupportedOperationException")
            || has_word(t, NOT_IMPLEMENTED_WORDS))
    {
        return Some(BodyShape::Stub(t.to_string()));
    }
    const TRIVIAL: &[&str] = &[
        "null",
        "Unit",
        "0",
        "0L",
        "0.0",
        "false",
        "true",
        "\"\"",
        "emptyList()",
        "emptyMap()",
        "emptySet()",
        "listOf()",
        "mapOf()",
        "setOf()",
    ];
    // An expression body (`fun f() = null`) is a bare expression.
    if TRIVIAL.contains(&t) {
        return Some(BodyShape::Trivial(t.to_string()));
    }
    trivial_return(t, TRIVIAL)
}

pub fn classify_php(t: &str) -> Option<BodyShape> {
    let t = strip_semicolon(t);
    if t.starts_with("throw ") && has_word(t, NOT_IMPLEMENTED_WORDS) {
        return Some(BodyShape::Stub(t.to_string()));
    }
    const TRIVIAL: &[&str] = &["null", "[]", "0", "''", "\"\"", "false", "true"];
    // An arrow function's body is a bare expression.
    if TRIVIAL.contains(&t) {
        return Some(BodyShape::Trivial(t.to_string()));
    }
    trivial_return(t, TRIVIAL)
}

pub fn classify_swift(t: &str) -> Option<BodyShape> {
    let t = strip_semicolon(t);
    // `fatalError()` / `preconditionFailure()` with nothing to say, or saying so.
    for head in ["fatalError(", "preconditionFailure("] {
        if let Some(rest) = t.strip_prefix(head) {
            if rest.trim() == ")" || has_word(t, NOT_IMPLEMENTED_WORDS) {
                return Some(BodyShape::Stub(t.to_string()));
            }
        }
    }
    const TRIVIAL: &[&str] = &[
        "nil", "0", "0.0", "false", "true", "\"\"", "[]", "[:]", "()",
    ];
    if TRIVIAL.contains(&t) {
        return Some(BodyShape::Trivial(t.to_string()));
    }
    trivial_return(t, TRIVIAL)
}

pub fn classify_scala(t: &str) -> Option<BodyShape> {
    let t = strip_semicolon(t);
    if t == "???" {
        return Some(BodyShape::Stub(t.to_string()));
    }
    if t.starts_with("throw ")
        && (t.contains("NotImplementedError") || has_word(t, NOT_IMPLEMENTED_WORDS))
    {
        return Some(BodyShape::Stub(t.to_string()));
    }
    const TRIVIAL: &[&str] = &[
        "null", "None", "()", "0", "0L", "0.0", "false", "true", "\"\"", "Nil", "List()", "Map()",
        "Set()", "Seq()", "Vector()",
    ];
    if TRIVIAL.contains(&t) {
        return Some(BodyShape::Trivial(t.to_string()));
    }
    trivial_return(t, TRIVIAL)
}

pub fn classify_objc(t: &str) -> Option<BodyShape> {
    let t = strip_semicolon(t);
    if t.contains("doesNotRecognizeSelector:")
        || t == "abort()"
        || (t.starts_with("@throw") && has_word(t, NOT_IMPLEMENTED_WORDS))
        || ((t.starts_with("NSAssert(NO") || t.starts_with("NSAssert(0"))
            && has_word(t, NOT_IMPLEMENTED_WORDS))
    {
        return Some(BodyShape::Stub(t.to_string()));
    }
    trivial_return(
        t,
        &[
            "nil", "NO", "YES", "0", "NULL", "@\"\"", "@[]", "@{}", "false", "true",
        ],
    )
}

pub fn classify_ruby(t: &str) -> Option<BodyShape> {
    let t = t.trim();
    if t.starts_with("raise NotImplementedError") || t == "raise NotImplementedError" {
        return Some(BodyShape::Stub(t.to_string()));
    }
    if ["nil", "[]", "{}", "0", "''", "\"\"", "false", "true"].contains(&t) {
        return Some(BodyShape::Trivial(t.to_string()));
    }
    trivial_return(t, &["nil", "[]", "{}", "0", "false", "true"])
}

pub fn classify_c(t: &str) -> Option<BodyShape> {
    let t = strip_semicolon(t);
    if t == "abort()"
        || t.starts_with("assert(false)")
        || t.starts_with("assert(0)")
        || (t.starts_with("assert(") && has_word(t, NOT_IMPLEMENTED_WORDS))
        || t.starts_with("throw std::logic_error") && has_word(t, NOT_IMPLEMENTED_WORDS)
    {
        return Some(BodyShape::Stub(t.to_string()));
    }
    trivial_return(t, &["0", "nullptr", "NULL", "false", "true", "{}", "\"\""])
}

/// Never skip.
pub fn skip_none(_: Node, _: &str) -> bool {
    false
}

/// Whether `path` matches one of the repository's declared test-scope globs.
pub fn declared_test_path(path: &str, globs: &[String]) -> bool {
    globs.iter().any(|g| {
        globset::Glob::new(g)
            .map(|g| g.compile_matcher().is_match(path))
            .unwrap_or(false)
    })
}

/// The shared test-path rule or a pack's own naming convention: true when `path`
/// matches [`test_path`] or the pack's own predicate `own` (`None` keeps the
/// shared rule alone; no pack passes `None` today, the packs without their own
/// convention call [`test_path`] directly).
///
/// Used for the whole-file handler check and for each pack's `*_fn_is_test`.
/// Not used for test/helper *extraction*, which still reads each pack's own
/// `is_test_path` field, nor for repository-declared test globs (see
/// [`declared_test_path`], which callers check separately).
pub fn is_test_file(path: &str, own: Option<fn(&str) -> bool>) -> bool {
    test_path(path) || own.is_some_and(|is_own| is_own(path))
}

/// A path under a test directory or with a test file name. Cargo's `benches/` and
/// `examples/` are compiled as their own crates and are not shipped code.
pub fn test_path(path: &str) -> bool {
    test_support_path(path) || has_dir(path, "benches", false) || has_dir(path, "examples", false)
}

/// A path whose functions are test code that tests call: [`test_path`] without Cargo's
/// `benches/` and `examples/`, which no test can call into. `assertion-reduction` tracks
/// the assertion helpers of these files.
///
/// A directory rule names one whole component of the path at any depth; a file-name
/// rule matches at a word boundary of the file name ([`ends_with_word`]), so `Latest.java`
/// and `Contests.cs` are not test files because of their last letters.
pub fn test_support_path(path: &str) -> bool {
    if ["tests", "test", "__tests__", "testutil", "testutils"]
        .iter()
        .any(|dir| has_dir(path, dir, false))
    {
        return true;
    }
    let lower = path.to_ascii_lowercase();
    let p = lower.as_str();
    let name = file_name(path);
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) => (stem, ext.to_ascii_lowercase()),
        None => (name, String::new()),
    };
    p == "conftest.py"
        || p.ends_with("/conftest.py")
        || p.ends_with("_test.go")
        || names_file(p, "testutil.go")
        || names_file(p, "testutils.go")
        || p.ends_with("_test.py")
        || (JS_EXTENSIONS.contains(&ext.as_str())
            && [".test", ".spec"]
                .iter()
                .any(|word| stem.to_ascii_lowercase().ends_with(word)))
        || names_file(p, "test-utils.ts")
        || names_file(p, "test-utils.js")
        || names_file(p, "test_utils.ts")
        || names_file(p, "test_utils.js")
        || (ext == "java" && ends_with_word(stem, "Test"))
        || (ext == "cs" && ends_with_word(stem, "Tests"))
        || p.ends_with("_spec.rb")
        || p.ends_with("_test.rb")
        || (ext == "php" && ends_with_word(stem, "Test"))
}

/// The extensions a `.test.` or `.spec.` file name is read before: every module and
/// JSX form of JavaScript and TypeScript.
const JS_EXTENSIONS: &[&str] = &["js", "ts", "mjs", "cjs", "mts", "cts", "jsx", "tsx"];

/// The last component of `path`.
pub fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Whether a directory of `path` is named `dir`: one whole component at any depth, the
/// first included, and never the file name itself (`__tests__/a.js` and
/// `web/__tests__/a.js`; not `my__tests__/a.js`, not a file named `__tests__`).
/// `exact_case` compares the name as written; otherwise ASCII case is ignored.
pub fn has_dir(path: &str, dir: &str, exact_case: bool) -> bool {
    let mut components = path.split('/');
    components.next_back();
    components.any(|c| {
        if exact_case {
            c == dir
        } else {
            c.eq_ignore_ascii_case(dir)
        }
    })
}

/// Whether `stem` (a file name without its extension) ends with the test word `word` at
/// a word boundary. `word` is written as a class name spells it (`Test`, `Tests`,
/// `TestCase`, `Spec`, `IT`).
///
/// A boundary is the start of the name, a separator (any character that is neither a
/// letter nor a digit: `_`, `-`, `.`), or a change of case that starts a new word:
/// - a capitalised word (`Test`) spelled with its capital starts a word wherever it
///   stands (`RepoTest`, `HTTPTest`, `V2Test`);
/// - a word in capitals (`IT`) starts one only after a lower-case letter or a digit
///   (`RepoIT`, not `AUDIT`);
/// - in any other spelling (`test`, `TEST`) the word needs the start of the name or a
///   separator before it (`repo_test`, not `Latest`, `repotest` or `LATEST`).
pub fn ends_with_word(stem: &str, word: &str) -> bool {
    if stem.len() < word.len() || !stem.is_char_boundary(stem.len() - word.len()) {
        return false;
    }
    let (before, tail) = stem.split_at(stem.len() - word.len());
    let Some(prev) = before.chars().next_back() else {
        return tail.eq_ignore_ascii_case(word);
    };
    if !prev.is_alphanumeric() {
        return tail.eq_ignore_ascii_case(word);
    }
    if tail != word {
        return false;
    }
    let capitalised = word.chars().nth(1).is_some_and(char::is_lowercase);
    capitalised || prev.is_lowercase() || prev.is_numeric()
}

/// Whether `stem` starts with the test word `Test`, `Tests` or `Testing` and a word
/// ends there: the next character is an upper-case letter, a digit, a separator, or
/// the end of the stem (`TestRepo`, `Test1`, `Test_x`, `TestingSupport`; not
/// `Testimonial`, `Tester`, `Testable`). This is the boundary-checked form of the
/// `Test*` include of Maven Surefire.
pub fn starts_with_test_word(stem: &str) -> bool {
    ["Test", "Tests", "Testing"].iter().any(|word| {
        stem.strip_prefix(word).is_some_and(|rest| {
            rest.chars()
                .next()
                .is_none_or(|c| c.is_uppercase() || c.is_numeric() || !c.is_alphanumeric())
        })
    })
}

/// The stems a test-named file would have without its test word: the name of the code
/// it tests, as the boundary rules split it. `test_loader.py`, `loader_test.py` and
/// `loader.test.js` give `loader`; `RepoTest.java`, `TestRepo.java` and `RepoSpec.kt`
/// give `Repo`. Empty when the name holds no test word at a boundary (`conftest.py`).
pub fn stems_without_test_word(name: &str) -> Vec<String> {
    const SUFFIXES: &[&str] = &["TestCase", "Tests", "Test", "Spec", "Suite", "Smoke", "IT"];
    const PREFIXES: &[&str] = &["Testing", "Tests", "Test", "Smoke"];
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    let separator = |c: char| !c.is_alphanumeric();
    let mut found = Vec::new();
    for word in SUFFIXES {
        if ends_with_word(stem, word) {
            found.push(stem[..stem.len() - word.len()].trim_end_matches(separator));
        }
    }
    for word in PREFIXES {
        let Some((head, rest)) = stem.split_at_checked(word.len()) else {
            continue;
        };
        let next = rest.chars().next();
        let at_separator = head.eq_ignore_ascii_case(word) && next.is_some_and(separator);
        let at_capital = head == *word && next.is_some_and(|c| c.is_uppercase() || c.is_numeric());
        if at_separator || at_capital {
            found.push(rest.trim_start_matches(separator));
        }
    }
    found.retain(|s| !s.is_empty());
    found.into_iter().map(str::to_string).collect()
}

/// Whether a build tool or test runner makes the file test code by construction,
/// whatever it holds, so that the path is more than a naming heuristic:
/// - Go compiles a `_test.go` file into test binaries only;
/// - Cargo builds a `tests/`, `benches/` or `examples/` directory as test, bench and
///   example targets (Rust files only);
/// - pytest loads a file named `conftest.py` and nothing else by that name;
/// - Maven and Gradle compile the `src/test/`, `src/androidTest/` and `src/it/` source
///   sets onto the test classpath (Java, Kotlin, Scala);
/// - the Swift Package Manager builds a `Tests/` directory as test targets (Swift files
///   only).
///
/// Every other test path ([`test_path`], a pack's own convention) is a name or a
/// directory a project chose: `tests/support/helpers.py`, `TestDataBuilder.cs`,
/// `__tests__/setup.js`, `testutil/files.go`.
pub fn test_by_construction(path: &str) -> bool {
    let name = file_name(path);
    let ext = name.rsplit_once('.').map_or("", |(_, ext)| ext);
    go_test_file(path)
        || name == "conftest.py"
        || (ext == "rs"
            && ["tests", "benches", "examples"]
                .iter()
                .any(|dir| has_dir(path, dir, true)))
        || (ext == "swift" && has_dir(path, "Tests", true))
        || (["java", "kt", "kts", "scala"].contains(&ext) && in_test_source_set(path))
}

/// A Go `_test.go` file.
pub fn go_test_file(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with("_test.go")
}

/// Whether a directory `src` of `path` is followed by a test source set of Maven or
/// Gradle: `src/test/`, `src/androidTest/` or `src/it/`.
fn in_test_source_set(path: &str) -> bool {
    let mut components: Vec<&str> = path.split('/').collect();
    components.pop();
    components
        .windows(2)
        .any(|pair| pair[0] == "src" && ["test", "androidTest", "it"].contains(&pair[1]))
}

/// Whether the file of `p` is named `name`, alone or after a separator
/// (`test-utils.ts`, `db/test-utils.ts`, `render-test-utils.ts`): `latest-utils.ts`
/// only ends with the same letters.
pub fn names_file(p: &str, name: &str) -> bool {
    p.strip_suffix(name).is_some_and(|before| {
        before
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_ascii_alphanumeric())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #562: a test-utility name is matched as a file name, alone or after a separator,
    /// not as the last letters of another name.
    #[test]
    fn a_test_utility_name_is_a_file_name_not_a_suffix() {
        for path in [
            "src/test-utils.ts",
            "test-utils.js",
            "src/render-test-utils.ts",
            "src/db_test_utils.js",
            "pkg/testutil.go",
            "pkg/db_testutils.go",
        ] {
            assert!(test_path(path), "{path}");
        }
        for path in [
            "src/latest-utils.ts",
            "src/latest_utils.js",
            "pkg/latestutil.go",
            "pkg/contestutils.go",
        ] {
            assert!(!test_path(path), "{path}");
        }
    }

    /// #598: a test word ends a name only at a word boundary.
    #[test]
    fn a_test_word_ends_a_name_only_at_a_word_boundary() {
        for (stem, word) in [
            ("RepoTest", "Test"),
            ("Test", "Test"),
            ("HTTPTest", "Test"),
            ("V2Test", "Test"),
            ("repo_test", "Test"),
            ("repo-TEST", "Test"),
            ("Repo.Tests", "Tests"),
            ("RepoIT", "IT"),
            ("V2IT", "IT"),
            ("Repo_it", "IT"),
            ("IT", "IT"),
        ] {
            assert!(ends_with_word(stem, word), "{stem} / {word}");
        }
        for (stem, word) in [
            ("Latest", "Test"),
            ("repotest", "Test"),
            ("LATEST", "Test"),
            ("Contests", "Tests"),
            ("AUDIT", "IT"),
            ("WebKIT", "IT"),
            ("Audit", "IT"),
            ("es", "Test"),
            ("\u{e9}est", "Test"),
        ] {
            assert!(!ends_with_word(stem, word), "{stem} / {word}");
        }
    }

    /// #598: `Test`, `Tests` and `Testing` start a name only when a word ends there.
    #[test]
    fn a_test_word_starts_a_name_only_when_a_word_ends_there() {
        for stem in [
            "Test",
            "TestRepo",
            "Test1",
            "Test_x",
            "TestsBase",
            "Testing",
            "TestingUtils",
            "TestDataBuilder",
        ] {
            assert!(starts_with_test_word(stem), "{stem}");
        }
        for stem in [
            "Testimonial",
            "TestimonialController",
            "Tester",
            "Testable",
            "Testsuite",
            "Testings",
            "test_x",
            "ATest",
        ] {
            assert!(!starts_with_test_word(stem), "{stem}");
        }
    }

    /// #598: the stem a test-named file would have without its test word.
    #[test]
    fn the_test_word_is_removed_from_a_stem_at_its_boundary() {
        for (name, stem) in [
            ("test_loader.py", "loader"),
            ("loader_test.py", "loader"),
            ("loader.test.js", "loader"),
            ("util.spec.mjs", "util"),
            ("repo_test.go", "repo"),
            ("RepoTest.java", "Repo"),
            ("TestRepo.java", "Repo"),
            ("RepoTests.cs", "Repo"),
            ("RepoTestCase.java", "Repo"),
            ("RepoSpec.kt", "Repo"),
            ("RepoIT.kt", "Repo"),
            ("repo_spec.rb", "repo"),
            ("smoke_io.c", "io"),
        ] {
            assert!(
                stems_without_test_word(name).contains(&stem.to_string()),
                "{name}: {:?}",
                stems_without_test_word(name)
            );
        }
        for name in [
            "conftest.py",
            "Latest.java",
            "Testimonial.java",
            "Test.java",
            "loader.py",
        ] {
            assert!(stems_without_test_word(name).is_empty(), "{name}");
        }
    }

    /// #598: a directory rule names a whole component at any depth, the first included.
    #[test]
    fn test_code_by_construction_is_a_build_tool_rule_not_a_name() {
        for path in [
            "pkg/repo_test.go",
            "pkg/REPO_TEST.GO",
            "tests/common/mod.rs",
            "crates/a/benches/support.rs",
            "examples/demo.rs",
            "conftest.py",
            "tests/unit/conftest.py",
            "svc/src/test/java/com/x/Fixtures.java",
            "app/src/androidTest/kotlin/Fixtures.kt",
            "src/it/scala/Fixtures.scala",
            "Tests/AppTests/Support.swift",
        ] {
            assert!(test_by_construction(path), "{path}");
        }
        for path in [
            "tests/support/helpers.py",
            "app/test_runner.py",
            "app/myconftest.py",
            "src/Core/TestDataBuilder.cs",
            "web/__tests__/setup.js",
            "pkg/testutil/files.go",
            "pkg/test/files.go",
            "src/testutil/files.rs",
            "Tests/support.rs",
            "tests/Support.swift",
            "tests/Fixtures.java",
            "test/java/Fixtures.java",
            "app/test/java/Fixtures.java",
            "src/main/test/Fixtures.java",
            "src/main/java/TestFixtures.java",
            "src/test",
        ] {
            assert!(!test_by_construction(path), "{path}");
        }
    }

    #[test]
    fn a_directory_rule_names_a_whole_component_at_any_depth() {
        assert!(has_dir("__tests__/a.js", "__tests__", false));
        assert!(has_dir("web/__TESTS__/deep/a.js", "__tests__", false));
        assert!(!has_dir("web/__TESTS__/a.js", "__tests__", true));
        assert!(!has_dir("my__tests__/a.js", "__tests__", false));
        assert!(!has_dir("web/__tests__", "__tests__", false));
        assert!(!has_dir("__tests__", "__tests__", false));
        assert!(test_path("__tests__/a.js"));
        assert!(test_path("crates/a/benches/x.rs"));
        assert!(!test_path("src/mybenches/x.rs"));
    }

    /// #598: `.test.` and `.spec.` before every JavaScript and TypeScript extension.
    #[test]
    fn test_and_spec_suffixes_cover_the_module_and_jsx_extensions() {
        for ext in ["js", "ts", "mjs", "cjs", "mts", "cts", "jsx", "tsx"] {
            assert!(test_path(&format!("web/util.test.{ext}")), "{ext}");
            assert!(test_path(&format!("web/util.spec.{ext}")), "{ext}");
            assert!(!test_path(&format!("web/utiltest.{ext}")), "{ext}");
        }
        assert!(!test_path("web/util.test.json"));
        assert!(!test_path("web/util.test.py"));
    }

    /// #562: `examples/` and `benches/` are test paths, but not test support: no test
    /// calls into them, so their functions are not assertion helpers.
    #[test]
    fn examples_and_benches_are_test_paths_but_not_test_support() {
        for path in [
            "examples/show.rs",
            "benches/load.rs",
            "crates/a/examples/x.rs",
        ] {
            assert!(test_path(path), "{path}");
            assert!(!test_support_path(path), "{path}");
        }
        for path in [
            "tests/common/mod.rs",
            "tests/examples/helpers.py",
            "conftest.py",
            "pkg/helpers_test.go",
            "src/test-utils.ts",
        ] {
            assert!(test_path(path), "{path}");
            assert!(test_support_path(path), "{path}");
        }
    }

    #[test]
    fn is_test_file_with_no_own_convention_is_the_shared_rule() {
        assert!(is_test_file("benches/a.rs", None));
        assert!(!is_test_file("src/a.rs", None));
    }

    #[test]
    fn classifiers_tell_stubs_from_trivial_and_substantive_bodies() {
        assert!(matches!(classify_rust("todo!()"), Some(BodyShape::Stub(_))));
        assert!(matches!(
            classify_rust("unimplemented!(\"later\");"),
            Some(BodyShape::Stub(_))
        ));
        assert!(matches!(
            classify_rust("panic!(\"not implemented\")"),
            Some(BodyShape::Stub(_))
        ));
        assert!(classify_rust("panic!(\"index out of range\")").is_none());
        assert!(matches!(classify_rust("None"), Some(BodyShape::Trivial(_))));
        assert!(classify_rust("self.inner.len()").is_none());

        assert!(matches!(
            classify_python("raise NotImplementedError(\"soon\")"),
            Some(BodyShape::Stub(_))
        ));
        assert!(matches!(classify_python("..."), Some(BodyShape::Stub(_))));
        assert!(matches!(
            classify_python("return None"),
            Some(BodyShape::Trivial(_))
        ));
        assert!(classify_python("return compute(x)").is_none());

        assert!(matches!(
            classify_javascript("throw new Error(\"Not implemented\");"),
            Some(BodyShape::Stub(_))
        ));
        assert!(classify_javascript("throw new Error(\"bad input\");").is_none());
        assert!(matches!(
            classify_javascript("return null;"),
            Some(BodyShape::Trivial(_))
        ));

        assert!(matches!(
            classify_go("panic(\"TODO\")"),
            Some(BodyShape::Stub(_))
        ));
        assert!(matches!(
            classify_go("return nil, nil"),
            Some(BodyShape::Trivial(_))
        ));
        assert!(matches!(
            classify_jvm("throw new UnsupportedOperationException();"),
            Some(BodyShape::Stub(_))
        ));
        assert!(matches!(
            classify_jvm("return null;"),
            Some(BodyShape::Trivial(_))
        ));
        assert!(matches!(
            classify_ruby("raise NotImplementedError"),
            Some(BodyShape::Stub(_))
        ));
        assert!(matches!(
            classify_php("throw new \\RuntimeException('not implemented');"),
            Some(BodyShape::Stub(_))
        ));
        assert!(matches!(classify_c("abort()"), Some(BodyShape::Stub(_))));
    }
}

#[cfg(all(
    test,
    feature = "lang-rust",
    feature = "lang-python",
    feature = "lang-javascript",
    feature = "lang-go",
    feature = "lang-java",
    feature = "lang-csharp",
    feature = "lang-php",
    feature = "lang-ruby",
    feature = "lang-c",
    feature = "lang-cpp",
    feature = "lang-kotlin"
))]
mod pack_tests {
    use super::BodyShape;
    use crate::ast::{default_registry, AssertVocabulary, Fact};

    fn shapes(path: &str, src: &str) -> Vec<(String, BodyShape, bool)> {
        let reg = default_registry();
        let pack = reg.find_pack(path).expect("pack");
        assert!(pack.supplies(Fact::Functions));
        pack.extract(path, src, &AssertVocabulary::default())
            .unwrap()
            .functions
            .into_iter()
            .map(|f| (f.name, f.shape, f.is_test))
            .collect()
    }

    fn stub(s: &str) -> BodyShape {
        BodyShape::Stub(s.into())
    }

    #[test]
    fn rust_functions_and_tests_and_cfg_test_modules() {
        let got = shapes(
            "src/lib.rs",
            "pub fn a() -> u8 { todo!() }\nfn b() {}\nfn c() -> Option<u8> { None }\n\
             fn d(x: u8) -> u8 { x + 1 }\n#[test]\nfn t() { todo!() }\n\
             #[cfg(test)]\nmod tests { fn helper() { unimplemented!() } }\n\
             trait T { fn m(&self) { todo!() } }\n",
        );
        assert_eq!(
            got,
            vec![
                ("a".into(), stub("todo!()"), false),
                ("b".into(), BodyShape::Empty, false),
                ("c".into(), BodyShape::Trivial("None".into()), false),
                ("d".into(), BodyShape::Substantive, false),
                ("t".into(), stub("todo!()"), true),
                ("m".into(), stub("todo!()"), false),
            ]
        );
    }

    #[test]
    fn python_docstrings_protocols_and_abstract_methods() {
        let got = shapes(
            "pkg/a.py",
            "from typing import Protocol\nfrom abc import abstractmethod\n\
             def a():\n    \"\"\"doc\"\"\"\n    raise NotImplementedError\n\
             def b(x):\n    \"\"\"doc\"\"\"\n    return x * 2\n\
             class P(Protocol):\n    def m(self): ...\n\
             class C:\n    @abstractmethod\n    def n(self): ...\n    def o(self):\n        pass\n\
             def test_z():\n    pass\n",
        );
        assert_eq!(
            got,
            vec![
                ("a".into(), stub("raise NotImplementedError"), false),
                ("b".into(), BodyShape::Substantive, false),
                ("o".into(), stub("pass"), false),
                ("test_z".into(), stub("pass"), true),
            ]
        );
    }

    #[test]
    fn javascript_declarations_methods_arrows_and_test_callbacks() {
        let got = shapes(
            "src/a.ts",
            "export function a() { throw new Error('Not implemented'); }\n\
             const b = () => null;\nconst c = (x: number) => { return x + 1; };\n\
             class K { m() { return null; } abstract n(): void; }\n\
             it('works', () => { expect(1).toBe(1); });\n",
        );
        let names: Vec<(String, bool)> = got.iter().map(|(n, _, t)| (n.clone(), *t)).collect();
        assert!(names.contains(&("a".into(), false)));
        assert!(matches!(&got[0].1, BodyShape::Stub(_)), "{got:?}");
        let m = got.iter().find(|(n, _, _)| n == "m").unwrap();
        assert!(matches!(m.1, BodyShape::Trivial(_)));
        let c = got.iter().find(|(n, _, _)| n == "c").unwrap();
        assert_eq!(c.1, BodyShape::Substantive);
        assert!(
            !got.iter().any(|(n, _, _)| n == "n"),
            "abstract member skipped: {got:?}"
        );
    }

    #[test]
    fn go_java_and_csharp_bodies() {
        let go = shapes(
            "pkg/a.go",
            "package a\nfunc A() error { panic(\"not implemented\") }\nfunc (s *S) B() (int, error) { return 0, nil }\nfunc C() int { return compute() }\n",
        );
        assert!(matches!(go[0].1, BodyShape::Stub(_)));
        assert!(matches!(go[1].1, BodyShape::Trivial(_)));
        assert_eq!(go[2].1, BodyShape::Substantive);

        let java = shapes(
            "src/main/java/A.java",
            "abstract class A {\n  abstract void x();\n  int y() { throw new UnsupportedOperationException(); }\n  int z() { return 1 + 2; }\n  @Test void t() { }\n}\n",
        );
        assert_eq!(java.len(), 3, "{java:?}");
        assert!(matches!(java[0].1, BodyShape::Stub(_)));
        assert_eq!(java[1].1, BodyShape::Substantive);
        assert!(java[2].2);

        let cs = shapes(
            "src/A.cs",
            "class A {\n  public int X() => throw new NotImplementedException();\n  public int Y() { return 0; }\n  public abstract int Z();\n}\ninterface I { int W(); }\n",
        );
        assert_eq!(cs.len(), 2, "{cs:?}");
        assert!(matches!(cs[0].1, BodyShape::Stub(_)), "{cs:?}");
        assert!(matches!(cs[1].1, BodyShape::Trivial(_)));
    }

    #[test]
    fn php_ruby_and_c_cpp_bodies() {
        let got = shapes(
            "src/Repo.php",
            "<?php\nabstract class Repo {\n    abstract function find(int $id);\n    function save($e) { throw new \\RuntimeException('not implemented'); }\n    function all(): array { return []; }\n    function count(): int { return $this->n + 1; }\n}\nfunction testHelper() { return null; }\n",
        );
        assert_eq!(
            got,
            vec![
                (
                    "save".into(),
                    stub("throw new \\RuntimeException('not implemented')"),
                    false
                ),
                ("all".into(), BodyShape::Trivial("return []".into()), false),
                ("count".into(), BodyShape::Substantive, false),
                (
                    "testHelper".into(),
                    BodyShape::Trivial("return null".into()),
                    true
                ),
            ]
        );

        let got = shapes(
            "lib/repo.rb",
            "class Repo\n  def find(id)\n    raise NotImplementedError\n  end\n  def all\n    []\n  end\n  def count\n    @n + 1\n  end\n  def self.build\n    new\n  end\n  def test_x\n    nil\n  end\nend\n",
        );
        assert_eq!(
            got,
            vec![
                ("find".into(), stub("raise NotImplementedError"), false),
                ("all".into(), BodyShape::Trivial("[]".into()), false),
                ("count".into(), BodyShape::Substantive, false),
                ("build".into(), BodyShape::Substantive, false),
                ("test_x".into(), BodyShape::Trivial("nil".into()), true),
            ]
        );

        // The name comes from inside the declarator chain, not `*g(int a)`.
        let got = shapes(
            "src/io.c",
            "static int *g(int a) { return 0; }\nvoid h(void) {}\nint k(int a) { abort(); }\nint m(int a) { return a + 1; }\nvoid test_m(void) { assert(m(1) == 2); }\n",
        );
        assert_eq!(
            got,
            vec![
                ("g".into(), BodyShape::Trivial("return 0".into()), false),
                ("h".into(), BodyShape::Empty, false),
                ("k".into(), stub("abort()"), false),
                ("m".into(), BodyShape::Substantive, false),
                ("test_m".into(), BodyShape::Substantive, true),
            ]
        );
        let got = shapes(
            "src/a.cpp",
            "namespace n {\nclass A {\npublic:\n  virtual int a() = 0;\n  int b() { throw std::logic_error(\"not implemented\"); }\n  A() = default;\n  ~A() {}\n};\n}\nint A::c() { return 1 + 2; }\nTEST(S, N) { EXPECT_EQ(1, 1); }\n",
        );
        assert_eq!(
            got,
            vec![
                (
                    "b".into(),
                    stub("throw std::logic_error(\"not implemented\")"),
                    false
                ),
                ("~A".into(), BodyShape::Empty, false),
                ("A::c".into(), BodyShape::Substantive, false),
                ("TEST".into(), BodyShape::Substantive, true),
            ]
        );
    }

    #[test]
    fn kotlin_expression_and_block_bodies() {
        let got = shapes(
            "src/main/kotlin/Repo.kt",
            "class Repo {\n    fun find(id: Int): Item = TODO(\"later\")\n    fun none() = null\n    fun count(): Int {\n        return n + 1\n    }\n    fun empty() {\n    }\n    abstract fun z()\n    fun gone(): Int {\n        TODO()\n    }\n    fun bail(): Int {\n        throw NotImplementedError()\n    }\n}\n",
        );
        assert_eq!(
            got,
            vec![
                ("find".into(), stub("TODO(\"later\")"), false),
                ("none".into(), BodyShape::Trivial("null".into()), false),
                ("count".into(), BodyShape::Substantive, false),
                ("empty".into(), BodyShape::Empty, false),
                ("gone".into(), stub("TODO()"), false),
                ("bail".into(), stub("throw NotImplementedError()"), false),
            ]
        );
        let got = shapes(
            "src/test/kotlin/RepoTest.kt",
            "class RepoTest {\n    @Test\n    fun finds() {\n        assertEquals(1, 1)\n    }\n}\n",
        );
        assert_eq!(got, vec![("finds".into(), BodyShape::Substantive, true)]);
    }

    #[test]
    fn a_stub_padded_with_logging_or_a_bare_assignment_is_a_stub() {
        let rs = shapes(
            "src/lib.rs",
            "fn a() { log::warn!(\"todo\"); todo!() }\nfn b() { init(); todo!() }\nfn c() { let n = 3; eprintln!(\"later {n}\"); unimplemented!() }\nfn d() { let n = compute(); todo!() }\n",
        );
        assert_eq!(
            rs,
            vec![
                ("a".into(), stub("todo!()"), false),
                ("b".into(), BodyShape::Substantive, false),
                ("c".into(), stub("unimplemented!()"), false),
                ("d".into(), BodyShape::Substantive, false),
            ]
        );
        let py = shapes(
            "pkg/a.py",
            "def a():\n    print(\"later\")\n    raise NotImplementedError\n\ndef b():\n    x = 1\n    raise NotImplementedError\n\ndef c():\n    setup()\n    raise NotImplementedError\n",
        );
        assert_eq!(
            py,
            vec![
                ("a".into(), stub("raise NotImplementedError"), false),
                ("b".into(), stub("raise NotImplementedError"), false),
                ("c".into(), BodyShape::Substantive, false),
            ]
        );
        let ts = shapes(
            "src/a.ts",
            "export function a() {\n  console.warn('todo');\n  throw new Error('not implemented');\n}\nexport function b() {\n  const r = prepare();\n  throw new Error('not implemented');\n}\n",
        );
        assert_eq!(
            ts,
            vec![
                (
                    "a".into(),
                    stub("throw new Error('not implemented')"),
                    false
                ),
                ("b".into(), BodyShape::Substantive, false),
            ]
        );
    }
}
