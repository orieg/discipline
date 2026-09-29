//! Language pack abstraction and fact extraction.
//!
//! Language packs extract language-neutral facts ([`ParsedFileFacts`]) from source
//! files for the agent-guard gates to reason about.
//! Everything here is keyed on language syntax nodes, never substring matches.

use anyhow::Result;

pub mod bounds;
pub mod budgets;
#[cfg(any(feature = "lang-c", feature = "lang-cpp"))]
pub mod c_cpp;
pub mod c_macros;
pub mod calls;
#[cfg(feature = "lang-csharp")]
pub mod csharp;
pub mod functions;
#[cfg(feature = "lang-go")]
pub mod r#go;
#[cfg(feature = "lang-golden")]
pub mod golden;
pub mod handlers;
#[cfg(feature = "lang-java")]
pub mod java;
#[cfg(feature = "lang-javascript")]
pub mod javascript;
#[cfg(feature = "lang-kotlin")]
pub mod kotlin;
pub mod mocks;
#[cfg(feature = "lang-objc")]
pub mod objc;
#[cfg(feature = "lang-php")]
pub mod php;
pub mod prose;
#[cfg(feature = "lang-python")]
pub mod python;
pub mod reach;
pub mod retries;
#[cfg(feature = "lang-ruby")]
pub mod ruby;
#[cfg(feature = "lang-rust")]
pub mod rust;
#[cfg(feature = "lang-scala")]
pub mod scala;
#[cfg(feature = "lang-swift")]
pub mod swift;

/// Language pack abstraction trait.
///
/// Any supported ecosystem (Rust, Python, Golden/Snapshot, JS/TS, etc.) implements
/// this trait to extract test items, assertion counts, and escape hatches into
/// universal [`ParsedFileFacts`].
pub trait LanguagePack: Send + Sync {
    /// Stable kebab-case pack identifier (e.g. "rust", "golden", "python", "javascript").
    fn id(&self) -> &'static str;

    /// Which facts this pack fills in. A gate that needs a fact the pack does not supply
    /// names the file as not analysed instead of reading an empty list as "none found".
    fn supplies(&self, fact: Fact) -> bool {
        matches!(fact, Fact::Tests | Fact::EscapeHatches)
    }

    /// Human-readable display name (e.g. "Rust", "Golden/Snapshot", "Python").
    fn name(&self) -> &'static str;

    /// Whether this pack handles the given relative path.
    fn matches(&self, path: &str) -> bool;

    /// Extract language-neutral facts from source text.
    fn extract(&self, path: &str, src: &str, vocab: &AssertVocabulary) -> Result<ParsedFileFacts>;
}

/// A kind of fact a pack may or may not extract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fact {
    Tests,
    EscapeHatches,
    UnsafeSites,
    Functions,
    Handlers,
    Prose,
    /// Testing-effort budgets (proptest `cases`, Hypothesis `max_examples`, ...).
    Budgets,
}

/// Registry of active language packs.
#[derive(Default)]
pub struct LanguageRegistry {
    packs: Vec<Box<dyn LanguagePack>>,
}

impl LanguageRegistry {
    pub fn new() -> Self {
        Self { packs: Vec::new() }
    }

    pub fn register(&mut self, pack: Box<dyn LanguagePack>) {
        self.packs.push(pack);
    }

    pub fn find_pack(&self, path: &str) -> Option<&dyn LanguagePack> {
        self.packs
            .iter()
            .find(|p| p.matches(path))
            .map(|p| p.as_ref())
    }

    pub fn is_supported(&self, path: &str) -> bool {
        self.find_pack(path).is_some()
    }

    pub fn packs(&self) -> &[Box<dyn LanguagePack>] {
        &self.packs
    }
}

/// Constructs the default registry containing all built-in packs enabled by Cargo features.
pub fn default_registry() -> LanguageRegistry {
    #[allow(unused_mut)]
    let mut reg = LanguageRegistry::new();
    #[cfg(feature = "lang-rust")]
    reg.register(Box::new(rust::RustPack));
    #[cfg(feature = "lang-golden")]
    reg.register(Box::new(golden::GoldenPack));
    #[cfg(feature = "lang-python")]
    reg.register(Box::new(python::PythonPack));
    #[cfg(feature = "lang-javascript")]
    reg.register(Box::new(javascript::JavaScriptPack));
    #[cfg(feature = "lang-java")]
    reg.register(Box::new(java::JavaPack));
    #[cfg(feature = "lang-go")]
    reg.register(Box::new(r#go::GoPack));
    #[cfg(feature = "lang-php")]
    reg.register(Box::new(php::PhpPack));
    #[cfg(feature = "lang-c")]
    reg.register(Box::new(c_cpp::CPack));
    #[cfg(feature = "lang-cpp")]
    reg.register(Box::new(c_cpp::CppPack));
    #[cfg(feature = "lang-csharp")]
    reg.register(Box::new(csharp::CSharpPack));
    #[cfg(feature = "lang-ruby")]
    reg.register(Box::new(ruby::RubyPack));
    #[cfg(feature = "lang-kotlin")]
    reg.register(Box::new(kotlin::KotlinPack));
    #[cfg(feature = "lang-swift")]
    reg.register(Box::new(swift::SwiftPack));
    #[cfg(feature = "lang-scala")]
    reg.register(Box::new(scala::ScalaPack));
    #[cfg(feature = "lang-objc")]
    reg.register(Box::new(objc::ObjcPack));
    reg
}

/// Languages with a built-in fact extractor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Java,
    Go,
    Php,
    C,
    Cpp,
    CSharp,
    Ruby,
    Kotlin,
    Swift,
    Scala,
    ObjectiveC,
}

/// Source extensions discipline recognises but cannot analyse yet. A change
/// touching these is *named* in the report: the AST gates did not look at it.
pub const UNSUPPORTED_SOURCE_EXTS: &[&str] = &[
    "py", "js", "jsx", "mjs", "cjs", "ts", "tsx", "kt", "kts", "scala", "c", "h", "cc", "cpp",
    "cxx", "hpp", "hh", "cs", "rb", "swift", "php", "phpt", "m", "mm",
    // Languages with no pack yet: named, never silently passed.
    "dart", "lua", "ex", "exs", "hs", "zig", "erl", "clj", "fs", "jl", "nim",
];

pub fn language_for(path: &str) -> Option<Language> {
    match extension(path)? {
        "rs" => Some(Language::Rust),
        "py" | "pyi" => Some(Language::Python),
        "js" | "jsx" | "mjs" | "cjs" => Some(Language::JavaScript),
        "ts" | "tsx" | "mts" | "cts" => Some(Language::TypeScript),
        "java" => Some(Language::Java),
        "go" => Some(Language::Go),
        "php" | "phtml" | "inc" => Some(Language::Php),
        "c" | "h" => Some(Language::C),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => Some(Language::Cpp),
        "cs" => Some(Language::CSharp),
        "rb" | "rake" | "gemspec" => Some(Language::Ruby),
        "kt" | "kts" => Some(Language::Kotlin),
        "swift" => Some(Language::Swift),
        "scala" | "sc" => Some(Language::Scala),
        "m" | "mm" => Some(Language::ObjectiveC),
        _ => None,
    }
}

pub fn is_unsupported_source(path: &str) -> bool {
    is_unsupported_source_in(path, &default_registry())
}

pub fn is_unsupported_source_in(path: &str, registry: &LanguageRegistry) -> bool {
    if registry.is_supported(path) {
        return false;
    }
    extension(path).is_some_and(|e| UNSUPPORTED_SOURCE_EXTS.contains(&e))
}

pub fn extension(path: &str) -> Option<&str> {
    let name = path.rsplit('/').next()?;
    name.rsplit_once('.').map(|(_, ext)| ext)
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TestFn {
    /// Module-qualified name or test identity, e.g. `tests::inserts_in_order`.
    pub name: String,
    /// 1-based line of the test definition.
    pub line: usize,
    /// 1-based line of the end of the test definition (or 0 if not tracked).
    pub end_line: usize,
    pub total_asserts: usize,
    /// Equality / pattern assertions (`assert_eq!`, `assert_ne!`, `assert_matches!` ...).
    pub strong_asserts: usize,
    pub tautologies: usize,
    pub ignored: bool,
    /// Specific conditional predicate (e.g. `miri`, `target_os = "..."`), or `None` if unconditionally ignored.
    pub conditional_ignore: Option<String>,
    /// Fatal assertions that abort execution on failure (e.g. `require.*`, `ASSERT_*`).
    pub fatal_asserts: usize,
    pub should_panic: bool,
    /// Test doubles constructed or programmed in the body (`Mock()`, `jest.fn()`, `when(`).
    pub mock_setups: usize,
    /// Assertions on a double's interactions (`assert_called_with`, `toHaveBeenCalled`).
    pub mock_asserts: usize,
    /// A retry / flaky marker on the test (`@pytest.mark.flaky`, `jest.retryTimes`).
    pub retries: Option<String>,
    /// Hard-coded delays in the body (`thread::sleep`, `time.sleep`, `setTimeout`).
    pub sleeps: usize,
    /// Assertions that hold for nearly any value (`is not None`, `toBeDefined`, `is_ok()`).
    pub trivial_asserts: usize,
    /// Calls to same-file helpers whose body has a failure path (an assertion, or a
    /// `raise` / `throw` / `panic!` on a failure branch). `assertion-reduction` reads a
    /// drop that coincides with more of these as checks moved into helpers.
    pub helper_checks: usize,
    /// Numeric bounds of its assertions (`super::bounds`), paired by skeleton across a
    /// change so a bound moved the loose way is seen although the count is unchanged.
    pub bounds: Vec<bounds::Bound>,
}

impl TestFn {
    /// Assertions that can actually fail.
    pub fn effective_asserts(&self) -> usize {
        self.total_asserts.saturating_sub(self.tautologies)
    }

    pub fn is_vacuous(&self) -> bool {
        self.effective_asserts() == 0 && !self.should_panic
    }
}

/// Aggregated assertion facts for non-test helper functions resolved in the same file.
#[derive(Debug, Clone, Default)]
pub struct HelperFacts {
    pub total_asserts: usize,
    pub strong_asserts: usize,
    pub tautologies: usize,
    pub fatal_asserts: usize,
    /// The one same-file function this helper's whole body calls, when it is a thin
    /// wrapper ([`thin_wrapper_callee`]); its checks are resolved as the wrapper's.
    pub wraps: Option<String>,
}

/// How many calls deep a test's same-file helpers are followed: a C or C++ test `main`
/// drives check functions that call one `require`-style helper that aborts, and a
/// script's `self_test` calls a function that calls the validator that raises.
pub const HELPER_DEPTH: usize = 3;

/// How many thin wrappers are followed from one helper call (`install` ->
/// `install_with` -> `install_in`); a wrapper hop costs no call level.
pub const WRAPPER_DEPTH: usize = 3;

/// A same-file helper's checks with those of the helpers it calls, up to `HELPER_DEPTH`
/// levels; a recursive call is not followed again. `None` when `name` is not a helper.
/// A thin wrapper resolves through to the function it wraps without spending a level.
pub fn transitive_helper(
    name: &str,
    helpers: &std::collections::HashMap<String, HelperFacts>,
    calls: &std::collections::HashMap<String, Vec<String>>,
    path: &mut Vec<String>,
) -> Option<HelperFacts> {
    resolve_helper(name, helpers, calls, path, 0, 0)
}

/// A one-level pack's helper: its own checks, or, for a thin wrapper, those of the
/// function it wraps (followed up to `WRAPPER_DEPTH` wrappers, a cycle stopped).
/// `None` when `name` is not a helper.
pub fn helper_through_wrappers(
    name: &str,
    helpers: &std::collections::HashMap<String, HelperFacts>,
) -> Option<HelperFacts> {
    let no_calls = std::collections::HashMap::new();
    // Level `HELPER_DEPTH - 1` follows no ordinary call: only wrapper hops.
    resolve_helper(
        name,
        helpers,
        &no_calls,
        &mut Vec::new(),
        HELPER_DEPTH - 1,
        0,
    )
}

fn resolve_helper(
    name: &str,
    helpers: &std::collections::HashMap<String, HelperFacts>,
    calls: &std::collections::HashMap<String, Vec<String>>,
    path: &mut Vec<String>,
    level: usize,
    hops: usize,
) -> Option<HelperFacts> {
    let own = helpers.get(name)?;
    if path.iter().any(|p| p == name) {
        return None;
    }
    let mut out = HelperFacts {
        total_asserts: own.total_asserts,
        strong_asserts: own.strong_asserts,
        tautologies: own.tautologies,
        fatal_asserts: own.fatal_asserts,
        wraps: None,
    };
    let add = |out: &mut HelperFacts, sub: HelperFacts| {
        out.total_asserts += sub.total_asserts;
        out.strong_asserts += sub.strong_asserts;
        out.tautologies += sub.tautologies;
        out.fatal_asserts += sub.fatal_asserts;
    };
    if let (Some(callee), true) = (&own.wraps, hops < WRAPPER_DEPTH) {
        path.push(name.to_string());
        let sub = resolve_helper(callee, helpers, calls, path, level, hops + 1);
        if let Some(sub) = sub {
            add(&mut out, sub);
        }
        // The calls computing a forwarding wrapper's locals are ordinary calls, followed
        // as a busy helper's are; the wrapped call itself is the hop above.
        if level + 1 < HELPER_DEPTH {
            let mut others: Vec<&String> = calls.get(name).into_iter().flatten().collect();
            if let Some(at) = others.iter().position(|c| *c == callee) {
                others.remove(at);
            }
            for other in others {
                if let Some(sub) = resolve_helper(other, helpers, calls, path, level + 1, hops) {
                    add(&mut out, sub);
                }
            }
        }
        path.pop();
        return Some(out);
    }
    if level + 1 < HELPER_DEPTH {
        path.push(name.to_string());
        for callee in calls.get(name).into_iter().flatten() {
            if let Some(sub) = resolve_helper(callee, helpers, calls, path, level + 1, hops) {
                add(&mut out, sub);
            }
        }
        path.pop();
    }
    Some(out)
}

/// How a pack's grammar spells a function body that is one call.
pub struct WrapperSpec {
    /// Bodies, statement lists and statements looked through when they hold exactly
    /// one named node: `{ f(x) }`, `return f(x)`, `f(x);`, `= f(x)`.
    pub through: &'static [&'static str],
    /// Call nodes.
    pub calls: &'static [&'static str],
    /// A call's argument containers and argument nodes, and collection literals,
    /// looked through to their parts (`(a, b)`, `name: a`, `&a`, `[]`, `[a, 1]`).
    pub arguments: &'static [&'static str],
    /// Unary kinds looked through to their operand only when their first token is
    /// the given operator: taking a reference (`&a`), not negating or receiving.
    pub references: &'static [(&'static str, &'static str)],
    /// Argument kinds besides identifiers and `*literal` kinds: a pass-through
    /// name or a literal (`true`, `None`, `self`).
    pub plain: &'static [&'static str],
    /// Comment kinds and operator nodes, skipped (Swift's `try`).
    pub skip: &'static [&'static str],
}

