//! C extension macros the grammar cannot read, masked before parsing.
//!
//! tree-sitter parses C without expanding macros, so the idioms of a PHP, Python or Ruby
//! C extension (`PHP_METHOD(Judy, size) { ... }`, `ZEND_PARSE_PARAMETERS_START(1, 1)`
//! with no semicolon, `PyObject_HEAD` in a struct) become error regions and the facts
//! inside them are lost. [`mask`] rewrites those spans before the parse, byte for byte:
//! the result has the same length and every newline where it was, so every node's
//! byte range and line are the original's.
//!
//! Two kinds of macro:
//! - **blanked** ([`BUILTIN_MACROS`], `[languages.c] macros`): the name, and its
//!   balanced `(...)` when called, become spaces. What is left is ordinary C:
//!   `static void f(zval *z) { ; }`, an empty initializer, a struct with its fields.
//! - **function heads** ([`BUILTIN_FUNCTION_MACROS`], `[languages.c] function_macros`):
//!   `PHP_METHOD(Judy, size)` becomes `int Judy_size()` padded with spaces, so the body
//!   after it is a function the stub, error-swallowing and assertion walkers read.
//!
//! An entry ending in `*` matches every name with that prefix. Comments, string and
//! character literals, and preprocessor lines (with their `\` continuations) are never
//! rewritten, so a `#define` of a listed macro keeps its text.

/// Macros blanked with their arguments: declaration and statement macros written without
/// a semicolon, list entries written without a comma, and attribute-like prefixes.
pub const BUILTIN_MACROS: &[&str] = &[
    // PHP (Zend): argument info blocks.
    "ZEND_BEGIN_ARG_*",
    "ZEND_ARG_*",
    "ZEND_END_ARG_INFO",
    // PHP (Zend): fast parameter parsing.
    "ZEND_PARSE_PARAMETERS_START",
    "ZEND_PARSE_PARAMETERS_START_EX",
    "ZEND_PARSE_PARAMETERS_END",
    "Z_PARAM_*",
    // PHP (Zend): module globals and thread-safe resource cache.
    "ZEND_DECLARE_MODULE_GLOBALS",
    "ZEND_EXTERN_MODULE_GLOBALS",
    "ZEND_BEGIN_MODULE_GLOBALS",
    "ZEND_END_MODULE_GLOBALS",
    "ZEND_TSRMLS_CACHE_DEFINE",
    "ZEND_TSRMLS_CACHE_EXTERN",
    "ZEND_TSRMLS_CACHE_UPDATE",
    "ZEND_GET_MODULE",
    // PHP (Zend): function and method tables.
    "PHP_FE",
    "PHP_FE_END",
    "PHP_ME",
    "PHP_MALIAS",
    "PHP_ABSTRACT_ME",
    "PHP_DEP_FE",
    "PHP_FALIAS",
    "ZEND_FE",
    "ZEND_FE_END",
    "ZEND_ME",
    "ZEND_MALIAS",
    "ZEND_ABSTRACT_ME",
    "ZEND_DEP_FE",
    "ZEND_FALIAS",
    "ZEND_NS_FE",
    // PHP (Zend): ini entries.
    "PHP_INI_BEGIN",
    "PHP_INI_END",
    "PHP_INI_ENTRY*",
    "STD_PHP_INI_*",
    // PHP (Zend): hash iteration heads (the body after one is a plain block).
    "ZEND_HASH_FOREACH_*",
    "ZEND_HASH_REVERSE_FOREACH_*",
    "ZEND_HASH_PACKED_FOREACH_*",
    "ZEND_HASH_MAP_FOREACH_*",
    // PHP (Zend): module dependency entries.
    "ZEND_MOD_REQUIRED",
    "ZEND_MOD_OPTIONAL",
    "ZEND_MOD_CONFLICTS",
    "ZEND_MOD_END",
    // PHP (Zend): attribute-like prefixes.
    "PHPAPI",
    "ZEND_API",
    "ZEND_FASTCALL",
    "zend_always_inline",
    "zend_never_inline",
    "ZEND_COLD",
    "ZEND_HOT",
    "ZEND_NORETURN",
    // CPython.
    "PyObject_HEAD",
    "PyObject_VAR_HEAD",
    "PyObject_HEAD_INIT",
    "PyVarObject_HEAD_INIT",
    "Py_BEGIN_ALLOW_THREADS",
    "Py_END_ALLOW_THREADS",
    "Py_BLOCK_THREADS",
    "Py_UNBLOCK_THREADS",
    // Ruby C API.
    "RUBY_EXTERN",
    "RUBY_FUNC_EXPORTED",
    "RUBY_SYMBOL_EXPORT_BEGIN",
    "RUBY_SYMBOL_EXPORT_END",
];

