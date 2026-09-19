//! Language pack abstraction and fact extraction.
//!
//! Language packs extract language-neutral facts ([`ParsedFileFacts`]) from source
//! files for the agent-guard gates to reason about.
//! Everything here is keyed on language syntax nodes, never substring matches.

use anyhow::Result;

#[cfg(feature = "lang-go")]
pub mod r#go;
#[cfg(feature = "lang-golden")]
pub mod golden;
#[cfg(feature = "lang-java")]
pub mod java;
#[cfg(feature = "lang-javascript")]
pub mod javascript;
#[cfg(feature = "lang-php")]
pub mod php;
#[cfg(feature = "lang-python")]
pub mod python;
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

    /// Human-readable display name (e.g. "Rust", "Golden/Snapshot", "Python").
    fn name(&self) -> &'static str;

    /// Whether this pack handles the given relative path.
    fn matches(&self, path: &str) -> bool;

    /// Extract language-neutral facts from source text.
    fn extract(&self, path: &str, src: &str, vocab: &AssertVocabulary) -> Result<ParsedFileFacts>;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestFn {
    /// Module-qualified name or test identity, e.g. `tests::inserts_in_order`.
    pub name: String,
    /// 1-based line of the test definition.
    pub line: usize,
    pub total_asserts: usize,
    /// Equality / pattern assertions (`assert_eq!`, `assert_ne!`, `assert_matches!` ...).
    pub strong_asserts: usize,
    pub tautologies: usize,
    pub ignored: bool,
    pub should_panic: bool,
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

#[derive(Debug, Clone, Default)]
pub struct ParsedFileFacts {
    pub tests: Vec<TestFn>,
    pub unsafe_sites: Vec<UnsafeSite>,
    pub escape_hatches: Vec<EscapeHatchSite>,
    /// The grammar could not parse part of the file; facts may be incomplete.
    pub has_parse_errors: bool,
}

/// Backwards compatibility alias for [`ParsedFileFacts`].
pub type RustFacts = ParsedFileFacts;

#[derive(Debug, Clone, Default)]
pub struct AssertVocabulary {
    pub extra_macros: Vec<String>,
    pub helper_fns: Vec<String>,
    pub safety_placeholders: Vec<String>,
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
        assert!(!is_unsupported_source("pkg/mod/a.py"));
        assert!(!is_unsupported_source("web/App.tsx"));
        assert!(!is_unsupported_source("service.java"));
        assert!(!is_unsupported_source("src/a.go"));
        assert!(!is_unsupported_source("src/a.php"));
        assert!(is_unsupported_source("service.kt"));
        assert!(is_unsupported_source("main.c"));
        assert!(!is_unsupported_source("src/a.rs"));
        assert!(!is_unsupported_source("tests/001.phpt"));
        assert!(!is_unsupported_source("docs/plan.md"));
        assert!(!is_unsupported_source("Makefile"));
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
            assert!(reg.is_supported("bindings/php/tests/ExpanseTest.php"));
            let php_pack = reg.find_pack("Test.php").expect("php pack found");
            assert_eq!(php_pack.id(), "php");
            assert_eq!(php_pack.name(), "PHP");
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
        assert!(is_unsupported_source_in("main.kt", &reg));

        struct KotlinDummy;
        impl LanguagePack for KotlinDummy {
            fn id(&self) -> &'static str {
                "kotlin"
            }
            fn name(&self) -> &'static str {
                "Kotlin"
            }
            fn matches(&self, path: &str) -> bool {
                extension(path) == Some("kt")
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

        reg.register(Box::new(KotlinDummy));
        assert!(!is_unsupported_source_in("main.kt", &reg));
    }
}