/// The body is a thin wrapper: once blocks, `return` and statement nodes holding a
/// single node are looked through, it is one call whose callee makes no call of its
/// own and whose other parts (its arguments, a method name, a trailing closure) are
/// all names passed through or literals. That call is then the only one in the body,
/// so the first of `callees` (the same-file calls the pack collected from the body)
/// names it; `None` when the pack collected none (a call on another object). A body
/// that does any other work, or makes any other call, is not a wrapper.
pub fn thin_wrapper_callee(
    body: tree_sitter::Node,
    spec: &WrapperSpec,
    callees: &[String],
) -> Option<String> {
    wrapper_call(body, spec, None)?;
    callees.first().cloned()
}

/// How a pack's grammar spells a local a wrapper computes before its call:
/// `let bin = locate(dir);`, `b = loc(d)`, `const b = loc(d);`, `int b = loc(d);`.
pub struct LocalSpec {
    /// Statements holding a binding: one of them is a binding when exactly one
    /// `binders` node sits in it (`const b = 1;`, not `const a = 1, b = 2;`).
    pub statements: &'static [&'static str],
    /// Nodes binding one name to one value.
    pub binders: &'static [&'static str],
    /// Fields, or else child kinds, holding what a binder binds; the first present is
    /// read, and a field given twice (`var a, b = ..`) is not one name.
    pub pattern: &'static [&'static str],
    /// Fields holding the value; empty when the grammar names none, and then the value
    /// is the binder's last named child, after the pattern.
    pub value: &'static [&'static str],
    /// Name kinds: what is bound, and what reads it.
    pub names: &'static [&'static str],
    /// Pattern kinds looked through to the one name they hold (`$b`, Go's `b :=`
    /// expression list, Kotlin's `val b: Int`, Swift's `let b`).
    pub holders: &'static [&'static str],
    /// Fields or child kinds that make a binding more than a local
    /// (`let x = y else { .. }`, a Kotlin getter or delegate).
    pub refused: &'static [&'static str],
}

/// A [`thin_wrapper_callee`] that may first compute locals and forward them with its
/// own parameters: `{ let bin = locate(dir).unwrap(); run_in(dir, &bin) }`. Before
/// the call the body holds only bindings of one name each, and every name bound is
/// forwarded to the call or used by a later binding; a binding nothing reads, or any
/// other statement, is work, and the body is not a wrapper. The call is then the last
/// one in the body, so the last of `callees` names it, provided that name is in the
/// call (a call the pack did not collect leaves an earlier one last).
pub fn forwarding_wrapper_callee(
    body: tree_sitter::Node,
    spec: &WrapperSpec,
    locals: &LocalSpec,
    callees: &[String],
    src: &[u8],
) -> Option<String> {
    let (call, bindings) = wrapper_call(body, spec, Some(locals))?;
    if bindings.is_empty() {
        return callees.first().cloned();
    }
    let bound: Vec<(&str, tree_sitter::Node)> = bindings
        .iter()
        .map(|b| bound_local(*b, locals, src))
        .collect::<Option<_>>()?;
    for (i, (name, _)) in bound.iter().enumerate() {
        let mut readers = bound[i + 1..].iter().map(|(_, value)| *value).chain([call]);
        if !readers.any(|n| reads_name(n, name, locals.names, src)) {
            return None;
        }
    }
    // A pack may key a method by its scope (`Class::method`, `self.method`).
    let last = callees.last()?;
    let short = last.rsplit("::").next()?.rsplit('.').next()?;
    names_word(call.utf8_text(src).ok()?, short).then(|| last.clone())
}

/// The binder a statement holds, when it is a binding.
fn binder_of<'t>(node: tree_sitter::Node<'t>, locals: &LocalSpec) -> Option<tree_sitter::Node<'t>> {
    if locals.binders.contains(&node.kind()) {
        return Some(node);
    }
    if !locals.statements.contains(&node.kind()) {
        return None;
    }
    fn collect<'t>(
        n: tree_sitter::Node<'t>,
        locals: &LocalSpec,
        out: &mut Vec<tree_sitter::Node<'t>>,
    ) {
        let mut cursor = n.walk();
        for c in n.named_children(&mut cursor) {
            if locals.binders.contains(&c.kind()) {
                out.push(c);
            } else {
                collect(c, locals, out);
            }
        }
    }
    let mut found = Vec::new();
    collect(node, locals, &mut found);
    let [only] = found.as_slice() else {
        return None;
    };
    Some(*only)
}