/// Apple SDK annotation and availability macros, blanked with their arguments in
/// Objective-C: `API_DEPRECATED("x", ios(8.0, API_TO_BE_DEPRECATED))`, `NS_SWIFT_NAME(x)`,
/// `CF_RETURNS_RETAINED` after a function head, `NS_ASSUME_NONNULL_BEGIN` alone on a line.
/// Explicit names: a prefix such as `API_*` would also blank a constant named `API_KEY`.
pub const APPLE_MACROS: &[&str] = &[
    "NS_ASSUME_NONNULL_BEGIN",
    "NS_ASSUME_NONNULL_END",
    "API_AVAILABLE",
    "API_UNAVAILABLE",
    "API_DEPRECATED",
    "API_DEPRECATED_WITH_REPLACEMENT",
    "API_TO_BE_DEPRECATED",
    "NS_AVAILABLE",
    "NS_AVAILABLE_IOS",
    "NS_AVAILABLE_MAC",
    "NS_DEPRECATED",
    "NS_DEPRECATED_IOS",
    "NS_DEPRECATED_MAC",
    "NS_CLASS_AVAILABLE",
    "NS_EXTENSION_UNAVAILABLE",
    "NS_EXTENSION_UNAVAILABLE_IOS",
    "NS_SWIFT_NAME",
    "NS_SWIFT_UNAVAILABLE",
    "NS_SWIFT_UI_ACTOR",
    "NS_SWIFT_NONISOLATED",
    "NS_SWIFT_SENDABLE",
    "NS_REFINED_FOR_SWIFT",
    "NS_DESIGNATED_INITIALIZER",
    "NS_UNAVAILABLE",
    "NS_REQUIRES_SUPER",
    "NS_REQUIRES_NIL_TERMINATION",
    "NS_NOESCAPE",
    "NS_FORMAT_FUNCTION",
    "NS_RETURNS_RETAINED",
    "NS_RETURNS_NOT_RETAINED",
    "NS_RETURNS_INNER_POINTER",
    "CF_RETURNS_RETAINED",
    "CF_RETURNS_NOT_RETAINED",
    "CF_CONSUMED",
    "NS_STRING_ENUM",
    "NS_EXTENSIBLE_STRING_ENUM",
    "NS_TYPED_ENUM",
    "NS_TYPED_EXTENSIBLE_ENUM",
    "FOUNDATION_EXPORT",
    "FOUNDATION_EXTERN",
    "UIKIT_EXTERN",
    "APPKIT_EXTERN",
    "__nullable",
    "__nonnull",
];

/// Apple enum macros written as a type head: `typedef NS_ENUM(NSInteger, SDImageCacheType)`.
const ENUM_HEAD_MACROS: &[&str] = &[
    "NS_ENUM",
    "NS_OPTIONS",
    "NS_CLOSED_ENUM",
    "NS_ERROR_ENUM",
    "CF_ENUM",
    "CF_OPTIONS",
    "CF_CLOSED_ENUM",
];

/// `typedef NS_ENUM(NSInteger, Name) { ... };` becomes `enum Name { ... };` padded with
/// spaces (same length, same newlines): the `typedef` and the macro are one span, and the
/// last argument is the type's name. `None` when the file has no such head.
pub fn mask_enum_heads(src: &str) -> Option<String> {
    let s = src.as_bytes();
    let mut out = s.to_vec();
    let mut changed = false;
    let mut i = 0;
    while i < s.len() {
        if let Some(end) = skip_opaque(s, i) {
            i = end;
            continue;
        }
        if !(s[i].is_ascii_alphabetic() || s[i] == b'_') || (i > 0 && is_ident(s[i - 1])) {
            i += 1;
            continue;
        }
        let start = i;
        while i < s.len() && is_ident(s[i]) {
            i += 1;
        }
        if !ENUM_HEAD_MACROS.contains(&&src[start..i]) {
            continue;
        }
        let mut open = i;
        while open < s.len() && (s[open] == b' ' || s[open] == b'\t') {
            open += 1;
        }
        let Some(end) = (s.get(open) == Some(&b'('))
            .then(|| matching_paren(s, open))
            .flatten()
        else {
            continue;
        };
        let args = String::from_utf8_lossy(&s[open + 1..end - 1]).to_string();
        let name: String = args
            .rsplit(',')
            .next()
            .unwrap_or("")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        // Take a `typedef` right before the macro into the span.
        let before = src[..start].trim_end();
        let from = if before.ends_with("typedef")
            && !before[..before.len() - 7]
                .ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
        {
            before.len() - 7
        } else {
            start
        };
        let head = if name.is_empty() {
            "enum".to_string()
        } else {
            format!("enum {name}")
        };
        if head.len() > end - from || s[from..end].contains(&b'\n') {
            continue;
        }
        out[from..from + head.len()].copy_from_slice(head.as_bytes());
        for o in &mut out[from + head.len()..end] {
            *o = b' ';
        }
        changed = true;
        i = end;
    }
    changed.then(|| String::from_utf8(out).ok()).flatten()
}

