//! Language pack abstraction and fact extraction.
//!
//! Language packs extract language-neutral facts ([`ParsedFileFacts`]) from source
//! files for the agent-guard gates to reason about.
//! Everything here is keyed on language syntax nodes, never substring matches.

use anyhow::Result;

pub mod budgets;
#[cfg(any(feature = "lang-c", feature = "lang-cpp"))]
pub mod c_cpp;
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
}

/// Source extensions discipline recognises but cannot analyse yet. A change
/// touching these is *named* in the report: the AST gates did not look at it.
pub const UNSUPPORTED_SOURCE_EXTS: &[&str] = &[
    "py", "js", "jsx", "mjs", "cjs", "ts", "tsx", "kt", "kts", "scala", "c", "h", "cc", "cpp",
    "cxx", "hpp", "hh", "cs", "rb", "swift", "php", "phpt", "m", "mm",
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
        assert!(is_unsupported_source("service.swift"));
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
        assert!(is_unsupported_source_in("main.scala", &reg));

        struct ScalaDummy;
        impl LanguagePack for ScalaDummy {
            fn id(&self) -> &'static str {
                "scala"
            }
            fn name(&self) -> &'static str {
                "Scala"
            }
            fn matches(&self, path: &str) -> bool {
                extension(path) == Some("scala")
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

        reg.register(Box::new(ScalaDummy));
        assert!(!is_unsupported_source_in("main.scala", &reg));
    }
}