/// The name a binding statement binds and the value it computes; `None` for a
/// pattern, a second name, no value, or a refused part.
fn bound_local<'t>(
    statement: tree_sitter::Node<'t>,
    locals: &LocalSpec,
    src: &'t [u8],
) -> Option<(&'t str, tree_sitter::Node<'t>)> {
    let binder = binder_of(statement, locals)?;
    let mut cursor = binder.walk();
    let children: Vec<tree_sitter::Node> = binder.named_children(&mut cursor).collect();
    let refused = locals.refused.iter().any(|r| {
        binder.child_by_field_name(r).is_some() || children.iter().any(|c| c.kind() == *r)
    });
    if refused {
        return None;
    }
    let pattern = locals.pattern.iter().find_map(|p| {
        let mut cursor = binder.walk();
        let fielded: Vec<_> = binder.children_by_field_name(p, &mut cursor).collect();
        match fielded.as_slice() {
            [one] => Some(Some(*one)),
            [] => children.iter().find(|c| c.kind() == *p).map(|c| Some(*c)),
            _ => Some(None),
        }
    })??;
    let name = if locals.names.contains(&pattern.kind()) {
        pattern
    } else if locals.holders.contains(&pattern.kind()) {
        let mut cursor = pattern.walk();
        let names: Vec<_> = pattern
            .named_children(&mut cursor)
            .filter(|c| locals.names.contains(&c.kind()))
            .collect();
        let [one] = names.as_slice() else {
            return None;
        };
        *one
    } else {
        return None;
    };
    let value = if locals.value.is_empty() {
        children
            .last()
            .filter(|v| v.start_byte() >= pattern.end_byte())
            .copied()
    } else {
        locals
            .value
            .iter()
            .find_map(|v| binder.child_by_field_name(v))
    }?;
    Some((name.utf8_text(src).ok()?, value))
}

/// The call a wrapper body comes down to, with the bindings (`locals`) that precede
/// it; `None` when the body is not one call forwarding names and literals.
fn wrapper_call<'t>(
    body: tree_sitter::Node<'t>,
    spec: &WrapperSpec,
    locals: Option<&LocalSpec>,
) -> Option<(tree_sitter::Node<'t>, Vec<tree_sitter::Node<'t>>)> {
    let mut node = body;
    let mut bindings = Vec::new();
    while !spec.calls.contains(&node.kind()) {
        if !spec.through.contains(&node.kind()) {
            return None;
        }
        let parts = wrapper_parts(node, spec);
        let bound = parts
            .iter()
            .take_while(|p| locals.is_some_and(|l| binder_of(**p, l).is_some()))
            .count();
        bindings.extend_from_slice(&parts[..bound]);
        let [only] = &parts[bound..] else {
            return None;
        };
        node = *only;
    }
    let parts = wrapper_parts(node, spec);
    let (callee, rest) = parts.split_first()?;
    let thin = !makes_a_call(*callee, spec) && rest.iter().all(|c| plain_argument(*c, spec));
    thin.then_some((node, bindings))
}

/// Fields that name a member, a method, a selector part or an argument label: an
/// identifier there (`x.b`, `[self run:x b:y]`, `run(b: x)`) is not a read of `b`.
const NOT_READS: &[&str] = &["method", "name", "field", "property", "attribute", "suffix"];

fn reads_name(node: tree_sitter::Node, name: &str, names: &[&str], src: &[u8]) -> bool {
    if names.contains(&node.kind()) && node.utf8_text(src).is_ok_and(|t| t == name) {
        return true;
    }
    (0..node.child_count()).any(|i| {
        let label = node
            .field_name_for_child(i as u32)
            .is_some_and(|f| NOT_READS.contains(&f));
        !label
            && node
                .child(i)
                .is_some_and(|c| reads_name(c, name, names, src))
    })
}

/// `word` occurs in `text` bounded by characters that cannot continue a name.
fn names_word(text: &str, word: &str) -> bool {
    let part = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    text.match_indices(word).any(|(at, _)| {
        !text[..at].chars().next_back().is_some_and(part)
            && !text[at + word.len()..].chars().next().is_some_and(part)
    })
}

fn makes_a_call(node: tree_sitter::Node, spec: &WrapperSpec) -> bool {
    let mut cursor = node.walk();
    let found = spec.calls.contains(&node.kind())
        || node
            .named_children(&mut cursor)
            .any(|c| makes_a_call(c, spec));
    found
}

fn wrapper_parts<'t>(
    node: tree_sitter::Node<'t>,
    spec: &WrapperSpec,
) -> Vec<tree_sitter::Node<'t>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|c| !spec.skip.contains(&c.kind()))
        .collect()
}

fn plain_argument(node: tree_sitter::Node, spec: &WrapperSpec) -> bool {
    let kind = node.kind();
    let reference = spec
        .references
        .iter()
        .any(|&(k, op)| k == kind && node.child(0).is_some_and(|c| c.kind() == op));
    if reference || spec.arguments.contains(&kind) {
        return wrapper_parts(node, spec)
            .into_iter()
            .all(|c| plain_argument(c, spec));
    }
    // A closure passed along is work the wrapper does, not a value it forwards.
    let closure = ["lambda", "func", "block", "closure"]
        .iter()
        .any(|k| kind.contains(k));
    if closure {
        return false;
    }
    if kind.ends_with("literal") {
        // A composite literal (`[]int{f()}`, `[f()]`, `@[f()]`) or an interpolated
        // string (`"\(f())"`, `"${f()}"`) can hold a call, which is work. A string's own
        // parts (`string_content`, `escape_sequence`) are not held to `plain_argument`.
        return !makes_a_call(node, spec);
    }
    kind.ends_with("identifier") || spec.plain.contains(&kind)
}

/// Failure exits in a helper body: nodes of one of `kinds` whose text starts with one of
/// `prefixes` (an empty prefix list accepts any text; a prefix ending in a space also
/// matches the bare keyword). Bodies of nested functions
/// (`nested`) are skipped: defining a closure runs none of its statements.
pub fn count_failure_exits(
    node: tree_sitter::Node,
    src: &[u8],
    kinds: &[&str],
    prefixes: &[&str],
    nested: &[&str],
) -> usize {
    let mut n = 0;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if nested.contains(&child.kind()) {
            continue;
        }
        if kinds.contains(&child.kind()) {
            let t = child.utf8_text(src).unwrap_or("");
            if prefixes.is_empty()
                || prefixes
                    .iter()
                    .any(|p| t.starts_with(p) || t == p.trim_end())
            {
                n += 1;
                continue;
            }
        }
        n += count_failure_exits(child, src, kinds, prefixes, nested);
    }
    n
}

/// Where a pack's test bodies name functions they run through a dispatch table.
pub struct DispatchSpec {
    /// Array / list literal kinds (`[check_a, check_b]`, `%i[check_a]`).
    pub containers: &'static [&'static str],
    /// Name kinds that count when their parent or grandparent is a container.
    pub names: &'static [&'static str],
    /// Method-reference kinds (`this::checkA`, `::checkA`); the last name child counts.
    pub references: &'static [&'static str],
}