/// Macros that expand to a function head: `PHP_METHOD(Judy, size) { ... }`.
pub const BUILTIN_FUNCTION_MACROS: &[&str] = &[
    "PHP_FUNCTION",
    "PHP_METHOD",
    "PHP_NAMED_FUNCTION",
    "PHP_MINIT_FUNCTION",
    "PHP_MSHUTDOWN_FUNCTION",
    "PHP_RINIT_FUNCTION",
    "PHP_RSHUTDOWN_FUNCTION",
    "PHP_MINFO_FUNCTION",
    "PHP_GINIT_FUNCTION",
    "PHP_GSHUTDOWN_FUNCTION",
    "ZEND_FUNCTION",
    "ZEND_METHOD",
    "ZEND_NAMED_FUNCTION",
    "ZEND_MINIT_FUNCTION",
    "ZEND_MSHUTDOWN_FUNCTION",
    "ZEND_RINIT_FUNCTION",
    "ZEND_RSHUTDOWN_FUNCTION",
    "ZEND_MINFO_FUNCTION",
    "ZEND_GINIT_FUNCTION",
    "ZEND_GSHUTDOWN_FUNCTION",
];

fn listed(name: &str, list: &[String]) -> bool {
    list.iter().any(|e| match e.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == e,
    })
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The end of the literal or comment starting at `i`, or `None` when none starts there.
fn skip_opaque(s: &[u8], i: usize) -> Option<usize> {
    match (s[i], s.get(i + 1)) {
        (b'/', Some(b'/')) => Some(
            s[i..]
                .iter()
                .position(|&b| b == b'\n')
                .map_or(s.len(), |p| i + p),
        ),
        (b'/', Some(b'*')) => Some(
            s[i + 2..]
                .windows(2)
                .position(|w| w == b"*/")
                .map_or(s.len(), |p| i + 2 + p + 2),
        ),
        (q @ (b'"' | b'\''), _) => {
            let mut j = i + 1;
            while j < s.len() && s[j] != q && s[j] != b'\n' {
                j += if s[j] == b'\\' { 2 } else { 1 };
            }
            Some((j + 1).min(s.len()))
        }
        _ => None,
    }
}

/// The index just past the `)` matching the `(` at `open`, or `None` when unbalanced.
fn matching_paren(s: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while i < s.len() {
        if let Some(end) = skip_opaque(s, i) {
            i = end;
            continue;
        }
        match s[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// `PHP_METHOD(Judy, size)` → `Judy_size`; `PHP_MINIT_FUNCTION(judy)` → `minit_judy`, so
/// the module hooks of one extension keep distinct names. The longest candidate that fits
/// in `width` bytes as `int NAME()`, else the arguments alone cut to fit.
fn head_name(macro_name: &str, args: &[u8], width: usize) -> String {
    let room = width.saturating_sub(6);
    let parts: Vec<String> = String::from_utf8_lossy(args)
        .split(',')
        .map(|a| {
            a.chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect()
        })
        .filter(|a: &String| !a.is_empty())
        .collect();
    let lower = macro_name.to_ascii_lowercase();
    let stem = lower
        .trim_start_matches("php_")
        .trim_start_matches("zend_")
        .trim_end_matches("_function");
    let bare = parts.join("_");
    let prefixed = std::iter::once(stem.to_string())
        .chain(parts)
        .collect::<Vec<_>>()
        .join("_");
    let mut name = if !matches!(stem, "method" | "function" | "named") && prefixed.len() <= room {
        prefixed
    } else {
        bare
    };
    if name.is_empty() || name.as_bytes()[0].is_ascii_digit() {
        name.insert(0, '_');
    }
    name.truncate(room);
    name
}

/// `src` with the listed macros rewritten, the same length and newlines as `src`; `None`
/// when nothing was rewritten.
pub fn mask(src: &str, macros: &[String], function_macros: &[String]) -> Option<String> {
    let s = src.as_bytes();
    let mut out = s.to_vec();
    let mut changed = false;
    let mut i = 0;
    let mut line_start = true;
    while i < s.len() {
        let b = s[i];
        if b == b'\n' {
            line_start = true;
            i += 1;
            continue;
        }
        if line_start && b == b'#' {
            // A preprocessor line, with its `\` continuations.
            while i < s.len() && !(s[i] == b'\n' && (i == 0 || s[i - 1] != b'\\')) {
                i += 1;
            }
            continue;
        }
        if !b.is_ascii_whitespace() {
            line_start = false;
        }
        if let Some(end) = skip_opaque(s, i) {
            i = end;
            continue;
        }
        if !(b.is_ascii_alphabetic() || b == b'_') || (i > 0 && is_ident(s[i - 1])) {
            i += 1;
            continue;
        }
        let start = i;
        while i < s.len() && is_ident(s[i]) {
            i += 1;
        }
        let name = &src[start..i];
        let is_head = listed(name, function_macros);
        if !is_head && !listed(name, macros) {
            continue;
        }
        let mut open = i;
        while open < s.len() && (s[open] == b' ' || s[open] == b'\t') {
            open += 1;
        }
        let call_end = (s.get(open) == Some(&b'('))
            .then(|| matching_paren(s, open))
            .flatten();
        if is_head {
            let Some(end) = call_end else { continue };
            let width = end - start;
            let name = head_name(name, &s[open + 1..end - 1], width);
            if name.is_empty() || s[start..end].contains(&b'\n') {
                continue;
            }
            let head = format!("int {name}()");
            out[start..start + head.len()].copy_from_slice(head.as_bytes());
            for o in &mut out[start + head.len()..end] {
                *o = b' ';
            }
            i = end;
        } else {
            let end = call_end.unwrap_or(i);
            for o in &mut out[start..end] {
                if *o != b'\n' {
                    *o = b' ';
                }
            }
            i = end;
        }
        changed = true;
    }
    // Only ASCII bytes were written, over whole literals-free spans, so the bytes are
    // still UTF-8; a failure here would be a bug, and the original is parsed instead.
    changed.then(|| String::from_utf8(out).ok()).flatten()
}

/// The built-in lists with the configured entries appended.
pub fn lists(configured: &[String], builtin: &[&str]) -> Vec<String> {
    builtin
        .iter()
        .map(|s| s.to_string())
        .chain(configured.iter().cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin(src: &str) -> String {
        mask(
            src,
            &lists(&[], BUILTIN_MACROS),
            &lists(&[], BUILTIN_FUNCTION_MACROS),
        )
        .unwrap_or_else(|| src.to_string())
    }

    #[test]
    fn masking_keeps_every_byte_offset_and_newline() {
        let src = "PHP_METHOD(Judy, size)\n{\n\tZEND_PARSE_PARAMETERS_START(1, 1)\n\t\tZ_PARAM_ZVAL(z)\n\tZEND_PARSE_PARAMETERS_END();\n\tRETURN_LONG(1);\n}\n";
        let m = builtin(src);
        assert_eq!(m.len(), src.len());
        let nl = |t: &str| t.match_indices('\n').map(|(i, _)| i).collect::<Vec<_>>();
        assert_eq!(nl(&m), nl(src));
        assert!(m.starts_with("int Judy_size()       \n{"), "{m:?}");
        assert!(m.contains("\t                                 \n"), "{m:?}");
        assert!(m.contains("\tRETURN_LONG(1);"), "{m:?}");
        // A blanked call whose arguments span lines keeps its line breaks.
        let multi = "ZEND_BEGIN_ARG_INFO_EX(arginfo_x,\n\t0, 0, 1)\nint y;\n";
        let m = builtin(multi);
        assert_eq!(nl(&m), nl(multi));
        assert!(
            m.ends_with("\nint y;\n") && m.trim().starts_with("int y;"),
            "{m:?}"
        );
    }

    #[test]
    fn comments_strings_and_directives_are_never_rewritten() {
        let src = "#define PHP_FE_END { NULL }\n#define X(a) \\\n  PHP_ME(a)\n/* PHP_ME(a, b) */\nconst char *s = \"Z_PARAM_ZVAL(z)\"; // PHP_FE_END\nint PHP_FE_ENDING;\n";
        assert_eq!(builtin(src), src);
    }

    #[test]
    fn module_hooks_keep_distinct_names_and_prefix_entries_match() {
        let m = builtin("PHP_MINIT_FUNCTION(judy)\n{\n}\nPHP_MSHUTDOWN_FUNCTION(judy)\n{\n}\n");
        assert!(m.contains("int minit_judy()"), "{m:?}");
        assert!(m.contains("int mshutdown_judy()"), "{m:?}");
        let configured = mask(
            "MY_EXPORT int f(void) { return 0; }\nMYEXT_METHOD(a) { }\n",
            &["MY_*".to_string()],
            &["MYEXT_METHOD".to_string()],
        )
        .unwrap();
        assert!(
            configured.starts_with("          int f(void)"),
            "{configured:?}"
        );
        // `int myext_method_a()` does not fit in `MYEXT_METHOD(a)`: the argument alone does.
        assert!(configured.contains("int a()        "), "{configured:?}");
        // An unlisted macro is left alone.
        assert_eq!(mask("OTHER(a)\n", &[], &[]), None);
    }
}

/// C headers open and close `extern "C"` under `#ifdef __cplusplus`:
///
/// ```c
/// #ifdef __cplusplus
/// extern "C" {
/// #endif
/// ...
/// #ifdef __cplusplus
/// }
/// #endif
/// ```
///
/// `extern "C"` is not C, and the lone `}` then closes nothing, so both are error regions
/// for the C grammar. Inside such a guard, the `extern "C" {` line and a lone `}` line
/// become spaces (same length, same newlines). `None` when the file has no such guard.
pub fn mask_cplusplus_guards(src: &str) -> Option<String> {
    let mut out = String::with_capacity(src.len());
    let mut in_guard = false;
    let mut changed = false;
    for line in src.split_inclusive('\n') {
        let t = line.trim();
        let directive: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
        if directive == "#ifdef __cplusplus"
            || directive == "#if defined(__cplusplus)"
            || directive == "#if defined __cplusplus"
        {
            in_guard = true;
        } else if t.starts_with("#endif") || t.starts_with("#else") || t.starts_with("#elif") {
            in_guard = false;
        } else if in_guard {
            // `} /* extern "C" */` and `} // extern "C"` carry a comment after the brace.
            let code = t.split("//").next().unwrap_or(t);
            let code = code.split("/*").next().unwrap_or(code);
            let compact: String = code.chars().filter(|c| !c.is_whitespace()).collect();
            if compact == "extern\"C\"{" || compact == "}" {
                out.extend(
                    line.chars()
                        .map(|c| if c == '\n' || c == '\r' { c } else { ' ' }),
                );
                changed = true;
                continue;
            }
        }
        out.push_str(line);
    }
    changed.then_some(out)
}

#[cfg(test)]
mod guard_tests {
    use super::mask_cplusplus_guards;

    #[test]
    fn extern_c_guards_are_blanked_and_other_braces_are_not() {
        let src = "#ifdef __cplusplus\nextern \"C\" {\n#endif\nint f(void);\nstruct s { int a; };\n#ifdef __cplusplus\n}\n#endif\n";
        let m = mask_cplusplus_guards(src).unwrap();
        assert_eq!(m.len(), src.len());
        assert!(
            m.contains("#ifdef __cplusplus\n            \n#endif"),
            "{m:?}"
        );
        assert!(m.contains("#ifdef __cplusplus\n \n#endif"), "{m:?}");
        assert!(m.contains("struct s { int a; };"));
        let commented = "#ifdef __cplusplus\n} /* extern \"C\" */\n#endif\n";
        assert_eq!(
            mask_cplusplus_guards(commented).unwrap(),
            format!(
                "#ifdef __cplusplus\n{}\n#endif\n",
                " ".repeat("} /* extern \"C\" */".len())
            )
        );
        // A `}` outside a __cplusplus guard is code.
        assert_eq!(mask_cplusplus_guards("int f(void) {\n}\n"), None);
    }
}

#[cfg(test)]
mod enum_tests {
    use super::mask_enum_heads;

    #[test]
    fn apple_enum_heads_become_plain_enums_of_the_same_length() {
        let src = "typedef NS_ENUM(NSInteger, SDImageCacheType) {\n    SDImageCacheTypeNone,\n};\ntypedef NS_OPTIONS(NSUInteger, SDOptions) { SDOptionA = 1 << 0 };\nint NS_ENUMERATE = 0;\n";
        let m = mask_enum_heads(src).unwrap();
        assert_eq!(m.len(), src.len());
        assert!(m.starts_with("enum SDImageCacheType"), "{m:?}");
        assert!(m.contains("\nenum SDOptions "), "{m:?}");
        // A name that only starts like the macro is left alone.
        assert!(m.contains("int NS_ENUMERATE = 0;"));
        assert_eq!(mask_enum_heads("enum Plain { A };\n"), None);
    }
}