/// Names a test body runs through a dispatch table: `for f in [check_a, check_b] { f() }`,
/// `[checkA, checkB].forEach(f => f())`, `List.of(this::checkA)`. Each name is resolved
/// like a direct call (an unknown name resolves to nothing).
pub fn dispatch_calls(
    node: tree_sitter::Node,
    src: &[u8],
    spec: &DispatchSpec,
    out: &mut Vec<String>,
) {
    let text = |n: tree_sitter::Node| n.utf8_text(src).unwrap_or("").to_string();
    let kind = node.kind();
    if spec.references.contains(&kind) {
        let mut cursor = node.walk();
        let last = node.named_children(&mut cursor).last();
        if let Some(name) = last {
            out.push(text(name));
        }
        return;
    }
    if spec.names.contains(&kind) {
        let in_container =
            |n: Option<tree_sitter::Node>| n.is_some_and(|p| spec.containers.contains(&p.kind()));
        let parent = node.parent();
        if in_container(parent) || in_container(parent.and_then(|p| p.parent())) {
            out.push(text(node).trim_start_matches(':').to_string());
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        dispatch_calls(child, src, spec, out);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsafeSite {
    pub line: usize,
    pub kind: &'static str,
    pub documented: bool,
    pub snippet: String,
}

/// Generalized escape hatch site (unsafe blocks, type ignores, linter suppressions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EscapeHatchSite {
    UnsafeBlock {
        line: usize,
        kind: &'static str,
        documented: bool,
        snippet: String,
    },
    TypeIgnore {
        line: usize,
        tool: String,
        snippet: String,
    },
    LinterDisable {
        line: usize,
        rule: String,
        snippet: String,
    },
}

#[derive(Debug, Clone)]
pub struct ParsedFileFacts {
    pub tests: Vec<TestFn>,
    pub unsafe_sites: Vec<UnsafeSite>,
    pub escape_hatches: Vec<EscapeHatchSite>,
    /// Every function with a body, and what the body amounts to (`Fact::Functions`).
    pub functions: Vec<functions::FunctionFacts>,
    /// Error handlers that swallow, and discarded results, outside tests (`Fact::Handlers`).
    pub swallowed: Vec<handlers::SwallowSite>,
    /// Comments, docstrings and string literals (`Fact::Prose`).
    pub prose: Vec<prose::ProseSpan>,
    /// Testing-effort budgets in configuration positions (`Fact::Budgets`).
    pub budgets: Vec<budgets::BudgetSite>,
    /// Number of compile-time assertions outside tests (e.g. `const _: () = assert!(...)`, `static_assert`).
    pub compile_time_asserts: usize,
    /// Line of the first compile-time assertion (if any).
    pub compile_time_assert_line: Option<usize>,
    /// Synthesized test representing compile-time assertions for pair matching.
    pub compile_time_test: Option<TestFn>,
    /// The grammar could not parse part of the file; facts may be incomplete.
    pub has_parse_errors: bool,
    /// Line of the first parse or preprocessor syntax error (if any).
    pub first_parse_error_line: Option<usize>,
    /// Number of skipped ERROR/MISSING regions in the AST.
    pub skipped_error_nodes_count: usize,
}

impl Default for ParsedFileFacts {
    fn default() -> Self {
        Self {
            tests: Vec::new(),
            unsafe_sites: Vec::new(),
            escape_hatches: Vec::new(),
            functions: Vec::new(),
            swallowed: Vec::new(),
            prose: Vec::new(),
            budgets: Vec::new(),
            compile_time_asserts: 0,
            compile_time_assert_line: None,
            compile_time_test: Some(TestFn {
                name: "compile-time-assertions".to_string(),
                line: 1,
                end_line: 1,
                total_asserts: 0,
                strong_asserts: 0,
                tautologies: 0,
                ignored: false,
                conditional_ignore: None,
                fatal_asserts: 0,
                should_panic: false,
                mock_setups: 0,
                mock_asserts: 0,
                retries: None,
                sleeps: 0,
                trivial_asserts: 0,
                helper_checks: 0,
                bounds: Vec::new(),
            }),
            has_parse_errors: false,
            first_parse_error_line: None,
            skipped_error_nodes_count: 0,
        }
    }
}

impl ParsedFileFacts {
    /// Builds the synthesized `compile-time-assertions` test from facts.
    pub fn build_compile_time_test(&mut self) {
        let line = self.compile_time_assert_line.unwrap_or(1);
        self.compile_time_test = Some(TestFn {
            name: "compile-time-assertions".to_string(),
            line,
            end_line: line,
            total_asserts: self.compile_time_asserts,
            strong_asserts: self.compile_time_asserts,
            tautologies: 0,
            ignored: false,
            conditional_ignore: None,
            fatal_asserts: 0,
            should_panic: false,
            mock_setups: 0,
            mock_asserts: 0,
            retries: None,
            sleeps: 0,
            trivial_asserts: 0,
            helper_checks: 0,
            bounds: Vec::new(),
        });
    }
}

/// Helper to collect syntax error regions and first error line from an AST root.
pub fn collect_error_nodes_info(root: tree_sitter::Node) -> (bool, Option<usize>, usize) {
    if !root.has_error() {
        return (false, None, 0);
    }
    let mut first_line = None;
    let mut count = 0;
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.is_error() || node.is_missing() {
            count += 1;
            let line = node.start_position().row + 1;
            if first_line.is_none() || Some(line) < first_line {
                first_line = Some(line);
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.has_error() || child.is_error() || child.is_missing() {
                stack.push(child);
            }
        }
    }
    (true, first_line.or(Some(1)), count.max(1))
}

/// Backwards compatibility alias for [`ParsedFileFacts`].
pub type RustFacts = ParsedFileFacts;

#[derive(Debug, Clone, Default)]
pub struct AssertVocabulary {
    pub extra_macros: Vec<String>,
    pub helper_fns: Vec<String>,
    pub safety_placeholders: Vec<String>,
    /// Callee fragments that construct or program a test double, beyond the built-in list.
    pub mock_setup_fns: Vec<String>,
    /// Callee fragments that assert on a double's interactions, beyond the built-in list.
    pub mock_assert_fns: Vec<String>,
    /// Function names that are test entry points wherever they appear (`[tests].functions`).
    pub test_functions: Vec<String>,
    /// Path globs whose every line is test scope (`[tests].paths`).
    pub test_paths: Vec<String>,
    /// C / C++ macros blanked before parsing, beyond the built-in list
    /// (`[languages.c].macros`).
    pub c_macros: Vec<String>,
    /// C / C++ macros that expand to a function head, beyond the built-in list
    /// (`[languages.c].function_macros`).
    pub c_function_macros: Vec<String>,
}

/// Top-level helper to analyze Rust code directly.
pub fn analyze(source: &str, vocab: &AssertVocabulary) -> Result<ParsedFileFacts> {
    #[cfg(feature = "lang-rust")]
    {
        rust::RustPack.extract("test.rs", source, vocab)
    }
    #[cfg(not(feature = "lang-rust"))]
    {
        let _ = (source, vocab);
        anyhow::bail!("lang-rust feature is not enabled")
    }
}

/// The thin-wrapper rule, read the same way for each pack. The source defines a helper
/// with one failure exit and tests whose names carry a marker, in this order:
/// `direct` calls it; `via` calls a wrapper around it (adding a literal argument);
/// `hollow` calls a wrapper around a function with no check; `busy` calls a helper
/// that calls it and also does other work, which is not a wrapper; `cycle` calls one
/// of two wrappers around each other. Returns each test's `(total_asserts,
/// helper_checks)`.
#[cfg(test)]
pub(crate) fn thin_wrapper_counts(
    pack: &dyn LanguagePack,
    path: &str,
    src: &str,
) -> [(usize, usize); 5] {
    let facts = pack
        .extract(path, src, &AssertVocabulary::default())
        .expect(path);
    ["direct", "via", "hollow", "busy", "cycle"].map(|marker| {
        let t = facts
            .tests
            .iter()
            .find(|t| t.name.to_lowercase().contains(marker))
            .unwrap_or_else(|| panic!("{path}: no `{marker}` test in {:?}", facts.tests));
        (t.total_asserts, t.helper_checks)
    })
}

/// What [`thin_wrapper_counts`] reads in a one-level pack: the wrapper gets the
/// credit of the direct call, a hollow wrapper and a cycle get none, and the busy
/// helper resolves one level (its callee's check is not reached).
#[cfg(test)]
pub(crate) const ONE_LEVEL_WRAPPER_COUNTS: [(usize, usize); 5] =
    [(1, 1), (1, 1), (0, 0), (0, 0), (0, 0)];

/// The same in C/C++ and Python, which follow ordinary calls: the busy helper reaches
/// its callee's check as a call one level down.
#[cfg(test)]
pub(crate) const TRANSITIVE_WRAPPER_COUNTS: [(usize, usize); 5] =
    [(1, 1), (1, 1), (0, 0), (1, 1), (0, 0)];

#[cfg(test)]
mod tests {
    use super::*;

    struct MockCustomPack;

    impl LanguagePack for MockCustomPack {
        fn id(&self) -> &'static str {
            "mock-xyz"
        }

        fn name(&self) -> &'static str {
            "Mock XYZ"
        }

        fn matches(&self, path: &str) -> bool {
            extension(path) == Some("xyz")
        }

        fn extract(
            &self,
            _path: &str,
            src: &str,
            _vocab: &AssertVocabulary,
        ) -> Result<ParsedFileFacts> {
            let mut facts = ParsedFileFacts::default();
            if src.contains("test_case") {
                facts.tests.push(TestFn {
                    name: "mock_test".to_string(),
                    line: 1,
                    total_asserts: 1,
                    strong_asserts: 1,
                    tautologies: 0,
                    ignored: false,
                    should_panic: false,
                    mock_setups: 0,
                    mock_asserts: 0,
                    retries: None,
                    sleeps: 0,
                    trivial_asserts: 0,
                    ..Default::default()
                });
            }
            Ok(facts)
        }
    }

    #[test]
    fn language_dispatch_is_by_extension() {
        assert_eq!(language_for("src/a.rs"), Some(Language::Rust));
        assert_eq!(language_for("src/a.py"), Some(Language::Python));
        assert_eq!(language_for("src/a.js"), Some(Language::JavaScript));
        assert_eq!(language_for("web/App.tsx"), Some(Language::TypeScript));
        assert_eq!(language_for("service.java"), Some(Language::Java));
        assert_eq!(language_for("src/a.go"), Some(Language::Go));
        assert_eq!(language_for("src/a.php"), Some(Language::Php));
        assert_eq!(language_for("src/a.c"), Some(Language::C));
        assert_eq!(language_for("src/a.cpp"), Some(Language::Cpp));
        assert_eq!(language_for("src/a.cs"), Some(Language::CSharp));
        assert_eq!(language_for("src/a.rb"), Some(Language::Ruby));
        assert!(!is_unsupported_source("pkg/mod/a.py"));
        assert!(!is_unsupported_source("web/App.tsx"));
        assert!(!is_unsupported_source("service.java"));
        assert!(!is_unsupported_source("src/a.go"));
        assert!(!is_unsupported_source("src/a.php"));
        assert!(!is_unsupported_source("src/a.c"));
        assert!(!is_unsupported_source("src/a.cpp"));
        assert!(!is_unsupported_source("src/a.cs"));
        assert!(!is_unsupported_source("src/a.rb"));
        assert!(!is_unsupported_source("service.kt"));
        assert!(!is_unsupported_source("build.gradle.kts"));
        assert!(!is_unsupported_source("service.swift"));
        assert!(!is_unsupported_source("service.scala"));
        assert!(!is_unsupported_source("service.m"));
        assert!(!is_unsupported_source("service.mm"));
        assert!(is_unsupported_source("lib/widget.dart"));
        assert!(is_unsupported_source("src/app.lua"));
        assert!(!is_unsupported_source("src/a.rs"));
        assert!(!is_unsupported_source("tests/001.phpt"));
        assert!(!is_unsupported_source("docs/plan.md"));
        assert!(!is_unsupported_source("Makefile"));
    }

    /// A same-file helper that fails by raising, throwing or panicking on a mismatch is a
    /// check at each call; a helper with no failure exit is not.
    #[test]
    fn a_failing_helper_is_a_check_in_every_pack_that_resolves_helpers() {
        let cases: &[(&str, &str)] = &[
            (
                "tests/check.rs",
                "fn check(a: u32, b: u32) {\n    if a != b {\n        panic!(\"mismatch\");\n    }\n}\nfn noop(_a: u32) {}\n#[test]\nfn t() {\n    check(1, 1);\n    noop(2);\n}\n",
            ),
            (
                "pkg/a_test.go",
                "package a\nimport \"testing\"\nfunc check(a, b int) {\n\tif a != b {\n\t\tpanic(\"mismatch\")\n\t}\n}\nfunc noop(a int) {}\nfunc TestT(t *testing.T) {\n\tcheck(1, 1)\n\tnoop(2)\n}\n",
            ),
            (
                "src/test/java/ATest.java",
                "class ATest {\n  void check(int a, int b) {\n    if (a != b) { throw new IllegalStateException(\"mismatch\"); }\n  }\n  void noop(int a) {}\n  @Test\n  void t() {\n    check(1, 1);\n    noop(2);\n  }\n}\n",
            ),
            (
                "tests/ATest.cs",
                "public class ATest {\n  void Check(int a, int b) {\n    if (a != b) { throw new System.Exception(\"mismatch\"); }\n  }\n  void Noop(int a) {}\n  [Fact]\n  public void T() {\n    Check(1, 1);\n    Noop(2);\n  }\n}\n",
            ),
            (
                "src/test/kotlin/ATest.kt",
                "class ATest {\n    private fun check(a: Int, b: Int) {\n        if (a != b) throw IllegalStateException(\"mismatch\")\n    }\n    private fun noop(a: Int) {}\n    @Test\n    fun t() {\n        check(1, 1)\n        noop(2)\n    }\n}\n",
            ),
            (
                "test/a_test.rb",
                "class ATest < Minitest::Test\n  def check(a, b)\n    raise ArgumentError, \"mismatch\" if a != b\n  end\n\n  def noop(a)\n  end\n\n  def test_t\n    check(1, 1)\n    noop(2)\n  end\nend\n",
            ),
            (
                "tests/test_a.py",
                "def check(a, b):\n    if a != b:\n        raise ValueError(\"mismatch\")\n\ndef noop(a):\n    pass\n\ndef test_t():\n    check(1, 1)\n    noop(2)\n",
            ),
        ];
        let reg = default_registry();
        let vocab = AssertVocabulary::default();
        for (path, src) in cases {
            let Some(pack) = reg.find_pack(path) else {
                continue;
            };
            let facts = pack.extract(path, src, &vocab).expect(path);
            assert_eq!(facts.tests.len(), 1, "{path}");
            let t = &facts.tests[0];
            assert_eq!((t.total_asserts, t.helper_checks), (1, 1), "{path}: {t:?}");
        }
    }

    fn helper(total: usize, wraps: Option<&str>) -> HelperFacts {
        HelperFacts {
            total_asserts: total,
            wraps: wraps.map(str::to_string),
            ..Default::default()
        }
    }

    /// Wrapper hops are bounded by `WRAPPER_DEPTH` and a wrapper cycle stops; they do
    /// not spend `HELPER_DEPTH` levels in the packs that follow ordinary calls.
    #[test]
    fn wrappers_resolve_to_their_callee_bounded_and_cycle_safe() {
        let helpers: std::collections::HashMap<String, HelperFacts> = [
            ("check", helper(2, None)),
            ("w1", helper(0, Some("check"))),
            ("w2", helper(0, Some("w1"))),
            ("w3", helper(0, Some("w2"))),
            ("w4", helper(0, Some("w3"))),
            ("ping", helper(0, Some("pong"))),
            ("pong", helper(0, Some("ping"))),
            ("gone", helper(0, Some("missing"))),
            ("outer", helper(0, None)),
            ("wa", helper(0, Some("wb"))),
            ("wb", helper(0, Some("busy"))),
            ("busy", helper(0, None)),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let total = |name: &str| helper_through_wrappers(name, &helpers).map(|h| h.total_asserts);
        assert_eq!(total("w1"), Some(2));
        assert_eq!(total("w3"), Some(2), "three wrapper hops are followed");
        assert_eq!(total("w4"), Some(0), "a fourth hop is not");
        assert_eq!(total("ping"), Some(0), "a cycle stops");
        assert_eq!(
            total("gone"),
            Some(0),
            "a callee that is not a helper adds nothing"
        );
        assert_eq!(total("nope"), None);
        // In a pack that follows calls, `outer -> wa -> wb -> busy -> check` spends two
        // levels (outer, busy): the wrapper hops are free, so `busy` may still follow
        // its call to `check`.
        let calls: std::collections::HashMap<String, Vec<String>> = [
            ("outer".to_string(), vec!["wa".to_string()]),
            ("busy".to_string(), vec!["check".to_string()]),
        ]
        .into_iter()
        .collect();
        let deep = |name: &str| {
            transitive_helper(name, &helpers, &calls, &mut Vec::new()).map(|h| h.total_asserts)
        };
        assert_eq!(deep("outer"), Some(2));
        assert_eq!(deep("ping"), Some(0));
    }

    /// What `thin_wrapper_callee` reads as a wrapper, on Rust bodies; each pack differs
    /// only in its `WrapperSpec` node kinds.
    #[cfg(feature = "lang-rust")]
    #[test]
    fn a_thin_wrapper_is_one_call_forwarding_names_and_literals() {
        let callee = |body: &str, callees: &[&str]| {
            let src = format!("fn w(x: u32) {body}");
            let mut parser = tree_sitter::Parser::new();
            parser
                .set_language(&tree_sitter_rust::LANGUAGE.into())
                .unwrap();
            let tree = parser.parse(&src, None).unwrap();
            let f = tree.root_node().named_child(0).unwrap();
            let callees: Vec<String> = callees.iter().map(|c| c.to_string()).collect();
            thin_wrapper_callee(
                f.child_by_field_name("body").unwrap(),
                &rust::RS_WRAPPER,
                &callees,
            )
        };
        let check = Some("check".to_string());
        assert_eq!(callee("{ check(x, true) }", &["check"]), check);
        assert_eq!(callee("{ return check(x, &y, 1); }", &["check"]), check);
        assert_eq!(callee("{ check(x)? } // forwarded", &["check"]), check);
        // References to names and literals, and collection literals of them.
        assert_eq!(callee("{ check(x, &[]) }", &["check"]), check);
        assert_eq!(
            callee("{ check(&[1, 2], &\"x\", &mut x) }", &["check"]),
            check
        );
        assert_eq!(callee("{ check((x, 1), (), [x; 3]) }", &["check"]), check);
        // Other work besides the call, even with no other call.
        assert_eq!(callee("{ let y = x; check(y) }", &["check"]), None);
        // A computed argument, a closure argument, a callee that calls.
        assert_eq!(callee("{ check(x + 1) }", &["check"]), None);
        assert_eq!(callee("{ check(x, || true) }", &["check"]), None);
        // A collection or reference built from a computed value; a macro, which can
        // expand to anything.
        assert_eq!(callee("{ check(x, &[g()]) }", &["check"]), None);
        assert_eq!(callee("{ check(&(x + 1)) }", &["check"]), None);
        assert_eq!(callee("{ check(x, vec![]) }", &["check"]), None);
        assert_eq!(callee("{ make().check(x) }", &["make"]), None);
        // A call the pack did not collect as a same-file call.
        assert_eq!(callee("{ other.check(x) }", &[]), None);
        assert_eq!(callee("{}", &[]), None);
    }

    /// What `forwarding_wrapper_callee` reads as a wrapper on Rust bodies: one call
    /// after bindings of one name each, every name read by the call or a later binding.
    #[cfg(feature = "lang-rust")]
    #[test]
    fn a_forwarding_wrapper_computes_only_the_locals_it_forwards() {
        let callee = |body: &str, callees: &[&str]| {
            let src = format!("fn w(x: u32) {body}");
            let mut parser = tree_sitter::Parser::new();
            parser
                .set_language(&tree_sitter_rust::LANGUAGE.into())
                .unwrap();
            let tree = parser.parse(&src, None).unwrap();
            let f = tree.root_node().named_child(0).unwrap();
            let callees: Vec<String> = callees.iter().map(|c| c.to_string()).collect();
            forwarding_wrapper_callee(
                f.child_by_field_name("body").unwrap(),
                &rust::RS_WRAPPER,
                &rust::RS_LOCALS,
                &callees,
                src.as_bytes(),
            )
        };
        let check = Some("check".to_string());
        // No binding: the thin-wrapper rule.
        assert_eq!(callee("{ check(x, true) }", &["check"]), check);
        assert_eq!(callee("{ check(x + 1) }", &["check"]), None);
        // Locals computed (calls included) and forwarded, directly or through another.
        assert_eq!(
            callee(
                "{ let y = g(x).unwrap(); check(x, &y) }",
                &["g", "unwrap", "check"]
            ),
            check
        );
        assert_eq!(
            callee(
                "{ let a = g(x); let mut b: u32 = a.h(); return check(x, b); }",
                &["g", "h", "check"]
            ),
            check
        );
        // A local nothing reads, a pattern, a `let`-`else`, a binding with no value.
        assert_eq!(callee("{ let y = g(x); check(x) }", &["g", "check"]), None);
        assert_eq!(
            callee("{ let (a, b) = g(x); check(a, b) }", &["g", "check"]),
            None
        );
        assert_eq!(
            callee(
                "{ let Some(y) = g(x) else { return }; check(y) }",
                &["g", "check"]
            ),
            None
        );
        assert_eq!(
            callee(
                "{ let y = g(x) else { return }; check(y) }",
                &["g", "check"]
            ),
            None
        );
        assert_eq!(callee("{ let y; check(y) }", &["check"]), None);
        // Other work before the call, or a call computing an argument.
        assert_eq!(
            callee("{ g(x); let y = h(x); check(y) }", &["g", "h", "check"]),
            None
        );
        assert_eq!(
            callee("{ let y = g(x); check(y.len()) }", &["g", "check", "len"]),
            None
        );
        // A final call the pack did not collect: the last collected one is not it.
        assert_eq!(callee("{ let y = g(x); other.check(y) }", &["g"]), None);
    }

    /// A literal forwarded by a wrapper is plain only when nothing in it calls: a
    /// composite literal (`[]int{f()}`, `[f()]`, `@[f()]`) or an interpolated string
    /// (`"\(f())"`, `"${f()}"`) holding a call is work, so its helper is not a wrapper.
    /// The same literals holding names, numbers and escapes stay plain. Each source's
    /// `plain` wrapper forwards the second kind and `calls` the first; one-level packs
    /// give a test calling a non-wrapper helper no credit.
    #[test]
    fn a_wrapper_forwarding_a_literal_that_calls_is_not_thin() {
        let cases: &[(&str, &str)] = &[
            (
                "pkg/wrap_test.go",
                "package a\nimport \"testing\"\nfunc checked(x int, xs []int, s string) { if x != 1 { panic(\"x\") } }\nfunc plain(x int) { checked(x, []int{1, x}, \"s\\n\") }\nfunc calls(x int) { checked(x, []int{f()}, \"s\") }\nfunc TestPlain(t *testing.T) { plain(1) }\nfunc TestCalls(t *testing.T) { calls(1) }\n",
            ),
            (
                "Tests/WrapTests.swift",
                "import XCTest\nfinal class WrapTests: XCTestCase {\n    func checked(_ x: Int, _ a: Any, _ s: String) { if x != 1 { fatalError(\"x\") } }\n    func plain(_ x: Int) { checked(x, [1, x], \"a\\(x)\\n\") }\n    func calls(_ x: Int) { checked(x, [f()], \"a\") }\n    func callsInDictionary(_ x: Int) { checked(x, [\"k\": f()], \"a\") }\n    func callsInString(_ x: Int) { checked(x, [x], \"a\\(f())\") }\n    func testPlain() { plain(1) }\n    func testCalls() { calls(1) }\n    func testCallsInDictionary() { callsInDictionary(1) }\n    func testCallsInString() { callsInString(1) }\n}\n",
            ),
            (
                "Tests/WrapTests.m",
                "@interface WrapTests : XCTestCase\n@end\n@implementation WrapTests\n- (void)checked:(int)x with:(id)a s:(id)s {\n    if (x != 1) { XCTFail(@\"x\"); }\n}\n- (void)plain:(int)x {\n    [self checked:x with:@[@1] s:@\"s\\n\"];\n}\n- (void)calls:(int)x {\n    [self checked:x with:@[f()] s:@\"s\"];\n}\n- (void)callsInDictionary:(int)x {\n    [self checked:x with:@{@\"k\": f()} s:@\"s\"];\n}\n- (void)testPlain {\n    [self plain:1];\n}\n- (void)testCalls {\n    [self calls:1];\n}\n- (void)testCallsInDictionary {\n    [self callsInDictionary:1];\n}\n@end\n",
            ),
            (
                "src/test/kotlin/WrapTest.kt",
                "class WrapTest {\n    private fun checked(x: Int, s: String) { if (x != 1) throw IllegalStateException(\"x\") }\n    private fun plain(x: Int) = checked(x, \"a$x\\n\")\n    private fun calls(x: Int) = checked(x, \"a${f()}\")\n    @Test\n    fun testPlain() { plain(1) }\n    @Test\n    fun testCalls() { calls(1) }\n}\n",
            ),
        ];
        let reg = default_registry();
        let vocab = AssertVocabulary::default();
        let mut wrong = Vec::new();
        for (path, src) in cases {
            let Some(pack) = reg.find_pack(path) else {
                continue;
            };
            let facts = pack.extract(path, src, &vocab).expect(path);
            let mut seen = 0;
            for t in &facts.tests {
                let name = t.name.to_lowercase();
                let want = if name.contains("plain") {
                    (1, 1)
                } else if name.contains("calls") {
                    (0, 0)
                } else {
                    continue;
                };
                seen += 1;
                if (t.total_asserts, t.helper_checks) != want {
                    wrong.push(format!("{path} {}: {t:?}", t.name));
                }
            }
            assert!(seen >= 2, "{path}: {:?}", facts.tests);
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    /// A wrapper forwarding a reference or a collection literal (`&x`, `[]`, `{a: 1}`)
    /// is thin in every pack whose grammar has one; a collection holding a call, or a
    /// unary operator other than `&`, is work. C/C++ and Python follow ordinary calls
    /// three levels, so there the wrapper sits under two busy helpers: its hop is free
    /// only when it is thin.
    #[test]
    fn a_wrapper_forwarding_a_reference_or_collection_literal_is_thin() {
        let cases: &[(&str, &str)] = &[
            (
                "tests/t.rs",
                "fn checked(x: u32, env: &[(&str, &str)]) { if x != env.len() as u32 { panic!(\"x\"); } }\nfn forwarded(x: u32) { checked(x, &[]) }\nfn computed(x: u32) { checked(x, &[g()]) }\n#[test]\nfn t_forwarded() { forwarded(1); }\n#[test]\nfn t_computed() { computed(1); }\n",
            ),
            (
                "tests/t_test.cc",
                "namespace {\nvoid Checked(int* x, std::vector<int> v) { if (*x != 1) std::abort(); }\nvoid Forwarded(int x) { Checked(&x, {}); }\nvoid Computed(int x) { Checked(&x, {G()}); }\nvoid B1(int x) { Setup(); Forwarded(x); }\nvoid A1(int x) { Setup(); B1(x); }\nvoid B2(int x) { Setup(); Computed(x); }\nvoid A2(int x) { Setup(); B2(x); }\n}  // namespace\nTEST(T, Forwarded) { A1(1); }\nTEST(T, Computed) { A2(1); }\n",
            ),
            (
                "tests/test_t.py",
                "def checked(x, *opts):\n    if x != 1:\n        raise ValueError(\"x\")\n\ndef forwarded(x):\n    return checked(x, [], (x, 1), {\"a\": x}, {1})\n\ndef computed(x):\n    return checked(x, [g()])\n\ndef b1(x):\n    setup()\n    forwarded(x)\n\ndef a1(x):\n    setup()\n    b1(x)\n\ndef b2(x):\n    setup()\n    computed(x)\n\ndef a2(x):\n    setup()\n    b2(x)\n\ndef test_forwarded():\n    a1(1)\n\ndef test_computed():\n    a2(1)\n",
            ),
            (
                "test/t.test.js",
                "function checked(x, ...opts) { if (x !== 1) { throw new Error('x'); } }\nfunction forwarded(x) { return checked(x, [], [1, x], { strict: true, x }); }\nfunction computed(x) { return checked(x, [g()]); }\ntest('forwarded', () => { forwarded(1); });\ntest('computed', () => { computed(1); });\n",
            ),
            (
                "pkg/t_test.go",
                "package a\n\nimport \"testing\"\n\nfunc checked(x *int, n int) {\n\tif *x != 1 {\n\t\tpanic(\"x\")\n\t}\n}\nfunc forwarded(x int) { checked(&x, 1) }\nfunc computed(x int, ch chan int) { checked(&x, <-ch) }\nfunc TestForwarded(t *testing.T) { forwarded(1) }\nfunc TestComputed(t *testing.T) { computed(1, nil) }\n",
            ),
            (
                "test/t_test.rb",
                "class TTest < Minitest::Test\n  def checked(x, *opts)\n    raise ArgumentError, 'x' if x != 1\n  end\n\n  def forwarded(x)\n    checked(x, [], [1, x], { strict: true })\n  end\n\n  def computed(x)\n    checked(x, [g(1)])\n  end\n\n  def test_forwarded\n    forwarded(1)\n  end\n\n  def test_computed\n    computed(1)\n  end\nend\n",
            ),
            (
                "tests/TTest.php",
                "<?php\nclass TTest extends TestCase {\n    private function checked(int $x, array $o): void { if ($x !== 1) { throw new RuntimeException('x'); } }\n    private function forwarded(int $x): void { $this->checked($x, [], ['strict' => true, $x]); }\n    private function computed(int $x): void { $this->checked($x, [g()]); }\n    public function testForwarded(): void { $this->forwarded(1); }\n    public function testComputed(): void { $this->computed(1); }\n}\n",
            ),
            (
                "Tests/TTests.swift",
                "import XCTest\n\nfinal class TTests: XCTestCase {\n    func checked(_ x: inout Int, _ n: Int) { if x != 1 { fatalError(\"x\") } }\n    func forwarded(_ x: inout Int) { checked(&x, 1) }\n    func computed(_ x: inout Int) { checked(&x, -x) }\n    func testForwarded() { var v = 1; forwarded(&v) }\n    func testComputed() { var v = 1; computed(&v) }\n}\n",
            ),
            (
                "Tests/TTests.m",
                "#import <XCTest/XCTest.h>\n@interface TTests : XCTestCase\n@end\n@implementation TTests\n- (void)checked:(int *)x n:(int)n {\n    if (*x != 1) { XCTFail(@\"x\"); }\n}\n- (void)forwarded:(int)x {\n    [self checked:&x n:1];\n}\n- (void)computed:(int)x {\n    [self checked:&x n:-x];\n}\n- (void)testForwarded {\n    [self forwarded:1];\n}\n- (void)testComputed {\n    [self computed:1];\n}\n@end\n",
            ),
        ];
        let reg = default_registry();
        let vocab = AssertVocabulary::default();
        let mut ran = 0;
        for (path, src) in cases {
            let Some(pack) = reg.find_pack(path) else {
                continue;
            };
            ran += 1;
            let facts = pack.extract(path, src, &vocab).expect(path);
            let counts = |marker: &str| {
                let t = facts
                    .tests
                    .iter()
                    .find(|t| t.name.to_lowercase().contains(marker))
                    .unwrap_or_else(|| panic!("{path}: no `{marker}` test in {:?}", facts.tests));
                (t.total_asserts, t.helper_checks)
            };
            assert_eq!(counts("forwarded"), (1, 1), "{path}");
            assert_eq!(counts("computed"), (0, 0), "{path}");
        }
        assert!(ran > 0, "no pack compiled in");
    }

    /// A wrapper may first compute locals it forwards, with its parameters, to its one
    /// call (`b = loc(x); checked(x, b)`), in every pack; a local nothing reads makes it
    /// an ordinary helper. C/C++ and Python sit the wrapper under two busy helpers, as
    /// above, so only a free wrapper hop reaches `checked`.
    #[test]
    fn a_wrapper_forwarding_a_computed_local_resolves_in_every_pack() {
        let cases: &[(&str, &str)] = &[
            (
                "tests/t.rs",
                "fn checked(x: u32, b: u32) { if x != b { panic!(\"x\"); } }\nfn forwards(x: u32) { let b = loc(x); checked(x, b) }\nfn unread(x: u32) { let b = loc(x); checked(x, x) }\n#[test]\nfn t_forwards() { forwards(1); }\n#[test]\nfn t_unread() { unread(1); }\n",
            ),
            (
                "tests/t_test.cc",
                "namespace {\nvoid Checked(int x, int b) { if (x != b) std::abort(); }\nvoid Forwards(int x) { auto b = Loc(x); Checked(x, b); }\nvoid Unread(int x) { auto b = Loc(x); Checked(x, x); }\nvoid B1(int x) { Setup(); Forwards(x); }\nvoid A1(int x) { Setup(); B1(x); }\nvoid B2(int x) { Setup(); Unread(x); }\nvoid A2(int x) { Setup(); B2(x); }\n}  // namespace\nTEST(T, Forwards) { A1(1); }\nTEST(T, Unread) { A2(1); }\n",
            ),
            (
                "tests/test_t.py",
                "def checked(x, b):\n    if x != b:\n        raise ValueError(\"x\")\n\ndef forwards(x):\n    b = loc(x)\n    return checked(x, b)\n\ndef unread(x):\n    b = loc(x)\n    return checked(x, x)\n\ndef b1(x):\n    setup()\n    forwards(x)\n\ndef a1(x):\n    setup()\n    b1(x)\n\ndef b2(x):\n    setup()\n    unread(x)\n\ndef a2(x):\n    setup()\n    b2(x)\n\ndef test_forwards():\n    a1(1)\n\ndef test_unread():\n    a2(1)\n",
            ),
            (
                "test/t.test.js",
                "function checked(x, b) { if (x !== b) { throw new Error('x'); } }\nfunction forwards(x) { const b = loc(x); let c = b; return checked(x, c); }\nfunction unread(x) { const b = loc(x); return checked(x, x); }\ntest('forwards', () => { forwards(1); });\ntest('unread', () => { unread(1); });\n",
            ),
            (
                "test/t.test.ts",
                "function checked(x: number, b: number) { if (x !== b) { throw new Error('x'); } }\nfunction forwards(x: number) { const b: number = loc(x); return checked(x, b); }\nfunction unread(x: number) { const b: number = loc(x); return checked(x, x); }\ntest('forwards', () => { forwards(1); });\ntest('unread', () => { unread(1); });\n",
            ),
            (
                "src/test/java/TTest.java",
                "class TTest {\n  void checked(int x, int b) { if (x != b) { throw new IllegalStateException(\"x\"); } }\n  void forwards(int x) { int b = loc(x); checked(x, b); }\n  void unread(int x) { int b = loc(x); checked(x, x); }\n  @Test void testForwards() { forwards(1); }\n  @Test void testUnread() { unread(1); }\n}\n",
            ),
            (
                "pkg/t_test.go",
                "package a\n\nimport \"testing\"\n\nfunc checked(x int, b int) {\n\tif x != b {\n\t\tpanic(\"x\")\n\t}\n}\nfunc forwards(x int) { b := loc(x); var c = b; checked(x, c) }\nfunc unread(x int) { b := loc(x); checked(x, x) }\nfunc TestForwards(t *testing.T) { forwards(1) }\nfunc TestUnread(t *testing.T) { unread(1) }\n",
            ),
            (
                "tests/TTest.php",
                "<?php\nclass TTest extends TestCase {\n    private function checked(int $x, int $b): void { if ($x !== $b) { throw new RuntimeException('x'); } }\n    private function forwards(int $x): void { $b = loc($x); $this->checked($x, $b); }\n    private function unread(int $x): void { $b = loc($x); $this->checked($x, $x); }\n    public function testForwards(): void { $this->forwards(1); }\n    public function testUnread(): void { $this->unread(1); }\n}\n",
            ),
            (
                "tests/TTest.cs",
                "public class TTest {\n  void Checked(int x, int b) { if (x != b) { throw new System.Exception(\"x\"); } }\n  void Forwards(int x) { var b = Loc(x); Checked(x, b); }\n  void Unread(int x) { var b = Loc(x); Checked(x, x); }\n  [Fact] public void TestForwards() { Forwards(1); }\n  [Fact] public void TestUnread() { Unread(1); }\n}\n",
            ),
            (
                "test/t_test.rb",
                "class TTest < Minitest::Test\n  def checked(x, b)\n    raise ArgumentError, 'x' if x != b\n  end\n\n  def forwards(x)\n    b = loc(x)\n    checked(x, b)\n  end\n\n  def unread(x)\n    b = loc(x)\n    checked(x, x)\n  end\n\n  def test_forwards\n    forwards(1)\n  end\n\n  def test_unread\n    unread(1)\n  end\nend\n",
            ),
            (
                "src/test/kotlin/TTest.kt",
                "class TTest {\n    private fun checked(x: Int, b: Int) { if (x != b) throw IllegalStateException(\"x\") }\n    private fun forwards(x: Int) { val b: Int = loc(x); checked(x, b) }\n    private fun unread(x: Int) { val b = loc(x); checked(x, x) }\n    @Test\n    fun testForwards() { forwards(1) }\n    @Test\n    fun testUnread() { unread(1) }\n}\n",
            ),
            (
                "Tests/TTests.swift",
                "import XCTest\n\nfinal class TTests: XCTestCase {\n    func checked(_ x: Int, _ b: Int) { if x != b { fatalError(\"x\") } }\n    func forwards(_ x: Int) { let b = loc(x); checked(x, b) }\n    func unread(_ x: Int) { let b = loc(x); checked(x, x) }\n    func testForwards() { forwards(1) }\n    func testUnread() { unread(1) }\n}\n",
            ),
            (
                "src/test/scala/TSuite.scala",
                "class TSuite extends AnyFunSuite {\n  private def checked(x: Int, b: Int): Unit = if (x != b) throw new IllegalStateException(\"x\")\n  private def forwards(x: Int): Unit = { val b = loc(x); checked(x, b) }\n  private def unread(x: Int): Unit = { val b = loc(x); checked(x, x) }\n  test(\"forwards\") { forwards(1) }\n  test(\"unread\") { unread(1) }\n}\n",
            ),
            (
                "Tests/TTests.m",
                "#import <XCTest/XCTest.h>\n@interface TTests : XCTestCase\n@end\n@implementation TTests\n- (void)checked:(int)x b:(int)b {\n    if (x != b) { XCTFail(@\"x\"); }\n}\n- (void)forwards:(int)x {\n    int b = loc(x);\n    [self checked:x b:b];\n}\n- (void)unread:(int)x {\n    int b = loc(x);\n    [self checked:x b:x];\n}\n- (void)testForwards {\n    [self forwards:1];\n}\n- (void)testUnread {\n    [self unread:1];\n}\n@end\n",
            ),
        ];
        let reg = default_registry();
        let vocab = AssertVocabulary::default();
        let mut ran = 0;
        let mut wrong = Vec::new();
        for (path, src) in cases {
            let Some(pack) = reg.find_pack(path) else {
                continue;
            };
            ran += 1;
            let facts = pack.extract(path, src, &vocab).expect(path);
            for (marker, want) in [("forwards", (1, 1)), ("unread", (0, 0))] {
                let t = facts
                    .tests
                    .iter()
                    .find(|t| t.name.to_lowercase().contains(marker))
                    .unwrap_or_else(|| panic!("{path}: no `{marker}` test in {:?}", facts.tests));
                if (t.total_asserts, t.helper_checks) != want {
                    wrong.push(format!(
                        "{path} {marker}: {:?}",
                        (t.total_asserts, t.helper_checks)
                    ));
                }
            }
        }
        assert!(ran > 0, "no pack compiled in");
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    /// In C/C++ and Python, which follow ordinary calls, a wrapper's other calls (in the
    /// locals it computes) are still followed: `both` counts `loc`'s checks and `checked`'s.
    #[test]
    fn a_forwarding_wrapper_still_follows_the_calls_computing_its_locals() {
        let cases: &[(&str, &str)] = &[
            (
                "tests/test_t.py",
                "def loc(x):\n    if x != 1:\n        raise ValueError(\"loc\")\n    return x\n\ndef checked(x, b):\n    if x != b:\n        raise ValueError(\"x\")\n\ndef both(x):\n    b = loc(x)\n    return checked(x, b)\n\ndef test_both():\n    both(1)\n\ndef test_loc():\n    loc(1)\n\ndef test_checked():\n    checked(1, 1)\n",
            ),
            (
                "tests/t_test.cc",
                "namespace {\nint Loc(int x) { if (x != 1) std::abort(); return x; }\nvoid Checked(int x, int b) { if (x != b) std::abort(); }\nvoid Both(int x) { auto b = Loc(x); Checked(x, b); }\n}  // namespace\nTEST(T, Both) { Both(1); }\nTEST(T, Loc) { Loc(1); }\nTEST(T, Checked) { Checked(1, 1); }\n",
            ),
        ];
        let reg = default_registry();
        let vocab = AssertVocabulary::default();
        for (path, src) in cases {
            let Some(pack) = reg.find_pack(path) else {
                continue;
            };
            let facts = pack.extract(path, src, &vocab).expect(path);
            let total = |i: usize| facts.tests[i].total_asserts;
            assert_eq!(total(0), total(1) + total(2), "{path}: {:?}", facts.tests);
            assert!(total(1) > 0 && total(2) > 0, "{path}: {:?}", facts.tests);
        }
    }

    /// Helpers named in a dispatch table and run in a loop resolve like direct calls.
    #[test]
    fn helpers_in_a_dispatch_table_resolve_in_every_pack_that_supports_it() {
        let cases: &[(&str, &str)] = &[
            (
                "tests/t.rs",
                "fn check_a(x: u32) { if x != 1 { panic!(\"a\"); } }\nfn check_b(x: u32) { if x != 2 { panic!(\"b\"); } }\n#[test]\nfn t() {\n    for f in [check_a, check_b] { f(g()); }\n}\n",
            ),
            (
                "test/t.test.js",
                "function checkA(x) { if (x !== 1) { throw new Error('a'); } }\nfunction checkB(x) { expect(x).toBe(2); }\ntest('t', () => {\n  [checkA, checkB].forEach((f) => f(g()));\n});\n",
            ),
            (
                "pkg/t_test.go",
                "package a\nimport \"testing\"\nfunc checkA(x int) { if x != 1 { panic(\"a\") } }\nfunc checkB(x int) { if x != 2 { panic(\"b\") } }\nfunc TestT(t *testing.T) {\n\tfor _, f := range []func(int){checkA, checkB} { f(g()) }\n}\n",
            ),
            (
                "src/test/java/TTest.java",
                "class TTest {\n  void checkA() { if (g() != 1) { throw new IllegalStateException(); } }\n  void checkB() { if (g() != 2) { throw new IllegalStateException(); } }\n  @Test\n  void t() {\n    List.<Runnable>of(this::checkA, this::checkB).forEach(Runnable::run);\n  }\n}\n",
            ),
            (
                "src/test/kotlin/TTest.kt",
                "class TTest {\n    private fun checkA() { if (g() != 1) throw IllegalStateException() }\n    private fun checkB() { if (g() != 2) throw IllegalStateException() }\n    @Test\n    fun t() {\n        listOf(::checkA, ::checkB).forEach { it() }\n    }\n}\n",
            ),
            (
                "tests/TTest.cs",
                "public class TTest {\n  void CheckA() { if (G() != 1) { throw new System.Exception(); } }\n  void CheckB() { if (G() != 2) { throw new System.Exception(); } }\n  [Fact]\n  public void T() {\n    foreach (var f in new System.Action[] { CheckA, CheckB }) { f(); }\n  }\n}\n",
            ),
            (
                "test/t_test.rb",
                "class TTest < Minitest::Test\n  def check_a\n    raise 'a' if g != 1\n  end\n\n  def check_b\n    raise 'b' if g != 2\n  end\n\n  def test_t\n    %i[check_a check_b].each { |m| send(m) }\n  end\nend\n",
            ),
        ];
        let reg = default_registry();
        let vocab = AssertVocabulary::default();
        for (path, src) in cases {
            let Some(pack) = reg.find_pack(path) else {
                continue;
            };
            let facts = pack.extract(path, src, &vocab).expect(path);
            let t = facts
                .tests
                .iter()
                .find(|t| t.name.ends_with('t') || t.name.ends_with("T"))
                .unwrap_or_else(|| panic!("{path}: {:?}", facts.tests));
            assert_eq!((t.total_asserts, t.helper_checks), (2, 2), "{path}: {t:?}");
        }
    }

    #[test]
    fn default_registry_contains_rust() {
        let reg = default_registry();
        assert!(reg.is_supported("src/lib.rs"));
        let pack = reg.find_pack("src/main.rs").expect("rust pack found");
        assert_eq!(pack.id(), "rust");
        assert_eq!(pack.name(), "Rust");
        #[cfg(feature = "lang-python")]
        {
            assert!(reg.is_supported("tests/test_foo.py"));
            let py_pack = reg.find_pack("test.py").expect("python pack found");
            assert_eq!(py_pack.id(), "python");
            assert_eq!(py_pack.name(), "Python");
        }
        #[cfg(feature = "lang-javascript")]
        {
            assert!(reg.is_supported("web/app.test.tsx"));
            assert!(reg.is_supported("web/app.test.jsx"));
            assert!(reg.is_supported("web/app.test.js"));
            let js_pack = reg.find_pack("index.js").expect("javascript pack found");
            assert_eq!(js_pack.id(), "javascript");
            assert_eq!(js_pack.name(), "JavaScript/TypeScript");
        }
        #[cfg(feature = "lang-java")]
        {
            assert!(reg.is_supported("src/test/java/CalcTest.java"));
            let java_pack = reg.find_pack("CalcTest.java").expect("java pack found");
            assert_eq!(java_pack.id(), "java");
            assert_eq!(java_pack.name(), "Java");
        }
        #[cfg(feature = "lang-go")]
        {
            assert!(reg.is_supported("calc_test.go"));
            let go_pack = reg.find_pack("calc_test.go").expect("go pack found");
            assert_eq!(go_pack.id(), "go");
            assert_eq!(go_pack.name(), "Go");
        }
        #[cfg(feature = "lang-php")]
        {
            assert!(reg.is_supported("bindings/php/tests/ExampleTest.php"));
            let php_pack = reg.find_pack("Test.php").expect("php pack found");
            assert_eq!(php_pack.id(), "php");
            assert_eq!(php_pack.name(), "PHP");
        }
        #[cfg(feature = "lang-c")]
        {
            assert!(reg.is_supported("crates/example-capi/smoke/modern_api_smoke.c"));
            let c_pack = reg.find_pack("smoke.c").expect("c pack found");
            assert_eq!(c_pack.id(), "c");
            assert_eq!(c_pack.name(), "C");
        }
        #[cfg(feature = "lang-cpp")]
        {
            assert!(reg.is_supported("tests/test_example.cpp"));
            let cpp_pack = reg.find_pack("test.cpp").expect("cpp pack found");
            assert_eq!(cpp_pack.id(), "cpp");
            assert_eq!(cpp_pack.name(), "C++");
        }
        #[cfg(feature = "lang-csharp")]
        {
            assert!(reg.is_supported("bindings/dotnet/tests/Example.NET.Tests/ExampleMapTests.cs"));
            let csharp_pack = reg.find_pack("test.cs").expect("csharp pack found");
            assert_eq!(csharp_pack.id(), "csharp");
            assert_eq!(csharp_pack.name(), "C#");
        }
        #[cfg(feature = "lang-ruby")]
        {
            assert!(reg.is_supported("bindings/ruby/test/test_example.rb"));
            let ruby_pack = reg.find_pack("test.rb").expect("ruby pack found");
            assert_eq!(ruby_pack.id(), "ruby");
            assert_eq!(ruby_pack.name(), "Ruby");
        }
    }

    #[test]
    fn registry_dispatch_and_custom_pack_registration() {
        let mut reg = LanguageRegistry::new();
        assert!(!reg.is_supported("test.xyz"));
        assert!(reg.find_pack("test.xyz").is_none());

        reg.register(Box::new(MockCustomPack));
        assert!(reg.is_supported("test.xyz"));
        let pack = reg.find_pack("sub/dir/test.xyz").expect("pack found");
        assert_eq!(pack.id(), "mock-xyz");
        assert_eq!(pack.name(), "Mock XYZ");

        let facts = pack
            .extract("test.xyz", "test_case()", &AssertVocabulary::default())
            .expect("extract succeeds");
        assert_eq!(facts.tests.len(), 1);
        assert_eq!(facts.tests[0].name, "mock_test");
        assert_eq!(facts.tests[0].total_asserts, 1);
    }

    #[test]
    fn unsupported_source_respects_active_registry() {
        let mut reg = default_registry();
        assert!(is_unsupported_source_in("main.dart", &reg));

        struct DartDummy;
        impl LanguagePack for DartDummy {
            fn id(&self) -> &'static str {
                "dart"
            }
            fn name(&self) -> &'static str {
                "Dart"
            }
            fn matches(&self, path: &str) -> bool {
                extension(path) == Some("dart")
            }
            fn extract(
                &self,
                _path: &str,
                _src: &str,
                _vocab: &AssertVocabulary,
            ) -> Result<ParsedFileFacts> {
                Ok(ParsedFileFacts::default())
            }
        }

        reg.register(Box::new(DartDummy));
        assert!(!is_unsupported_source_in("main.dart", &reg));
    }
}
