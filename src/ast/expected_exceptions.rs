//! Expected exceptions and panics inside assertions and test attributes:
//! `#[should_panic(expected = "...")]`, `pytest.raises(...)`, `assertThrows(...)`,
//! `toThrow(...)`, `Assert.Throws<...>`, `$this->expectException(...)`.
//!
//! Widening what an existing test accepts as expected failure keeps assertion count
//! and strength unchanged, so a count cannot see `#[should_panic(expected = "...")]`
//! become bare `#[should_panic]`, or `pytest.raises(ValueError, match="...")` become
//! `pytest.raises(Exception)`. Each expected exception or panic is recorded with its
//! skeleton, kind, exception type, and matcher. `assertion-reduction` pairs the base and
//! head expectations of one test ([`widened`]) and reports the ones that accept more.

use super::bounds::{text, walk};
use super::TestFn;
use tree_sitter::Node;

/// One expected exception or panic of one test.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExpectedException {
    pub line: usize,
    /// The assertion's skeleton used for pairing between base and head.
    pub skeleton: String,
    /// The assertion kind: "should_panic", "pytest.raises", "assertRaises", "assertThrows",
    /// "assertThrowsExactly", "test_expected", "toThrow", "not.toThrow", "Assert.Throws",
    /// "Assert.ThrowsAny", "expectException", "expectExceptionMessage",
    /// "expectExceptionCode", "xfail".
    pub kind: String,
    /// The expected exception type (e.g. "ValueError", "IllegalArgumentException", "TypeError"),
    /// if any. Several accepted types (a Python tuple) are joined with ", ".
    pub exception_type: Option<String>,
    /// The matcher (expected pattern, message, or substring), if any: the content of a
    /// string literal, or behind [`OPAQUE`] the text of any other expression.
    pub matcher: Option<String>,
}

/// An expected exception or panic that was widened between base and head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Widened {
    /// The head line of the widened expectation; 0 when `dropped`.
    pub line: usize,
    pub skeleton: String,
    pub detail: String,
    /// The base expectation has no counterpart at head: nothing in the test checks it.
    pub dropped: bool,
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    if (t.starts_with('"') && t.ends_with('"'))
        || (t.starts_with('\'') && t.ends_with('\''))
        || (t.starts_with('`') && t.ends_with('`'))
    {
        if t.len() >= 2 {
            t[1..t.len() - 1].to_string()
        } else {
            t.to_string()
        }
    } else {
        t.to_string()
    }
}

/// Leads a matcher that is not a string literal (a constant, a call, a regular expression
/// literal): its text can be told apart from another, and says nothing about what it matches.
pub(super) const OPAQUE: char = '\u{1}';

/// The content of a string literal node, so that a prefix or a quote style (`r"x"`, `'x'`)
/// does not change it. `None` for a literal with an interpolation and for any other node.
fn string_literal(node: Node, src: &str) -> Option<String> {
    if !matches!(
        node.kind(),
        "string"
            | "template_string"
            | "string_literal"
            | "verbatim_string_literal"
            | "raw_string_literal"
            | "encapsed_string"
    ) {
        return None;
    }
    let mut cursor = node.walk();
    let (mut content, mut delimited) = (String::new(), false);
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "string_content" | "string_fragment" | "string_literal_content" | "escape_sequence" => {
                content.push_str(text(child, src));
            }
            "string_start" | "string_end" => delimited = true,
            _ => return None,
        }
    }
    Some(if !content.is_empty() || delimited {
        content
    } else {
        unquote(text(node, src).trim_start_matches('@'))
    })
}

/// A matcher argument as [`ExpectedException::matcher`] keeps it.
fn matcher_value(node: Node, src: &str) -> String {
    string_literal(node, src).unwrap_or_else(|| format!("{OPAQUE}{}", text(node, src)))
}

/// The language an expectation kind belongs to: which hierarchy and matcher rules apply.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    Python,
    Java,
    Js,
    CSharp,
    Php,
    Other,
}

fn family(kind: &str) -> Family {
    match kind {
        "pytest.raises" | "assertRaises" => Family::Python,
        "assertThrows" | "assertThrowsExactly" | "test_expected" => Family::Java,
        "toThrow" | "not.toThrow" => Family::Js,
        "Assert.Throws" | "Assert.ThrowsAny" => Family::CSharp,
        "expectException" | "expectExceptionMessage" | "expectExceptionCode" => Family::Php,
        _ => Family::Other,
    }
}

/// Kinds that accept the named class only, and no subclass of it.
fn is_exact(kind: &str) -> bool {
    matches!(kind, "Assert.Throws" | "assertThrowsExactly")
}

/// Kinds that state the test passes when nothing (or nothing of the type) is thrown.
fn is_negated(kind: &str) -> bool {
    kind == "not.toThrow"
}

/// Kinds written on the test rather than in its body. Removing one changes what the test
/// is (it must now pass without the failure), which is not an expectation left unchecked.
fn is_attribute(kind: &str) -> bool {
    matches!(kind, "should_panic" | "test_expected" | "xfail")
}

/// Expectations that can stand for each other when a test is rewritten. PHP states the
/// class, the message and the code in separate calls, which never replace one another.
fn interchangeable(b: &str, h: &str) -> bool {
    let (fb, fh) = (family(b), family(h));
    fb == fh
        && is_negated(b) == is_negated(h)
        && (b == h || !matches!(fb, Family::Php | Family::Other))
}

/// Ancestor rank for exception types across supported languages:
/// 2: Root exception hierarchy (`BaseException`, `Throwable`).
/// 1: General exception hierarchy (`Exception`, `Error`, `System.Exception`, `StandardError`).
/// 0: Specific exception (`ValueError`, `IllegalArgumentException`, `TypeError`, `CustomError`, etc.).
fn ancestor_rank(type_name: &str) -> usize {
    match type_name {
        "BaseException" | "Throwable" => 2,
        "Exception" | "Error" | "StandardError" => 1,
        _ => 0,
    }
}

/// `(parent, direct children)` of each language's standard exception classes. Only the
/// classes a language ships are listed; a class a project defines is in no table, and its
/// relation to any other class is unknown.
type Hierarchy = &'static [(&'static str, &'static [&'static str])];

/// Python built-in exceptions ("Exception hierarchy", library reference, built-in exceptions).
const PYTHON_HIERARCHY: Hierarchy = &[
    (
        "BaseException",
        &[
            "BaseExceptionGroup",
            "GeneratorExit",
            "KeyboardInterrupt",
            "SystemExit",
            "Exception",
        ],
    ),
    (
        "Exception",
        &[
            "ArithmeticError",
            "AssertionError",
            "AttributeError",
            "BufferError",
            "EOFError",
            "ImportError",
            "LookupError",
            "MemoryError",
            "NameError",
            "OSError",
            "ReferenceError",
            "RuntimeError",
            "StopAsyncIteration",
            "StopIteration",
            "SyntaxError",
            "SystemError",
            "TypeError",
            "ValueError",
            "Warning",
        ],
    ),
    ("BaseExceptionGroup", &["ExceptionGroup"]),
    (
        "ArithmeticError",
        &["FloatingPointError", "OverflowError", "ZeroDivisionError"],
    ),
    ("ImportError", &["ModuleNotFoundError"]),
    ("LookupError", &["IndexError", "KeyError"]),
    ("NameError", &["UnboundLocalError"]),
    (
        "OSError",
        &[
            "BlockingIOError",
            "ChildProcessError",
            "ConnectionError",
            "FileExistsError",
            "FileNotFoundError",
            "InterruptedError",
            "IsADirectoryError",
            "NotADirectoryError",
            "PermissionError",
            "ProcessLookupError",
            "TimeoutError",
        ],
    ),
    (
        "ConnectionError",
        &[
            "BrokenPipeError",
            "ConnectionAbortedError",
            "ConnectionRefusedError",
            "ConnectionResetError",
        ],
    ),
    ("RuntimeError", &["NotImplementedError", "RecursionError"]),
    ("SyntaxError", &["IndentationError"]),
    ("IndentationError", &["TabError"]),
    ("ValueError", &["UnicodeError"]),
    (
        "UnicodeError",
        &[
            "UnicodeDecodeError",
            "UnicodeEncodeError",
            "UnicodeTranslateError",
        ],
    ),
    (
        "Warning",
        &[
            "BytesWarning",
            "DeprecationWarning",
            "EncodingWarning",
            "FutureWarning",
            "ImportWarning",
            "PendingDeprecationWarning",
            "ResourceWarning",
            "RuntimeWarning",
            "SyntaxWarning",
            "UnicodeWarning",
            "UserWarning",
        ],
    ),
];

/// Java exceptions of `java.lang`, `java.io`, `java.net`, `java.nio.file`, `java.util` and
/// `java.time` (the class hierarchy each class's API documentation states).
const JAVA_HIERARCHY: Hierarchy = &[
    ("Throwable", &["Exception", "Error"]),
    (
        "Exception",
        &[
            "RuntimeException",
            "IOException",
            "CloneNotSupportedException",
            "InterruptedException",
            "ReflectiveOperationException",
            "TimeoutException",
            "ExecutionException",
            "URISyntaxException",
        ],
    ),
    (
        "ReflectiveOperationException",
        &[
            "ClassNotFoundException",
            "IllegalAccessException",
            "InstantiationException",
            "NoSuchFieldException",
            "NoSuchMethodException",
            "InvocationTargetException",
        ],
    ),
    (
        "RuntimeException",
        &[
            "ArithmeticException",
            "ArrayStoreException",
            "ClassCastException",
            "IllegalArgumentException",
            "IllegalMonitorStateException",
            "IllegalStateException",
            "IndexOutOfBoundsException",
            "NegativeArraySizeException",
            "NullPointerException",
            "SecurityException",
            "UnsupportedOperationException",
            "ConcurrentModificationException",
            "NoSuchElementException",
            "EmptyStackException",
            "MissingResourceException",
            "UncheckedIOException",
            "DateTimeException",
            "CompletionException",
        ],
    ),
    (
        "IllegalArgumentException",
        &[
            "NumberFormatException",
            "IllegalThreadStateException",
            "InvalidPathException",
            "PatternSyntaxException",
            "IllegalFormatException",
            "IllegalCharsetNameException",
            "UnsupportedCharsetException",
        ],
    ),
    ("IllegalStateException", &["CancellationException"]),
    (
        "IndexOutOfBoundsException",
        &[
            "ArrayIndexOutOfBoundsException",
            "StringIndexOutOfBoundsException",
        ],
    ),
    ("NoSuchElementException", &["InputMismatchException"]),
    (
        "UnsupportedOperationException",
        &["ReadOnlyBufferException"],
    ),
    ("DateTimeException", &["DateTimeParseException"]),
    (
        "IOException",
        &[
            "FileNotFoundException",
            "EOFException",
            "UnsupportedEncodingException",
            "MalformedURLException",
            "SocketException",
            "UnknownHostException",
            "InterruptedIOException",
            "FileSystemException",
            "CharConversionException",
            "ObjectStreamException",
            "UTFDataFormatException",
            "ZipException",
            "CharacterCodingException",
            "ProtocolException",
        ],
    ),
    (
        "SocketException",
        &[
            "ConnectException",
            "BindException",
            "NoRouteToHostException",
            "PortUnreachableException",
        ],
    ),
    ("InterruptedIOException", &["SocketTimeoutException"]),
    (
        "FileSystemException",
        &[
            "NoSuchFileException",
            "AccessDeniedException",
            "FileAlreadyExistsException",
            "DirectoryNotEmptyException",
            "NotDirectoryException",
        ],
    ),
    (
        "Error",
        &["AssertionError", "LinkageError", "VirtualMachineError"],
    ),
    (
        "VirtualMachineError",
        &["OutOfMemoryError", "StackOverflowError", "InternalError"],
    ),
    (
        "LinkageError",
        &[
            "NoClassDefFoundError",
            "ExceptionInInitializerError",
            "ClassFormatError",
            "UnsatisfiedLinkError",
            "IncompatibleClassChangeError",
            "VerifyError",
        ],
    ),
    (
        "IncompatibleClassChangeError",
        &[
            "AbstractMethodError",
            "IllegalAccessError",
            "InstantiationError",
            "NoSuchFieldError",
            "NoSuchMethodError",
        ],
    ),
];

/// .NET exceptions of `System`, `System.IO`, `System.Collections.Generic` and
/// `System.Threading.Tasks` (the inheritance each class's API reference states).
const CSHARP_HIERARCHY: Hierarchy = &[
    (
        "Exception",
        &[
            "SystemException",
            "ApplicationException",
            "AggregateException",
        ],
    ),
    (
        "SystemException",
        &[
            "ArgumentException",
            "ArithmeticException",
            "ArrayTypeMismatchException",
            "FormatException",
            "IndexOutOfRangeException",
            "InvalidCastException",
            "InvalidOperationException",
            "IOException",
            "KeyNotFoundException",
            "MemberAccessException",
            "NotImplementedException",
            "NotSupportedException",
            "NullReferenceException",
            "OperationCanceledException",
            "OutOfMemoryException",
            "RankException",
            "StackOverflowException",
            "TimeoutException",
            "TypeLoadException",
            "UnauthorizedAccessException",
        ],
    ),
    (
        "ArgumentException",
        &["ArgumentNullException", "ArgumentOutOfRangeException"],
    ),
    (
        "ArithmeticException",
        &[
            "DivideByZeroException",
            "OverflowException",
            "NotFiniteNumberException",
        ],
    ),
    ("FormatException", &["UriFormatException"]),
    ("InvalidOperationException", &["ObjectDisposedException"]),
    (
        "IOException",
        &[
            "FileNotFoundException",
            "DirectoryNotFoundException",
            "EndOfStreamException",
            "PathTooLongException",
            "FileLoadException",
            "DriveNotFoundException",
        ],
    ),
    (
        "MemberAccessException",
        &[
            "FieldAccessException",
            "MethodAccessException",
            "MissingMemberException",
        ],
    ),
    (
        "MissingMemberException",
        &["MissingFieldException", "MissingMethodException"],
    ),
    ("NotSupportedException", &["PlatformNotSupportedException"]),
    ("OperationCanceledException", &["TaskCanceledException"]),
    ("OutOfMemoryException", &["InsufficientMemoryException"]),
    (
        "TypeLoadException",
        &["DllNotFoundException", "EntryPointNotFoundException"],
    ),
];

/// ECMAScript native errors (the language specification, "Native Error Types").
const JS_HIERARCHY: Hierarchy = &[(
    "Error",
    &[
        "EvalError",
        "RangeError",
        "ReferenceError",
        "SyntaxError",
        "TypeError",
        "URIError",
        "AggregateError",
    ],
)];

/// PHP predefined and SPL exceptions (the manual, "Predefined Exceptions" and
/// "SPL Exceptions").
const PHP_HIERARCHY: Hierarchy = &[
    ("Throwable", &["Exception", "Error"]),
    (
        "Exception",
        &[
            "ErrorException",
            "JsonException",
            "LogicException",
            "RuntimeException",
        ],
    ),
    (
        "LogicException",
        &[
            "BadFunctionCallException",
            "DomainException",
            "InvalidArgumentException",
            "LengthException",
            "OutOfRangeException",
        ],
    ),
    ("BadFunctionCallException", &["BadMethodCallException"]),
    (
        "RuntimeException",
        &[
            "OutOfBoundsException",
            "OverflowException",
            "RangeException",
            "UnderflowException",
            "UnexpectedValueException",
        ],
    ),
    (
        "Error",
        &[
            "ArithmeticError",
            "AssertionError",
            "CompileError",
            "TypeError",
            "ValueError",
            "UnhandledMatchError",
        ],
    ),
    ("ArithmeticError", &["DivisionByZeroError"]),
    ("CompileError", &["ParseError"]),
    ("TypeError", &["ArgumentCountError"]),
];

fn hierarchy(fam: Family) -> Hierarchy {
    match fam {
        Family::Python => PYTHON_HIERARCHY,
        Family::Java => JAVA_HIERARCHY,
        Family::CSharp => CSHARP_HIERARCHY,
        Family::Js => JS_HIERARCHY,
        Family::Php => PHP_HIERARCHY,
        Family::Other => &[],
    }
}

/// Whether the standard hierarchy of `fam` places `ancestor` strictly above `class`.
fn table_ancestor(fam: Family, ancestor: &str, class: &str) -> bool {
    let table = hierarchy(fam);
    let mut current = class;
    // A table is a tree, so the walk ends; the bound guards a table edited into a cycle.
    for _ in 0..table.len() {
        let Some((parent, _)) = table.iter().find(|(_, kids)| kids.contains(&current)) else {
            return false;
        };
        if *parent == ancestor {
            return true;
        }
        current = parent;
    }
    false
}

/// Whether `ancestor` is known to accept everything `class` does and more: by the general
/// names of [`ancestor_rank`], or by the standard hierarchy. An exact-type assertion
/// accepts no subclass, so for it only a move to a general name is a known widening.
fn known_ancestor(fam: Family, ancestor: &str, class: &str, exact: bool) -> bool {
    ancestor != class
        && (ancestor_rank(ancestor) > ancestor_rank(class)
            || (!exact && table_ancestor(fam, ancestor, class)))
}

/// The class names an expectation accepts: the last segment of each (`java.io.IOException`,
/// `\LogicException` and `System.IO.IOException` are the class they end in), with the
/// names Python keeps as aliases of `OSError` read as `OSError`.
fn type_names(e: &ExpectedException) -> Vec<String> {
    let fam = family(&e.kind);
    let Some(raw) = &e.exception_type else {
        return Vec::new();
    };
    let mut names: Vec<String> = Vec::new();
    for part in raw.split(", ") {
        let part = part.trim().trim_end_matches(".class");
        let last = part.rsplit(['.', '\\', ':']).next().unwrap_or(part).trim();
        let name = match (fam, last) {
            (Family::Python, "IOError" | "EnvironmentError" | "WindowsError") => "OSError",
            _ => last,
        };
        if !name.is_empty() && !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
    }
    names
}

/// How the accepted classes moved between base and head.
enum TypeChange {
    Same,
    /// Head accepts a class base did not: `from .. to ..` or `now also accepts ..`.
    Widened(String),
    /// Head names no class where base did.
    Dropped,
    /// Head accepts strictly less: a known subclass, fewer classes, or a specific class
    /// where base named none.
    Narrowed,
    /// A replacement whose relation is not known.
    Unrelated,
}

fn type_change(b: &ExpectedException, h: &ExpectedException) -> TypeChange {
    let fam = family(&h.kind);
    let exact = is_exact(&h.kind);
    let (bt, ht) = (type_names(b), type_names(h));
    match (bt.is_empty(), ht.is_empty()) {
        (true, true) => return TypeChange::Same,
        (false, true) => return TypeChange::Dropped,
        (true, false) => {
            return if ht.iter().all(|t| ancestor_rank(t) == 0) {
                TypeChange::Narrowed
            } else {
                TypeChange::Unrelated
            };
        }
        (false, false) => {}
    }
    // A head class is covered when base accepted it already: the same class, or a class
    // below one base named.
    let uncovered: Vec<&String> = ht
        .iter()
        .filter(|t| {
            !bt.iter()
                .any(|b| b == *t || known_ancestor(fam, b, t, false))
        })
        .collect();
    if uncovered.is_empty() {
        let same = bt.len() == ht.len() && bt.iter().all(|b| ht.contains(b));
        return if same {
            TypeChange::Same
        } else {
            TypeChange::Narrowed
        };
    }
    for t in &uncovered {
        if let Some(below) = bt.iter().find(|b| known_ancestor(fam, t, b, exact)) {
            return TypeChange::Widened(format!("from `{below}` to `{t}`"));
        }
    }
    // Unknown relations: one class replaced by one other is a substitution, and is not
    // reported. More new classes than classes given up is a longer list of accepted ones.
    let given_up = bt.iter().filter(|b| !ht.contains(b)).count();
    if uncovered.len() > given_up {
        let added = uncovered[uncovered.len() - 1];
        return TypeChange::Widened(format!("now also accepts `{added}`"));
    }
    TypeChange::Unrelated
}

/// Whether `s` is free of pattern metacharacters, so it matches as the literal it is.
fn is_literal_pattern(s: &str) -> bool {
    !s.chars().any(|c| "\\^$.|?*+()[]{}".contains(c))
}

/// The pattern of a matcher written as a JavaScript regular expression literal.
fn js_regex_body(m: &str) -> Option<&str> {
    let rest = m.strip_prefix(OPAQUE)?.strip_prefix('/')?;
    let end = rest.rfind('/')?;
    Some(&rest[..end])
}

/// The matcher as a constraint: `None` when there is none, or when it accepts every
/// message (an empty string, or a pattern that is only `.*`).
fn effective_matcher(e: &ExpectedException) -> Option<&str> {
    let m = e.matcher.as_deref()?;
    let any_pattern = |p: &str| {
        let p = p.strip_prefix("(?s)").unwrap_or(p);
        let p = p.strip_prefix('^').unwrap_or(p);
        let p = p.strip_suffix('$').unwrap_or(p);
        p.is_empty() || p == ".*"
    };
    let accepts_all = match family(&e.kind) {
        _ if m.starts_with(OPAQUE) => js_regex_body(m).is_some_and(any_pattern),
        Family::Python => any_pattern(m),
        Family::CSharp => m.is_empty() || m == "*",
        _ => m.is_empty(),
    };
    (!accepts_all).then_some(m)
}

/// Whether the string a matcher holds is read as a pattern: a regular expression for
/// `pytest.raises(match=..)` and `assertRaisesRegex`, a wildcard for `WithMessage`.
fn matcher_is_pattern(e: &ExpectedException) -> bool {
    matches!(family(&e.kind), Family::Python | Family::CSharp)
}

enum MatcherChange {
    Same,
    /// Head has no constraint on the message where base had one.
    Dropped,
    /// Head's message is a proper part of base's, so it matches every message base did.
    Loosened,
    Tightened,
    /// Replaced by a matcher whose relation to the old one is not known.
    Unrelated,
}

fn matcher_change(b: &ExpectedException, h: &ExpectedException) -> MatcherChange {
    match (effective_matcher(b), effective_matcher(h)) {
        (None, None) => MatcherChange::Same,
        (Some(_), None) => MatcherChange::Dropped,
        (None, Some(_)) => MatcherChange::Tightened,
        (Some(x), Some(y)) if x == y => MatcherChange::Same,
        (Some(x), Some(y)) => {
            // Only two plain strings are compared as text: an expression's text and a
            // pattern say nothing about which messages they match.
            let literal = |e: &ExpectedException, m: &str| {
                !m.starts_with(OPAQUE) && (!matcher_is_pattern(e) || is_literal_pattern(m))
            };
            if !literal(b, x) || !literal(h, y) {
                MatcherChange::Unrelated
            } else if x.contains(y) {
                MatcherChange::Loosened
            } else if y.contains(x) {
                MatcherChange::Tightened
            } else {
                MatcherChange::Unrelated
            }
        }
    }
}

/// Whether `h` accepts more failures than `b`, for expectations that a failure satisfies.
fn accepts_more(b: &ExpectedException, h: &ExpectedException) -> Option<String> {
    let types = type_change(b, h);
    let matcher = matcher_change(b, h);
    // An exact-type assertion replaced by one that accepts subclasses too.
    let exactness_lost =
        family(&b.kind) == family(&h.kind) && is_exact(&b.kind) && !is_exact(&h.kind);
    let matcher_weaker = matches!(matcher, MatcherChange::Dropped | MatcherChange::Loosened);

    // A matcher given up for a narrower class is a trade, not a widening: the new
    // expectation rejects failures the old one accepted.
    if matcher_weaker && matches!(types, TypeChange::Narrowed) && !exactness_lost {
        return None;
    }
    if matcher_weaker {
        let what = match matcher {
            MatcherChange::Dropped if h.matcher.is_some() => "now accepts any message",
            MatcherChange::Dropped => "was removed",
            _ => "was shortened and matches more messages",
        };
        return Some(match types {
            TypeChange::Widened(tw) => {
                format!("expected exception type widened {tw} and matcher {what}")
            }
            _ => format!("expected pattern or message matcher {what}"),
        });
    }
    match types {
        TypeChange::Dropped => Some("expected exception type was removed".to_string()),
        TypeChange::Widened(tw) => Some(format!("expected exception type widened {tw}")),
        _ if exactness_lost => Some(
            "expected exception is no longer checked as the exact type, so a subclass passes"
                .to_string(),
        ),
        _ => None,
    }
}

/// Returns whether `h` is a widening of `b`.
pub fn is_widened(b: &ExpectedException, h: &ExpectedException) -> Option<String> {
    match (is_negated(&b.kind), is_negated(&h.kind)) {
        (false, false) => accepts_more(b, h),
        // "Does not throw X" passes on any other failure, so it is the bare form that
        // rejects the most: naming a type or a message is the widening.
        (true, true) => accepts_more(h, b).map(|_| {
            "negated expectation now names a type or message, so any other failure passes"
                .to_string()
        }),
        // An expectation turned into its negation is another assertion, not a wider one.
        _ => None,
    }
}

/// Pairs the `base` and `head` expectations of one test and returns the base expectations
/// that head checks less strictly, or no longer checks.
///
/// 1. An expectation written the same on both sides (skeleton, kind, type and matcher)
///    is unchanged, however many times it occurs and in whatever order.
/// 2. Of the rest, a skeleton left exactly once on each side is the same assertion edited
///    in place: the two are compared.
/// 3. What remains was rewritten beyond its skeleton (an edited block, a renamed binding,
///    a site replaced by another). Each base expectation needs a head expectation of its
///    own that accepts no more than it did; the assignment that satisfies the most base
///    expectations is taken. One left without is reported against a remaining head
///    expectation, or as dropped when head has none left.
pub fn widened(base: &[ExpectedException], head: &[ExpectedException]) -> Vec<Widened> {
    let same = |b: &ExpectedException, h: &ExpectedException| {
        b.skeleton == h.skeleton
            && b.kind == h.kind
            && b.exception_type == h.exception_type
            && b.matcher == h.matcher
    };
    let mut out = Vec::new();
    let mut head_left: Vec<&ExpectedException> = head.iter().collect();
    let mut base_left: Vec<&ExpectedException> = Vec::new();
    for b in base {
        match head_left.iter().position(|h| same(b, h)) {
            Some(i) => {
                head_left.remove(i);
            }
            None => base_left.push(b),
        }
    }

    let once =
        |set: &[&ExpectedException], s: &str| set.iter().filter(|x| x.skeleton == s).count() == 1;
    let in_place: Vec<(&ExpectedException, &ExpectedException)> = base_left
        .iter()
        .filter(|b| once(&base_left, &b.skeleton) && once(&head_left, &b.skeleton))
        .filter_map(|b| {
            let h = head_left.iter().find(|h| h.skeleton == b.skeleton)?;
            Some((*b, *h))
        })
        .collect();
    for (b, h) in &in_place {
        base_left.retain(|x| !std::ptr::eq(*x, *b));
        head_left.retain(|x| !std::ptr::eq(*x, *h));
        if let Some(detail) = is_widened(b, h) {
            out.push(Widened {
                line: h.line,
                skeleton: h.skeleton.clone(),
                detail,
                dropped: false,
            });
        }
    }

    // `owner[j]` is the base expectation that head expectation `j` stands for.
    let covers = |b: &ExpectedException, h: &ExpectedException| {
        interchangeable(&b.kind, &h.kind) && is_widened(b, h).is_none()
    };
    let mut owner: Vec<Option<usize>> = vec![None; head_left.len()];
    for i in 0..base_left.len() {
        let mut seen = vec![false; head_left.len()];
        assign(i, &base_left, &head_left, &covers, &mut owner, &mut seen);
    }
    for (i, b) in base_left.iter().enumerate() {
        if owner.contains(&Some(i)) {
            continue;
        }
        let spare = (0..head_left.len())
            .find(|&j| owner[j].is_none() && interchangeable(&b.kind, &head_left[j].kind));
        match spare {
            Some(j) => {
                owner[j] = Some(i);
                let h = head_left[j];
                out.push(Widened {
                    line: h.line,
                    skeleton: h.skeleton.clone(),
                    detail: is_widened(b, h).unwrap_or_else(|| {
                        "replaced by an expectation that accepts more".to_string()
                    }),
                    dropped: false,
                });
            }
            None if !is_attribute(&b.kind) => out.push(Widened {
                line: 0,
                skeleton: b.skeleton.clone(),
                detail: match type_names(b).first() {
                    Some(t) => format!("expected exception `{t}` is no longer checked"),
                    None => "expected failure is no longer checked".to_string(),
                },
                dropped: true,
            }),
            None => {}
        }
    }
    out.sort_by_key(|w| (w.dropped, w.line));
    out
}

/// One augmenting step of a bipartite matching: gives base expectation `i` a head
/// expectation that covers it, moving an earlier assignment aside when it has another.
fn assign(
    i: usize,
    base: &[&ExpectedException],
    head: &[&ExpectedException],
    covers: &dyn Fn(&ExpectedException, &ExpectedException) -> bool,
    owner: &mut [Option<usize>],
    seen: &mut [bool],
) -> bool {
    for j in 0..head.len() {
        if seen[j] || !covers(base[i], head[j]) {
            continue;
        }
        seen[j] = true;
        let free = match owner[j] {
            None => true,
            Some(other) => assign(other, base, head, covers, owner, seen),
        };
        if free {
            owner[j] = Some(i);
            return true;
        }
    }
    false
}

fn attribute(tests: &mut [TestFn], exp: ExpectedException) {
    let line = exp.line;
    if let Some(t) = tests
        .iter_mut()
        .filter(|t| t.line <= line && line <= t.end_line.max(t.line))
        .min_by_key(|t| t.end_line.saturating_sub(t.line))
    {
        if !t
            .expected_exceptions
            .iter()
            .any(|e| e.skeleton == exp.skeleton && e.line == exp.line)
        {
            t.expected_exceptions.push(exp);
        }
    }
}

/// Parses a Rust `#[should_panic]` attribute into an [`ExpectedException`].
pub fn parse_rust_should_panic(attr_text: &str, line: usize) -> ExpectedException {
    let matcher = parse_rust_matcher(attr_text);
    ExpectedException {
        line,
        skeleton: "#[should_panic]".to_string(),
        kind: "should_panic".to_string(),
        exception_type: None,
        matcher,
    }
}

/// One token of an attribute's text.
#[derive(Debug, PartialEq, Eq)]
enum AttrToken {
    Ident(String),
    /// A string literal's content, between its quotes.
    Str(String),
    Punct(char),
}

/// Splits the text of one attribute into identifiers, string literals and punctuation, so
/// that a word inside a string is never read as a key.
fn attr_tokens(text: &str) -> Vec<AttrToken> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '"' {
            let mut s = String::new();
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    s.push(chars[i]);
                    i += 1;
                }
                s.push(chars[i]);
                i += 1;
            }
            i += 1;
            out.push(AttrToken::Str(s));
        } else if c == 'r' && matches!(chars.get(i + 1), Some('"' | '#')) {
            let mut j = i + 1;
            while chars.get(j) == Some(&'#') {
                j += 1;
            }
            if chars.get(j) != Some(&'"') {
                // `r#ident`: a raw identifier.
                out.push(AttrToken::Punct(c));
                i += 1;
                continue;
            }
            let hashes = j - (i + 1);
            let mut s = String::new();
            let mut k = j + 1;
            while k < chars.len() {
                if chars[k] == '"'
                    && k + hashes < chars.len()
                    && chars[k + 1..=k + hashes].iter().all(|h| *h == '#')
                {
                    break;
                }
                s.push(chars[k]);
                k += 1;
            }
            i = k + 1 + hashes;
            out.push(AttrToken::Str(s));
        } else if c.is_alphanumeric() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(AttrToken::Ident(chars[start..i].iter().collect()));
        } else {
            out.push(AttrToken::Punct(c));
            i += 1;
        }
    }
    out
}

/// The message a `should_panic` attribute requires: `should_panic(expected = "..")` or
/// `should_panic = ".."`.
fn parse_rust_matcher(text: &str) -> Option<String> {
    let tokens = attr_tokens(text);
    let at = tokens
        .iter()
        .position(|t| *t == AttrToken::Ident("should_panic".to_string()))?;
    let rest = &tokens[at + 1..];
    match rest {
        [AttrToken::Punct('='), AttrToken::Str(s), ..] => Some(s.clone()),
        [AttrToken::Punct('('), args @ ..] => args.windows(3).find_map(|w| match w {
            [AttrToken::Ident(k), AttrToken::Punct('='), AttrToken::Str(s)] if k == "expected" => {
                Some(s.clone())
            }
            _ => None,
        }),
        _ => None,
    }
}

/// Rust: attribute-level `#[should_panic]` is handled directly by `rust.rs`.
pub fn rust(_root: Node, _src: &str, _tests: &mut [TestFn]) {}

/// Python: `pytest.raises(...)` (in `with` statement or call) and `self.assertRaises(...)` / `assertRaisesRegex`.
pub fn python(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    walk(root, &mut |node| {
        match node.kind() {
            "with_statement" => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "with_clause" {
                        let mut c2 = child.walk();
                        for item in child.children(&mut c2) {
                            if item.kind() == "with_item" {
                                inspect_python_with_item(node, item, src, tests);
                            }
                        }
                    } else if child.kind() == "with_item" {
                        inspect_python_with_item(node, child, src, tests);
                    }
                }
                true
            }
            "call" => {
                // Standalone calls like self.assertRaises(ValueError, f, -1). The context
                // manager of a `with` item, bound with `as` or not, is read above.
                if !is_python_with_item_value(node) {
                    inspect_python_standalone_call(node, src, tests);
                }
                true
            }
            _ => true,
        }
    });
}

/// Whether `call` is the context manager of a `with` item: its value, or the value an
/// `as` binding wraps (`with f() as e:` parses as `with_item > as_pattern > call`).
fn is_python_with_item_value(call: Node) -> bool {
    let Some(parent) = call.parent() else {
        return false;
    };
    match parent.kind() {
        "with_item" => true,
        "as_pattern" => {
            parent.parent().is_some_and(|g| g.kind() == "with_item")
                && parent.named_child(0).is_some_and(|c| c.id() == call.id())
        }
        _ => false,
    }
}

/// The classes a Python exception argument names: one class, or the members of a tuple
/// joined with ", ".
fn python_exception_type(arg: Node, src: &str) -> String {
    let inner = if arg.kind() == "parenthesized_expression" {
        arg.named_child(0).unwrap_or(arg)
    } else {
        arg
    };
    if inner.kind() == "tuple" {
        let mut cursor = inner.walk();
        inner
            .named_children(&mut cursor)
            .map(|c| text(c, src))
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        text(inner, src).to_string()
    }
}

/// The arguments of a `pytest.raises` call.
struct PytestRaises<'t> {
    exception_type: Option<String>,
    matcher: Option<String>,
    /// Positional arguments after the class: the callable of the call form and what it
    /// is called with.
    rest: Vec<Node<'t>>,
}

fn pytest_raises_arguments<'t>(args: Node<'t>, src: &str) -> PytestRaises<'t> {
    let mut cursor = args.walk();
    let mut out = PytestRaises {
        exception_type: None,
        matcher: None,
        rest: Vec::new(),
    };
    let mut positional = 0;
    for arg in args.named_children(&mut cursor) {
        if arg.kind() == "keyword_argument" {
            let (Some(name), Some(val)) = (
                arg.child_by_field_name("name"),
                arg.child_by_field_name("value"),
            ) else {
                continue;
            };
            match text(name, src) {
                "match" => out.matcher = Some(matcher_value(val, src)),
                "expected_exception" => out.exception_type = Some(python_exception_type(val, src)),
                _ => {}
            }
        } else if arg.kind() != "comment" {
            if positional == 0 && out.exception_type.is_none() {
                out.exception_type = Some(python_exception_type(arg, src));
            } else {
                out.rest.push(arg);
            }
            positional += 1;
        }
    }
    out
}

fn is_pytest_raises(func_text: &str) -> bool {
    func_text == "pytest.raises" || func_text.ends_with(".pytest.raises")
}

fn inspect_python_with_item(with_stmt: Node, item: Node, src: &str, tests: &mut [TestFn]) {
    let call = if let Some(val) = item.child_by_field_name("value") {
        val
    } else {
        let mut found = None;
        let mut cursor = item.walk();
        for c in item.children(&mut cursor) {
            if c.kind() == "call" {
                found = Some(c);
                break;
            }
        }
        let Some(c) = found else {
            return;
        };
        c
    };
    // `with pytest.raises(E) as e:` wraps the call in an `as_pattern`.
    let call = if call.kind() == "as_pattern" {
        let Some(inner) = call.named_child(0) else {
            return;
        };
        inner
    } else {
        call
    };
    let Some(func) = call.child_by_field_name("function") else {
        return;
    };
    let func_text = text(func, src);

    if is_pytest_raises(func_text) {
        let Some(args) = call.child_by_field_name("arguments") else {
            return;
        };
        let parsed = pytest_raises_arguments(args, src);

        // Skeleton: with_stmt text with the call's arguments replaced by (#)
        let (ws, we) = (with_stmt.start_byte(), with_stmt.end_byte());
        let (as_pos, ae_pos) = (args.start_byte(), args.end_byte());
        let skeleton_raw = if as_pos >= ws && ae_pos <= we {
            format!("{}(#){}", &src[ws..as_pos], &src[ae_pos..we])
        } else {
            format!("with pytest.raises(#): {}", text(with_stmt, src))
        };
        let skeleton = collapse_ws(&skeleton_raw);

        attribute(
            tests,
            ExpectedException {
                line: call.start_position().row + 1,
                skeleton,
                kind: "pytest.raises".to_string(),
                exception_type: parsed.exception_type,
                matcher: parsed.matcher,
            },
        );
    } else if func_text.ends_with("assertRaises") || func_text.ends_with("assertRaisesRegex") {
        let is_regex = func_text.ends_with("assertRaisesRegex");
        let Some(args) = call.child_by_field_name("arguments") else {
            return;
        };
        let mut cursor = args.walk();
        let named: Vec<Node> = args.named_children(&mut cursor).collect();

        let exception_type = named.first().map(|a| python_exception_type(*a, src));
        let matcher = if is_regex && named.len() >= 2 {
            Some(matcher_value(named[1], src))
        } else {
            None
        };

        // Normalize assertRaisesRegex -> assertRaises in skeleton
        let (ws, we) = (with_stmt.start_byte(), with_stmt.end_byte());
        let (as_pos, ae_pos) = (args.start_byte(), args.end_byte());
        let mut raw = if as_pos >= ws && ae_pos <= we {
            format!("{}(#){}", &src[ws..as_pos], &src[ae_pos..we])
        } else {
            format!("with self.assertRaises(#): {}", text(with_stmt, src))
        };
        if is_regex {
            raw = raw.replace("assertRaisesRegex", "assertRaises");
        }
        let skeleton = collapse_ws(&raw);

        attribute(
            tests,
            ExpectedException {
                line: call.start_position().row + 1,
                skeleton,
                kind: "assertRaises".to_string(),
                exception_type,
                matcher,
            },
        );
    }
}

fn inspect_python_standalone_call(call: Node, src: &str, tests: &mut [TestFn]) {
    let Some(func) = call.child_by_field_name("function") else {
        return;
    };
    let func_text = text(func, src);
    if is_pytest_raises(func_text) {
        // The call form: `pytest.raises(ValueError, f, -1)`.
        let Some(args) = call.child_by_field_name("arguments") else {
            return;
        };
        let parsed = pytest_raises_arguments(args, src);
        if parsed.exception_type.is_none() {
            return;
        }
        let rest: Vec<&str> = parsed.rest.iter().map(|a| text(*a, src)).collect();
        attribute(
            tests,
            ExpectedException {
                line: call.start_position().row + 1,
                skeleton: collapse_ws(&format!("pytest.raises(#, {})", rest.join(", "))),
                kind: "pytest.raises".to_string(),
                exception_type: parsed.exception_type,
                matcher: parsed.matcher,
            },
        );
        return;
    }
    if !func_text.ends_with("assertRaises") && !func_text.ends_with("assertRaisesRegex") {
        return;
    }
    let is_regex = func_text.ends_with("assertRaisesRegex");
    let Some(args) = call.child_by_field_name("arguments") else {
        return;
    };
    let mut cursor = args.walk();
    let named: Vec<Node> = args.named_children(&mut cursor).collect();
    if named.is_empty() {
        return;
    }
    let exception_type = Some(python_exception_type(named[0], src));
    let matcher = if is_regex && named.len() >= 2 {
        Some(matcher_value(named[1], src))
    } else {
        None
    };

    let mask_end = if is_regex && named.len() >= 2 {
        named[1].end_byte()
    } else {
        named[0].end_byte()
    };
    let (cs, ce) = (call.start_byte(), call.end_byte());
    let (ms, me) = (named[0].start_byte(), mask_end);
    let mut raw = format!("{}#{}", &src[cs..ms], &src[me..ce]);
    if is_regex {
        raw = raw.replace("assertRaisesRegex", "assertRaises");
    }
    let skeleton = collapse_ws(&raw);

    attribute(
        tests,
        ExpectedException {
            line: call.start_position().row + 1,
            skeleton,
            kind: "assertRaises".to_string(),
            exception_type,
            matcher,
        },
    );
}

/// How a name passed to `toThrow` reads: a class is written in PascalCase, and anything
/// else (a constant such as `ERR_MSG`, a variable) holds the message or the error.
fn js_name_is_class(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_uppercase()) && name.chars().any(|c| c.is_lowercase())
}

/// JavaScript / TypeScript: `expect(...).toThrow(...)` and `.toThrowError(...)`, and their
/// `.not` forms.
pub fn javascript(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    walk(root, &mut |node| {
        if node.kind() != "call_expression" {
            return true;
        }
        let Some(callee) = node.child_by_field_name("function") else {
            return true;
        };
        if callee.kind() != "member_expression" {
            return true;
        }
        let Some(prop) = callee.child_by_field_name("property") else {
            return true;
        };
        let prop_text = text(prop, src);
        if prop_text != "toThrow" && prop_text != "toThrowError" {
            return true;
        }
        let Some(obj) = callee.child_by_field_name("object") else {
            return true;
        };

        // `expect(f).not.toThrow()`: each `.not` in the chain inverts the expectation.
        let mut negated = false;
        let mut link = obj;
        while link.kind() == "member_expression" {
            if link
                .child_by_field_name("property")
                .is_some_and(|p| text(p, src) == "not")
            {
                negated = !negated;
            }
            let Some(next) = link.child_by_field_name("object") else {
                break;
            };
            link = next;
        }

        let mut exception_type = None;
        let mut matcher = None;

        if let Some(args) = node.child_by_field_name("arguments") {
            let mut cursor = args.walk();
            let named: Vec<Node> = args.named_children(&mut cursor).collect();
            for arg in named {
                match arg.kind() {
                    "comment" => {}
                    "identifier" | "member_expression" => {
                        // `errors.NotFound` is the class its last segment names.
                        let name = if arg.kind() == "member_expression" {
                            arg.child_by_field_name("property")
                                .map_or("", |p| text(p, src))
                        } else {
                            text(arg, src)
                        };
                        if js_name_is_class(name) {
                            if exception_type.is_none() {
                                exception_type = Some(text(arg, src).to_string());
                            }
                        } else if matcher.is_none() {
                            matcher = Some(matcher_value(arg, src));
                        }
                    }
                    "new_expression" => {
                        // `toThrow(new RangeError("too big"))`: the class and its message.
                        if let Some(ctor) = arg.child_by_field_name("constructor") {
                            if exception_type.is_none() {
                                exception_type = Some(text(ctor, src).to_string());
                            }
                        }
                        if let Some(first) = arg
                            .child_by_field_name("arguments")
                            .and_then(|a| a.named_child(0))
                        {
                            if matcher.is_none() {
                                matcher = Some(matcher_value(first, src));
                            }
                        }
                    }
                    // A string (a template without a substitution is the string it
                    // spells), a regular expression, or any other expression: a
                    // constraint on the failure.
                    _ => {
                        if matcher.is_none() {
                            matcher = Some(matcher_value(arg, src));
                        }
                    }
                }
            }
        }

        // Skeleton: obj text + ".toThrow(#)"
        let obj_text = text(obj, src);
        let skeleton = collapse_ws(&format!("{obj_text}.toThrow(#)"));

        attribute(
            tests,
            ExpectedException {
                line: node.start_position().row + 1,
                skeleton,
                kind: if negated { "not.toThrow" } else { "toThrow" }.to_string(),
                exception_type,
                matcher,
            },
        );
        true
    });
}

/// Java: `assertThrows(...)`, `assertThrowsExactly(...)`, and `@Test(expected = ...)`.
pub fn java(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    walk(root, &mut |node| {
        match node.kind() {
            "method_declaration" => {
                // Check annotations for @Test(expected = Foo.class)
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "modifiers" {
                        let mut mc = child.walk();
                        for m in child.children(&mut mc) {
                            if m.kind() == "annotation" {
                                inspect_java_annotation(m, src, tests);
                            }
                        }
                    } else if child.kind() == "annotation" {
                        inspect_java_annotation(child, src, tests);
                    }
                }
                true
            }
            "method_invocation" => {
                let Some(name) = node.child_by_field_name("name") else {
                    return true;
                };
                let name_text = text(name, src);
                if name_text != "assertThrows" && name_text != "assertThrowsExactly" {
                    return true;
                }
                let Some(args) = node.child_by_field_name("arguments") else {
                    return true;
                };
                let mut cursor = args.walk();
                let named: Vec<Node> = args.named_children(&mut cursor).collect();
                if named.is_empty() {
                    return true;
                }

                // First argument is usually the expected class: e.g. IllegalArgumentException.class
                let class_arg = named[0];
                let class_text = text(class_arg, src).trim_end_matches(".class").trim();
                let exception_type = Some(class_text.to_string());

                // The skeleton spells both methods `assertThrows`, so that the exact form
                // and the form accepting subclasses pair; the kind keeps them apart.
                let (ns, ne) = (node.start_byte(), node.end_byte());
                let (cs, ce) = (class_arg.start_byte(), class_arg.end_byte());
                let mut raw = format!("{}#{}", &src[ns..cs], &src[ce..ne]);
                if name_text == "assertThrowsExactly" {
                    raw = raw.replace("assertThrowsExactly", "assertThrows");
                }
                let skeleton = collapse_ws(&raw);

                attribute(
                    tests,
                    ExpectedException {
                        line: node.start_position().row + 1,
                        skeleton,
                        kind: name_text.to_string(),
                        exception_type,
                        matcher: None,
                    },
                );
                true
            }
            _ => true,
        }
    });
}

/// `@Test(expected = Foo.class)`, read from the annotation's `expected` element whatever
/// other elements stand beside it.
pub(super) fn java_annotation_expected(anno: Node, src: &str) -> Option<ExpectedException> {
    let args = anno.child_by_field_name("arguments")?;
    let mut cursor = args.walk();
    let value = args
        .named_children(&mut cursor)
        .filter(|pair| pair.kind() == "element_value_pair")
        .find(|pair| {
            pair.child_by_field_name("key")
                .is_some_and(|k| text(k, src) == "expected")
        })?
        .child_by_field_name("value")?;
    let class = text(value, src).trim_end_matches(".class").trim();
    if class.is_empty() {
        return None;
    }
    Some(ExpectedException {
        line: anno.start_position().row + 1,
        skeleton: "@Test(expected = #)".to_string(),
        kind: "test_expected".to_string(),
        exception_type: Some(class.to_string()),
        matcher: None,
    })
}

fn inspect_java_annotation(anno: Node, src: &str, tests: &mut [TestFn]) {
    if let Some(exp) = java_annotation_expected(anno, src) {
        attribute(tests, exp);
    }
}

/// The assertion classes of the C# pack (`csharp.rs`, `extract_assertions_in_body`).
const CSHARP_ASSERT_CLASSES: &[&str] = &[
    "Assert",
    "StringAssert",
    "CollectionAssert",
    "ClassicAssert",
];

/// The method name and type arguments of a C# member name: `Throws<T>` is a
/// `generic_name`, an identifier and a `type_argument_list`.
fn csharp_member<'t>(name: Node<'t>) -> (Node<'t>, Option<Node<'t>>) {
    if name.kind() == "generic_name" {
        let mut cursor = name.walk();
        let type_args = name
            .children(&mut cursor)
            .find(|c| c.kind() == "type_argument_list");
        (name.child(0).unwrap_or(name), type_args)
    } else {
        (name, None)
    }
}

/// Whether `receiver` is an assertion class, bare (`Assert`) or qualified (`Xunit.Assert`).
fn is_csharp_assert_class(receiver: Node, src: &str) -> bool {
    let class = match receiver.kind() {
        "identifier" => Some(receiver),
        "member_access_expression" | "qualified_name" => receiver.child_by_field_name("name"),
        _ => None,
    };
    class.is_some_and(|c| c.kind() == "identifier" && CSHARP_ASSERT_CLASSES.contains(&text(c, src)))
}

/// C#: `Assert.Throws<...>` / `Assert.ThrowsAny<...>` / `Assert.Catch<...>`, their async,
/// exact, `typeof` and MSTest spellings, and FluentAssertions' `.Should().Throw<...>()`.
/// A call is an expected exception only as a member of an assertion class or of a
/// `Should()` chain: `mock.Setup(..).Throws(..)` configures a double and asserts nothing.
pub fn csharp(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    walk(root, &mut |node| {
        if node.kind() != "invocation_expression" {
            return true;
        }
        let Some(expr) = node.child_by_field_name("function") else {
            return true;
        };
        if expr.kind() != "member_access_expression" {
            return true;
        }
        let (Some(receiver), Some(name)) = (
            expr.child_by_field_name("expression"),
            expr.child_by_field_name("name"),
        ) else {
            return true;
        };
        let (method, type_args) = csharp_member(name);
        if let Some(subject) = csharp_should_subject(receiver, src) {
            inspect_csharp_fluent(node, subject, text(method, src), type_args, src, tests);
            return true;
        }
        if !is_csharp_assert_class(receiver, src) {
            return true;
        }
        // `Catch` is NUnit's form that accepts subclasses, as xUnit's `ThrowsAny` does.
        let kind = match text(method, src) {
            "ThrowsAny" | "ThrowsAnyAsync" | "Catch" | "CatchAsync" => "Assert.ThrowsAny",
            "Throws"
            | "ThrowsAsync"
            | "ThrowsExactly"
            | "ThrowsExactlyAsync"
            | "ThrowsException"
            | "ThrowsExceptionAsync" => "Assert.Throws",
            _ => return true,
        };
        let Some(args) = node.child_by_field_name("arguments") else {
            return true;
        };
        let mut exception_type = type_args
            .and_then(|l| l.named_child(0))
            .map(|t| text(t, src).to_string());
        let mut skeleton = collapse_ws(&format!("Assert.Throws#{}", text(args, src)));
        // `Assert.Throws(typeof(T), () => ..)`: the class is the first argument.
        if exception_type.is_none() {
            let mut cursor = args.walk();
            let named: Vec<Node> = args.named_children(&mut cursor).collect();
            let type_of = named.first().and_then(|a| {
                let value = if a.kind() == "argument" {
                    a.named_child(0)?
                } else {
                    *a
                };
                (value.kind() == "typeof_expression").then_some(value)
            });
            if let Some(type_of) = type_of {
                exception_type = type_of.named_child(0).map(|t| text(t, src).to_string());
                let rest: Vec<&str> = named[1..].iter().map(|a| text(*a, src)).collect();
                skeleton = collapse_ws(&format!("Assert.Throws#({})", rest.join(", ")));
            }
        }

        attribute(
            tests,
            ExpectedException {
                line: node.start_position().row + 1,
                skeleton,
                kind: kind.to_string(),
                exception_type,
                matcher: None,
            },
        );
        true
    });
}

/// The subject of a FluentAssertions chain: `act` in `act.Should()`.
fn csharp_should_subject<'t>(receiver: Node<'t>, src: &str) -> Option<Node<'t>> {
    if receiver.kind() != "invocation_expression" {
        return None;
    }
    let callee = receiver.child_by_field_name("function")?;
    if callee.kind() != "member_access_expression" {
        return None;
    }
    let name = callee.child_by_field_name("name")?;
    if text(name, src) != "Should" {
        return None;
    }
    callee.child_by_field_name("expression")
}

/// `act.Should().Throw<T>()` and `.ThrowExactly<T>()`, with the `.WithMessage(..)` that
/// follows in the same chain.
fn inspect_csharp_fluent(
    call: Node,
    subject: Node,
    method: &str,
    type_args: Option<Node>,
    src: &str,
    tests: &mut [TestFn],
) {
    let kind = match method {
        "Throw" | "ThrowAsync" => "Assert.ThrowsAny",
        "ThrowExactly" | "ThrowExactlyAsync" => "Assert.Throws",
        _ => return,
    };
    let exception_type = type_args
        .and_then(|l| l.named_child(0))
        .map(|t| text(t, src).to_string());
    // Each further call in the chain is `invocation > member_access > (this call)`.
    let mut matcher = None;
    let mut link = call;
    while let Some(access) = link
        .parent()
        .filter(|p| p.kind() == "member_access_expression")
    {
        let Some(outer) = access
            .parent()
            .filter(|p| p.kind() == "invocation_expression")
        else {
            break;
        };
        let is_message = access
            .child_by_field_name("name")
            .is_some_and(|n| text(n, src) == "WithMessage");
        if is_message && matcher.is_none() {
            matcher = outer
                .child_by_field_name("arguments")
                .and_then(|a| a.named_child(0))
                .map(|a| {
                    let value = if a.kind() == "argument" {
                        a.named_child(0).unwrap_or(a)
                    } else {
                        a
                    };
                    matcher_value(value, src)
                });
        }
        link = outer;
    }
    attribute(
        tests,
        ExpectedException {
            line: call.start_position().row + 1,
            skeleton: collapse_ws(&format!("{}.Should().Throw#", text(subject, src))),
            kind: kind.to_string(),
            exception_type,
            matcher,
        },
    );
}

/// PHP: the class of `$this->expectException(Foo::class)`, and the message or code of
/// `expectExceptionMessage(..)` / `expectExceptionCode(..)`.
pub(super) fn php_expectation(
    call_name: &str,
    first_arg: Option<Node>,
    src: &str,
    line: usize,
) -> ExpectedException {
    let (mut exception_type, mut matcher) = (None, None);
    if let Some(arg) = first_arg {
        if call_name == "expectException" {
            // `Foo::class` names the class by its scope; a variable is kept as written.
            let class = if arg.kind() == "class_constant_access_expression" {
                arg.named_child(0).unwrap_or(arg)
            } else {
                arg
            };
            exception_type = Some(text(class, src).to_string());
        } else {
            matcher = Some(matcher_value(arg, src));
        }
    }
    ExpectedException {
        line,
        skeleton: format!("$this->{call_name}#"),
        kind: call_name.to_string(),
        exception_type,
        matcher,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ast::csharp::CSharpPack;
    use crate::ast::python::PythonPack;
    use crate::ast::{AssertVocabulary, LanguagePack};

    fn py(src: &str) -> Vec<TestFn> {
        PythonPack
            .extract("tests/test_t.py", src, &AssertVocabulary::default())
            .expect("extract succeeds")
            .tests
    }

    fn cs(body: &str) -> Vec<TestFn> {
        let src = format!(
            "using Xunit;\npublic class T {{\n    [Fact]\n    public void Run() {{\n        {body}\n    }}\n}}\n"
        );
        CSharpPack
            .extract("tests/T.cs", &src, &AssertVocabulary::default())
            .expect("extract succeeds")
            .tests
    }

    #[test]
    fn python_raises_as_form_is_read_like_the_plain_form() {
        let plain = py(
            "def test_t():\n    with pytest.raises(ValueError, match=\"neg\"):\n        f(-1)\n",
        );
        let bound = py(
            "def test_t():\n    with pytest.raises(ValueError, match=\"neg\") as e:\n        f(-1)\n",
        );
        assert_eq!(plain[0].expected_exceptions.len(), 1);
        let got = &bound[0].expected_exceptions;
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].kind, "pytest.raises");
        assert_eq!(got[0].exception_type.as_deref(), Some("ValueError"));
        assert_eq!(got[0].matcher.as_deref(), Some("neg"));
        assert_eq!(got[0].skeleton, "with pytest.raises(#) as e: f(-1)");
        assert_eq!(got[0].line, 2);
    }

    #[test]
    fn python_raises_as_form_widened_is_reported_and_unchanged_is_not() {
        let narrow = py(
            "def test_t():\n    with pytest.raises(ValueError, match=\"neg\") as e:\n        f(-1)\n",
        );
        let wide = py("def test_t():\n    with pytest.raises(Exception) as e:\n        f(-1)\n");
        let w = widened(&narrow[0].expected_exceptions, &wide[0].expected_exceptions);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].detail.contains("from `ValueError` to `Exception`"));
        assert!(widened(
            &narrow[0].expected_exceptions,
            &narrow[0].expected_exceptions
        )
        .is_empty());
    }

    #[test]
    fn python_assert_raises_as_form_is_recorded_once_on_the_with_path() {
        let src = |call: &str| {
            format!(
                "class TestC(unittest.TestCase):\n    def test_t(self):\n        with {call} as cm:\n            f(-1)\n"
            )
        };
        let narrow = py(&src("self.assertRaisesRegex(ValueError, \"neg\")"));
        let got = &narrow[0].expected_exceptions;
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].kind, "assertRaises");
        assert_eq!(got[0].exception_type.as_deref(), Some("ValueError"));
        assert_eq!(got[0].matcher.as_deref(), Some("neg"));
        assert_eq!(got[0].skeleton, "with self.assertRaises(#) as cm: f(-1)");

        let wide = py(&src("self.assertRaises(Exception)"));
        let w = widened(got, &wide[0].expected_exceptions);
        assert_eq!(w.len(), 1, "{w:?}");
    }

    #[test]
    fn csharp_mock_setup_throws_is_not_an_expected_exception() {
        let mocked = cs("mock.Setup(m => m.Get()).Throws(new Exception()); sut.Run();");
        assert!(
            mocked[0].expected_exceptions.is_empty(),
            "{:?}",
            mocked[0].expected_exceptions
        );
        assert!(mocked[0].is_vacuous());

        // The same holds for any receiver that is not an assertion class.
        for body in [
            "stub.Throws<InvalidOperationException>(); sut.Run();",
            "mock.Setup(m => m.GetAsync()).ThrowsAsync(new Exception()); sut.Run();",
            "ThrowsHelper(); sut.Run();",
            "Guard.Throws<InvalidOperationException>(() => sut.Run());",
        ] {
            let t = cs(body);
            assert!(t[0].expected_exceptions.is_empty(), "{body}");
            assert!(t[0].is_vacuous(), "{body}");
        }
    }

    #[test]
    fn csharp_assert_throws_is_an_expected_exception_and_not_vacuous() {
        for (body, kind) in [
            (
                "Assert.Throws<InvalidOperationException>(() => sut.Run());",
                "Assert.Throws",
            ),
            (
                "Assert.ThrowsAny<InvalidOperationException>(() => sut.Run());",
                "Assert.ThrowsAny",
            ),
            (
                "Assert.ThrowsExactly<InvalidOperationException>(() => sut.Run());",
                "Assert.Throws",
            ),
            (
                "ClassicAssert.Throws<InvalidOperationException>(() => sut.Run());",
                "Assert.Throws",
            ),
        ] {
            let t = cs(body);
            let got = &t[0].expected_exceptions;
            assert_eq!(got.len(), 1, "{body}: {got:?}");
            assert_eq!(got[0].kind, kind, "{body}");
            assert_eq!(got[0].skeleton, "Assert.Throws#(() => sut.Run())");
            assert!(!t[0].is_vacuous(), "{body}");
        }
        // An assertion that nothing is thrown expects no exception.
        let none = cs("Assert.DoesNotThrow(() => sut.Run());");
        assert!(none[0].expected_exceptions.is_empty());
    }

    #[test]
    fn csharp_assert_throws_reads_its_type_argument() {
        let narrow = cs("Assert.Throws<ArgumentNullException>(() => sut.Run());");
        let wide = cs("Assert.Throws<Exception>(() => sut.Run());");
        let sibling = cs("Assert.Throws<ArgumentException>(() => sut.Run());");
        let n = &narrow[0].expected_exceptions;
        assert_eq!(
            n[0].exception_type.as_deref(),
            Some("ArgumentNullException")
        );
        let w = widened(n, &wide[0].expected_exceptions);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0]
            .detail
            .contains("from `ArgumentNullException` to `Exception`"));
        assert!(widened(n, &sibling[0].expected_exceptions).is_empty());
        assert!(widened(n, n).is_empty());
    }

    #[test]
    fn test_widened_rust_panic_matcher_removed() {
        let b = vec![ExpectedException {
            line: 5,
            skeleton: "#[should_panic]".to_string(),
            kind: "should_panic".to_string(),
            exception_type: None,
            matcher: Some("overflow".to_string()),
        }];
        let h = vec![ExpectedException {
            line: 5,
            skeleton: "#[should_panic]".to_string(),
            kind: "should_panic".to_string(),
            exception_type: None,
            matcher: None,
        }];
        let w = widened(&b, &h);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].line, 5);
        assert!(w[0].detail.contains("matcher was removed"));
    }

    #[test]
    fn test_widened_python_exception_type_and_matcher() {
        let b = vec![ExpectedException {
            line: 10,
            skeleton: "with pytest.raises(#): f(-1)".to_string(),
            kind: "pytest.raises".to_string(),
            exception_type: Some("ValueError".to_string()),
            matcher: Some("negative".to_string()),
        }];
        let h = vec![ExpectedException {
            line: 10,
            skeleton: "with pytest.raises(#): f(-1)".to_string(),
            kind: "pytest.raises".to_string(),
            exception_type: Some("Exception".to_string()),
            matcher: None,
        }];
        let w = widened(&b, &h);
        assert_eq!(w.len(), 1);
        assert!(w[0].detail.contains("widened"));
    }

    #[test]
    fn test_widened_js_to_throw_type_dropped() {
        let b = vec![ExpectedException {
            line: 12,
            skeleton: "expect(() => f()).toThrow(#)".to_string(),
            kind: "toThrow".to_string(),
            exception_type: Some("TypeError".to_string()),
            matcher: None,
        }];
        let h = vec![ExpectedException {
            line: 12,
            skeleton: "expect(() => f()).toThrow(#)".to_string(),
            kind: "toThrow".to_string(),
            exception_type: None,
            matcher: None,
        }];
        let w = widened(&b, &h);
        assert_eq!(w.len(), 1);
        assert!(w[0].detail.contains("type was removed"));
    }

    #[test]
    fn test_widened_csharp_throws_any() {
        let b = vec![ExpectedException {
            line: 20,
            skeleton: "Assert.Throws#(() => {})".to_string(),
            kind: "Assert.Throws".to_string(),
            exception_type: Some("ArgumentException".to_string()),
            matcher: None,
        }];
        let h = vec![ExpectedException {
            line: 20,
            skeleton: "Assert.Throws#(() => {})".to_string(),
            kind: "Assert.ThrowsAny".to_string(),
            exception_type: Some("Exception".to_string()),
            matcher: None,
        }];
        let w = widened(&b, &h);
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn test_negative_control_unchanged() {
        let b = vec![ExpectedException {
            line: 5,
            skeleton: "#[should_panic]".to_string(),
            kind: "should_panic".to_string(),
            exception_type: None,
            matcher: Some("overflow".to_string()),
        }];
        let h = b.clone();
        assert!(widened(&b, &h).is_empty());
    }

    #[test]
    fn test_negative_control_narrowed() {
        let b = vec![ExpectedException {
            line: 10,
            skeleton: "with pytest.raises(#): f(-1)".to_string(),
            kind: "pytest.raises".to_string(),
            exception_type: Some("Exception".to_string()),
            matcher: None,
        }];
        let h = vec![ExpectedException {
            line: 10,
            skeleton: "with pytest.raises(#): f(-1)".to_string(),
            kind: "pytest.raises".to_string(),
            exception_type: Some("ValueError".to_string()),
            matcher: Some("negative".to_string()),
        }];
        assert!(widened(&b, &h).is_empty());
    }

    #[test]
    fn test_negative_control_sibling_exception() {
        let b = vec![ExpectedException {
            line: 10,
            skeleton: "with pytest.raises(#): f(-1)".to_string(),
            kind: "pytest.raises".to_string(),
            exception_type: Some("ValueError".to_string()),
            matcher: None,
        }];
        let h = vec![ExpectedException {
            line: 10,
            skeleton: "with pytest.raises(#): f(-1)".to_string(),
            kind: "pytest.raises".to_string(),
            exception_type: Some("TypeError".to_string()),
            matcher: None,
        }];
        assert!(widened(&b, &h).is_empty());
    }

    #[test]
    fn test_widened_java_assert_throws_to_ancestor() {
        let b = vec![ExpectedException {
            line: 15,
            skeleton: "assertThrows(#, () -> f())".to_string(),
            kind: "assertThrows".to_string(),
            exception_type: Some("IllegalArgumentException".to_string()),
            matcher: None,
        }];
        let h = vec![ExpectedException {
            line: 15,
            skeleton: "assertThrows(#, () -> f())".to_string(),
            kind: "assertThrows".to_string(),
            exception_type: Some("Throwable".to_string()),
            matcher: None,
        }];
        let w = widened(&b, &h);
        assert_eq!(w.len(), 1);
        assert!(w[0].detail.contains("widened"));
    }

    #[test]
    fn test_negative_control_java_sibling_exception() {
        let b = vec![ExpectedException {
            line: 15,
            skeleton: "assertThrows(#, () -> f())".to_string(),
            kind: "assertThrows".to_string(),
            exception_type: Some("IllegalArgumentException".to_string()),
            matcher: None,
        }];
        let h = vec![ExpectedException {
            line: 15,
            skeleton: "assertThrows(#, () -> f())".to_string(),
            kind: "assertThrows".to_string(),
            exception_type: Some("IllegalStateException".to_string()),
            matcher: None,
        }];
        assert!(widened(&b, &h).is_empty());
    }

    // ----- #560: pairing, the class hierarchy, matchers and the forms read -----

    use crate::ast::java::JavaPack;
    use crate::ast::javascript::JavaScriptPack;
    use crate::ast::php::PhpPack;
    use crate::ast::rust::RustPack;

    fn facts(pack: &dyn LanguagePack, path: &str, src: &str) -> Vec<ExpectedException> {
        let tests = pack
            .extract(path, src, &AssertVocabulary::default())
            .expect("extract succeeds")
            .tests;
        assert_eq!(tests.len(), 1, "{tests:?}");
        tests[0].expected_exceptions.clone()
    }

    fn js(body: &str) -> Vec<ExpectedException> {
        facts(
            &JavaScriptPack,
            "tests/sut.test.js",
            &format!("test(\"t\", () => {{\n  {body}\n}});\n"),
        )
    }

    fn java_test(body: &str) -> Vec<ExpectedException> {
        facts(
            &JavaPack,
            "src/test/java/SutTest.java",
            &format!("class SutTest {{\n    @Test\n    void t() {{\n        {body}\n    }}\n}}\n"),
        )
    }

    fn php(body: &str) -> Vec<ExpectedException> {
        facts(
            &PhpPack,
            "tests/SutTest.php",
            &format!(
                "<?php\nclass SutTest extends TestCase {{\n    public function testT(): void {{\n        {body}\n    }}\n}}\n"
            ),
        )
    }

    fn py_body(body: &str) -> Vec<ExpectedException> {
        let indented: String = body.lines().map(|l| format!("    {l}\n")).collect();
        py(&format!("def test_t():\n{indented}"))[0]
            .expected_exceptions
            .clone()
    }

    fn site(
        kind: &str,
        skeleton: &str,
        ty: Option<&str>,
        matcher: Option<&str>,
    ) -> ExpectedException {
        ExpectedException {
            line: 1,
            skeleton: skeleton.to_string(),
            kind: kind.to_string(),
            exception_type: ty.map(str::to_string),
            matcher: matcher.map(str::to_string),
        }
    }

    fn raises(skeleton: &str, ty: &str) -> ExpectedException {
        site("pytest.raises", skeleton, Some(ty), None)
    }

    #[test]
    fn pairing_ignores_the_order_of_unchanged_expectations() {
        let base = vec![raises("a", "ValueError"), raises("b", "Exception")];
        let head = vec![raises("b", "Exception"), raises("a", "ValueError")];
        assert!(widened(&base, &head).is_empty());
        // The same skeleton twice on each side is unchanged too.
        let twice = vec![raises("a", "ValueError"), raises("a", "ValueError")];
        assert!(widened(&twice, &twice).is_empty());
    }

    #[test]
    fn pairing_compares_one_of_two_identical_sites() {
        let base = vec![raises("a", "ValueError"), raises("a", "ValueError")];
        let head = vec![raises("a", "ValueError"), raises("a", "Exception")];
        let w = widened(&base, &head);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].detail.contains("from `ValueError` to `Exception`"));
        assert!(!w[0].dropped);
    }

    #[test]
    fn pairing_follows_a_site_whose_skeleton_changed() {
        let base = vec![raises("with f(-1)", "ValueError")];
        let head = vec![raises("with f(-2)", "Exception")];
        let w = widened(&base, &head);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].detail.contains("from `ValueError` to `Exception`"));
        // Control: the same rewrite with the expectation kept.
        assert!(widened(&base, &[raises("with f(-2)", "ValueError")]).is_empty());
    }

    #[test]
    fn pairing_keeps_same_skeleton_sites_together_when_they_trade_strictness() {
        let base = vec![raises("a", "ValueError"), raises("b", "Exception")];
        let head = vec![raises("a", "Exception"), raises("b", "ValueError")];
        let w = widened(&base, &head);
        assert_eq!(w.len(), 1, "{w:?}");
        assert_eq!(w[0].skeleton, "a");
    }

    #[test]
    fn pairing_gives_each_rewritten_site_a_head_site_of_its_own() {
        // Both rewritten; `LookupError` is satisfied by either head site, `KeyError` only
        // by the first, so the first assignment has to be moved aside.
        let base = vec![raises("a", "LookupError"), raises("b", "KeyError")];
        let head = vec![raises("c", "KeyError"), raises("d", "LookupError")];
        assert!(widened(&base, &head).is_empty());
        // One head site cannot stand for two base sites.
        let w = widened(&base, &[raises("c", "KeyError")]);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].dropped);
    }

    #[test]
    fn pairing_reports_a_dropped_site_in_the_body_and_not_a_removed_attribute() {
        let w = widened(&[raises("a", "ValueError")], &[]);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].dropped);
        assert_eq!(w[0].line, 0);
        assert!(w[0].detail.contains("`ValueError` is no longer checked"));
        // Without `#[should_panic]` the test must pass without panicking.
        let attr = site("should_panic", "#[should_panic]", None, Some("overflow"));
        assert!(widened(&[attr], &[]).is_empty());
        // An added expectation is never a finding.
        assert!(widened(&[], &[raises("a", "Exception")]).is_empty());
    }

    #[test]
    fn pairing_does_not_let_one_php_call_stand_for_another() {
        let class = site(
            "expectException",
            "$this->expectException#",
            Some("DomainException"),
            None,
        );
        let message = site(
            "expectExceptionMessage",
            "$this->expectExceptionMessage#",
            None,
            Some("negative"),
        );
        let w = widened(
            &[class.clone(), message.clone()],
            std::slice::from_ref(&class),
        );
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].dropped);
        // A class named where a message was required is another call, not its replacement.
        let w = widened(&[message], &[class]);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].dropped);
    }

    #[test]
    fn standard_hierarchy_orders_a_class_and_its_ancestors() {
        let moved = |kind: &str, from: &str, to: &str| {
            is_widened(
                &site(kind, "s", Some(from), None),
                &site(kind, "s", Some(to), None),
            )
        };
        for (kind, from, to) in [
            ("pytest.raises", "KeyError", "LookupError"),
            ("assertRaises", "FileNotFoundError", "OSError"),
            ("pytest.raises", "UnicodeDecodeError", "ValueError"),
            // `IOError` is `OSError` under another name.
            ("pytest.raises", "FileNotFoundError", "IOError"),
            ("assertThrows", "FileNotFoundException", "IOException"),
            ("assertThrows", "NumberFormatException", "RuntimeException"),
            (
                "test_expected",
                "IllegalArgumentException",
                "RuntimeException",
            ),
            (
                "Assert.ThrowsAny",
                "ArgumentNullException",
                "ArgumentException",
            ),
            (
                "Assert.ThrowsAny",
                "FileNotFoundException",
                "System.IO.IOException",
            ),
            ("toThrow", "TypeError", "Error"),
            (
                "expectException",
                "InvalidArgumentException",
                "LogicException",
            ),
            ("expectException", "\\DomainException", "\\Exception"),
        ] {
            let got = moved(kind, from, to);
            assert!(got.is_some(), "{kind}: {from} -> {to}");
            // The reverse is a narrowing.
            assert_eq!(moved(kind, to, from), None, "{kind}: {to} -> {from}");
        }
        // Siblings, a qualified spelling, an alias, and two classes no table lists.
        for (kind, from, to) in [
            ("pytest.raises", "ValueError", "TypeError"),
            ("pytest.raises", "KeyError", "IndexError"),
            ("pytest.raises", "ValueError", "builtins.ValueError"),
            ("pytest.raises", "IOError", "OSError"),
            ("pytest.raises", "OrderError", "PaymentError"),
            ("pytest.raises", "OrderError", "ValueError"),
            ("assertThrows", "IOException", "java.io.IOException"),
            (
                "assertThrows",
                "IllegalArgumentException",
                "IllegalStateException",
            ),
            ("toThrow", "TypeError", "RangeError"),
            ("expectException", "DomainException", "\\DomainException"),
            // One language's table says nothing about another's classes.
            ("assertThrows", "KeyError", "LookupError"),
        ] {
            assert_eq!(moved(kind, from, to), None, "{kind}: {from} -> {to}");
        }
    }

    #[test]
    fn exact_type_assertions_use_the_general_names_only() {
        let exact = |kind: &str, ty: &str| site(kind, "s", Some(ty), None);
        for kind in ["Assert.Throws", "assertThrowsExactly"] {
            assert_eq!(
                is_widened(
                    &exact(kind, "ArgumentNullException"),
                    &exact(kind, "ArgumentException")
                ),
                None
            );
            assert!(is_widened(
                &exact(kind, "ArgumentNullException"),
                &exact(kind, "Exception")
            )
            .is_some());
        }
        // Exact to subclass-accepting is a widening; the reverse is not.
        let relaxed = is_widened(
            &exact("assertThrowsExactly", "IllegalStateException"),
            &exact("assertThrows", "IllegalStateException"),
        );
        assert!(relaxed.is_some_and(|d| d.contains("exact type")));
        assert_eq!(
            is_widened(
                &exact("assertThrows", "IllegalStateException"),
                &exact("assertThrowsExactly", "IllegalStateException"),
            ),
            None
        );
    }

    #[test]
    fn tuple_of_classes_is_compared_as_a_set() {
        let t = |ty: &str| raises("s", ty);
        assert_eq!(
            is_widened(&t("ValueError, TypeError"), &t("TypeError, ValueError")),
            None
        );
        assert_eq!(
            is_widened(&t("ValueError, TypeError"), &t("ValueError")),
            None
        );
        assert_eq!(
            is_widened(&t("ValueError, TypeError"), &t("ValueError, KeyError")),
            None
        );
        assert_eq!(
            is_widened(&t("LookupError"), &t("KeyError, IndexError")),
            None
        );
        let grown = is_widened(&t("ValueError"), &t("ValueError, TypeError"));
        assert!(grown.is_some_and(|d| d.contains("now also accepts `TypeError`")));
        assert!(is_widened(&t("ValueError"), &t("TypeError, KeyError")).is_some());
        let parent = is_widened(&t("KeyError"), &t("KeyError, LookupError"));
        assert!(parent.is_some_and(|d| d.contains("from `KeyError` to `LookupError`")));
    }

    #[test]
    fn matcher_that_accepts_every_message_is_no_matcher() {
        let m = |kind: &str, matcher: &str| site(kind, "s", None, Some(matcher));
        for (kind, any) in [
            ("should_panic", ""),
            ("pytest.raises", ""),
            ("pytest.raises", ".*"),
            ("pytest.raises", "^.*$"),
            ("pytest.raises", "(?s).*"),
            ("assertRaises", ".*"),
            ("toThrow", ""),
            ("toThrow", "\u{1}/.*/"),
            ("toThrow", "\u{1}/^.*$/s"),
            ("Assert.ThrowsAny", "*"),
            ("expectExceptionMessage", ""),
        ] {
            let got = is_widened(&m(kind, "negative"), &m(kind, any));
            assert!(
                got.is_some_and(|d| d.contains("accepts any message")),
                "{kind}: {any:?}"
            );
            // Control: from one such matcher to none nothing is lost.
            assert_eq!(
                is_widened(&m(kind, any), &site(kind, "s", None, None)),
                None
            );
        }
        // `.*` is a message like any other where the matcher is a substring.
        assert_eq!(
            is_widened(&m("should_panic", ".*"), &m("should_panic", ".*")),
            None
        );
        assert!(is_widened(
            &m("should_panic", ".*"),
            &site("should_panic", "s", None, None)
        )
        .is_some());
    }

    #[test]
    fn matcher_shortened_to_a_part_of_itself_is_a_widening() {
        let m = |kind: &str, matcher: &str| site(kind, "s", None, Some(matcher));
        for kind in [
            "should_panic",
            "pytest.raises",
            "toThrow",
            "expectExceptionMessage",
        ] {
            let got = is_widened(&m(kind, "negative value"), &m(kind, "value"));
            assert!(got.is_some_and(|d| d.contains("shortened")), "{kind}");
            assert_eq!(
                is_widened(&m(kind, "value"), &m(kind, "negative value")),
                None
            );
            assert_eq!(is_widened(&m(kind, "negative"), &m(kind, "positive")), None);
        }
        // A pattern is not compared as text: one that contains another may match less.
        assert_eq!(
            is_widened(
                &m("pytest.raises", "neg(ative)?"),
                &m("pytest.raises", "neg")
            ),
            None
        );
        assert_eq!(
            is_widened(&m("toThrow", "\u{1}/a.c/"), &m("toThrow", "\u{1}/a./")),
            None
        );
        // Nor is the name of a constant: `MSG` says nothing about `MSG_LONG`.
        assert_eq!(
            is_widened(&m("toThrow", "\u{1}MSG_LONG"), &m("toThrow", "\u{1}MSG")),
            None
        );
    }

    #[test]
    fn matcher_traded_for_a_narrower_class_is_not_a_widening() {
        let b = site("pytest.raises", "s", Some("Exception"), Some("neg"));
        assert_eq!(is_widened(&b, &raises("s", "ValueError")), None);
        let b = site("pytest.raises", "s", Some("LookupError"), Some("neg"));
        assert_eq!(is_widened(&b, &raises("s", "KeyError")), None);
        let b = site("toThrow", "s", None, Some("neg"));
        assert_eq!(
            is_widened(&b, &site("toThrow", "s", Some("TypeError"), None)),
            None
        );
        // Controls: the same class, a sibling, and a general class keep the report.
        let b = site("pytest.raises", "s", Some("ValueError"), Some("neg"));
        assert!(is_widened(&b, &raises("s", "ValueError")).is_some());
        assert!(is_widened(&b, &raises("s", "TypeError")).is_some());
        let b = site("toThrow", "s", None, Some("neg"));
        assert!(is_widened(&b, &site("toThrow", "s", Some("Error"), None)).is_some());
    }

    #[test]
    fn negated_expectation_is_widened_by_naming_a_type() {
        let not = |ty: Option<&str>| site("not.toThrow", "s", ty, None);
        assert_eq!(is_widened(&not(Some("TypeError")), &not(None)), None);
        assert!(is_widened(&not(None), &not(Some("TypeError"))).is_some());
        assert_eq!(is_widened(&not(None), &not(None)), None);
        // A negation and its positive form are not compared, and do not stand for each other.
        let positive = site("toThrow", "s", Some("TypeError"), None);
        assert_eq!(is_widened(&positive, &not(None)), None);
        let w = widened(
            &[site("toThrow", "a", Some("TypeError"), None)],
            &[site("not.toThrow", "b", None, None)],
        );
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].dropped);
    }

    #[test]
    fn rust_should_panic_message_is_read_in_both_forms() {
        for (attr, want) in [
            ("#[should_panic]", None),
            ("#[should_panic(expected = \"overflow\")]", Some("overflow")),
            ("#[should_panic = \"overflow\"]", Some("overflow")),
            ("should_panic(expected = \"overflow\")", Some("overflow")),
            (
                "#[should_panic(expected = r#\"a \"b\" c\"#)]",
                Some("a \"b\" c"),
            ),
            ("#[should_panic(expected = \"\")]", Some("")),
            (
                "#[should_panic(expected = \"say \\\"no\\\"\")]",
                Some("say \\\"no\\\""),
            ),
            // A word inside the message is not a key.
            (
                "#[should_panic = \"expected = \\\"x\\\"\"]",
                Some("expected = \\\"x\\\""),
            ),
            ("#[should_panic(note = \"expected\")]", None),
        ] {
            let got = parse_rust_should_panic(attr, 1);
            assert_eq!(got.matcher.as_deref(), want, "{attr}");
        }
    }

    #[test]
    fn rust_proptest_case_carries_its_should_panic() {
        let got = facts(
            &RustPack,
            "tests/t.rs",
            "proptest! {\n    #[test]\n    #[should_panic(expected = \"overflow\")]\n    fn t(x in 0..10i32) {\n        f(x);\n    }\n}\n",
        );
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].kind, "should_panic");
        assert_eq!(got[0].matcher.as_deref(), Some("overflow"));
    }

    #[test]
    fn python_reads_the_call_form_keyword_tuple_and_string_prefix() {
        let call = py_body("pytest.raises(ValueError, f, -1)");
        assert_eq!(call.len(), 1, "{call:?}");
        assert_eq!(call[0].kind, "pytest.raises");
        assert_eq!(call[0].exception_type.as_deref(), Some("ValueError"));
        assert_eq!(call[0].skeleton, "pytest.raises(#, f, -1)");

        let kw = py_body(
            "with pytest.raises(expected_exception=ValueError, match=r\"neg\"):\n    f(-1)",
        );
        assert_eq!(kw.len(), 1, "{kw:?}");
        assert_eq!(kw[0].exception_type.as_deref(), Some("ValueError"));
        assert_eq!(kw[0].matcher.as_deref(), Some("neg"));

        let tuple = py_body("with pytest.raises((ValueError, TypeError)):\n    f(-1)");
        assert_eq!(
            tuple[0].exception_type.as_deref(),
            Some("ValueError, TypeError")
        );

        let empty = py_body("with pytest.raises(ValueError, match=r\"\"):\n    f(-1)");
        assert_eq!(empty[0].matcher.as_deref(), Some(""));

        // The `with` form is recorded once, not again as a call.
        let with = py_body("with pytest.raises(ValueError):\n    f(-1)");
        assert_eq!(with.len(), 1, "{with:?}");
    }

    #[test]
    fn javascript_reads_types_messages_and_negation() {
        for (call, kind, ty, matcher) in [
            ("expect(() => f()).toThrow();", "toThrow", None, None),
            (
                "expect(() => f()).toThrow(TypeError);",
                "toThrow",
                Some("TypeError"),
                None,
            ),
            (
                "expect(() => f()).toThrowError(TypeError);",
                "toThrow",
                Some("TypeError"),
                None,
            ),
            (
                "expect(() => f()).toThrow(errors.NotFound);",
                "toThrow",
                Some("errors.NotFound"),
                None,
            ),
            (
                "expect(() => f()).toThrow(\"bad\");",
                "toThrow",
                None,
                Some("bad"),
            ),
            (
                "expect(() => f()).toThrow('bad');",
                "toThrow",
                None,
                Some("bad"),
            ),
            (
                "expect(() => f()).toThrow(`bad`);",
                "toThrow",
                None,
                Some("bad"),
            ),
            (
                "expect(() => f()).toThrow(/bad/i);",
                "toThrow",
                None,
                Some("\u{1}/bad/i"),
            ),
            (
                "expect(() => f()).toThrow(ERR_MSG);",
                "toThrow",
                None,
                Some("\u{1}ERR_MSG"),
            ),
            (
                "expect(() => f()).toThrow(messages.ERR_MSG);",
                "toThrow",
                None,
                Some("\u{1}messages.ERR_MSG"),
            ),
            (
                "expect(() => f()).toThrow(message);",
                "toThrow",
                None,
                Some("\u{1}message"),
            ),
            (
                "expect(() => f()).toThrow(new RangeError(\"too big\"));",
                "toThrow",
                Some("RangeError"),
                Some("too big"),
            ),
            (
                "expect(() => f()).not.toThrow();",
                "not.toThrow",
                None,
                None,
            ),
            (
                "expect(() => f()).not.toThrow(TypeError);",
                "not.toThrow",
                Some("TypeError"),
                None,
            ),
            (
                "await expect(p).rejects.toThrow(TypeError);",
                "toThrow",
                Some("TypeError"),
                None,
            ),
            (
                "await expect(p).rejects.not.toThrow();",
                "not.toThrow",
                None,
                None,
            ),
        ] {
            let got = js(call);
            assert_eq!(got.len(), 1, "{call}: {got:?}");
            assert_eq!(got[0].kind, kind, "{call}");
            assert_eq!(got[0].exception_type.as_deref(), ty, "{call}");
            assert_eq!(got[0].matcher.as_deref(), matcher, "{call}");
            assert_eq!(got[0].line, 2, "{call}");
        }
        assert_eq!(
            js("expect(() => f(1)).toThrow(TypeError);")[0].skeleton,
            "expect(() => f(1)).toThrow(#)"
        );
        // A call that is not `toThrow` expects no failure.
        assert!(js("expect(f()).toBe(1);").is_empty());
        // A template with a substitution is still a matcher, and not a string to compare.
        let templ = js("expect(() => f()).toThrow(`bad ${id}`);");
        assert_eq!(templ[0].matcher.as_deref(), Some("\u{1}`bad ${id}`"));
    }

    #[test]
    fn java_reads_assert_throws_its_exact_form_and_the_annotation() {
        let plain = java_test("assertThrows(IllegalStateException.class, () -> sut.run(1));");
        assert_eq!(plain.len(), 1, "{plain:?}");
        assert_eq!(plain[0].kind, "assertThrows");
        assert_eq!(
            plain[0].exception_type.as_deref(),
            Some("IllegalStateException")
        );
        assert_eq!(plain[0].skeleton, "assertThrows(#, () -> sut.run(1))");
        assert_eq!(plain[0].line, 4);

        let exact =
            java_test("assertThrowsExactly(IllegalStateException.class, () -> sut.run(1));");
        assert_eq!(exact[0].kind, "assertThrowsExactly");
        assert_eq!(exact[0].skeleton, plain[0].skeleton);

        let qualified =
            java_test("Assertions.assertThrows(java.io.IOException.class, () -> sut.run(1));");
        assert_eq!(
            qualified[0].exception_type.as_deref(),
            Some("java.io.IOException")
        );

        assert!(java_test("assertEquals(1, sut.run(1));").is_empty());

        for (annotation, want) in [
            (
                "@Test(expected = IllegalStateException.class)",
                Some("IllegalStateException"),
            ),
            (
                "@Test(expected=IllegalStateException.class)",
                Some("IllegalStateException"),
            ),
            (
                "@Test(expected = IllegalStateException.class, timeout = 100)",
                Some("IllegalStateException"),
            ),
            (
                "@Test(timeout = 100, expected = java.io.IOException.class)",
                Some("java.io.IOException"),
            ),
            ("@Test(timeout = 100)", None),
            ("@Test", None),
            // A word in another element's value is not the `expected` element.
            ("@Test(description = \"expected = X.class\")", None),
        ] {
            let got = facts(
                &JavaPack,
                "src/test/java/SutTest.java",
                &format!("class SutTest {{\n    {annotation}\n    public void t() {{\n        sut.run(1);\n    }}\n}}\n"),
            );
            assert_eq!(
                got.first().and_then(|e| e.exception_type.as_deref()),
                want,
                "{annotation}"
            );
            assert_eq!(
                got.len(),
                usize::from(want.is_some()),
                "{annotation}: {got:?}"
            );
        }
    }

    #[test]
    fn csharp_reads_catch_qualified_typeof_and_fluent_forms() {
        for (body, kind, ty, matcher) in [
            ("Assert.Catch<ArgumentException>(() => sut.Run());", "Assert.ThrowsAny", Some("ArgumentException"), None),
            ("Assert.Catch(() => sut.Run());", "Assert.ThrowsAny", None, None),
            (
                "Xunit.Assert.Throws<ArgumentException>(() => sut.Run());",
                "Assert.Throws",
                Some("ArgumentException"),
                None,
            ),
            (
                "Assert.Throws(typeof(ArgumentException), () => sut.Run());",
                "Assert.Throws",
                Some("ArgumentException"),
                None,
            ),
            ("act.Should().Throw<ArgumentException>();", "Assert.ThrowsAny", Some("ArgumentException"), None),
            ("act.Should().ThrowExactly<ArgumentException>();", "Assert.Throws", Some("ArgumentException"), None),
            (
                "act.Should().Throw<ArgumentException>().WithMessage(\"*negative*\");",
                "Assert.ThrowsAny",
                Some("ArgumentException"),
                Some("*negative*"),
            ),
            (
                "act.Should().Throw<ArgumentException>().WithMessage(\"*negative*\").And.ParamName.Should().Be(\"x\");",
                "Assert.ThrowsAny",
                Some("ArgumentException"),
                Some("*negative*"),
            ),
        ] {
            let got = cs(body)[0].expected_exceptions.clone();
            assert_eq!(got.len(), 1, "{body}: {got:?}");
            assert_eq!(got[0].kind, kind, "{body}");
            assert_eq!(got[0].exception_type.as_deref(), ty, "{body}");
            assert_eq!(got[0].matcher.as_deref(), matcher, "{body}");
        }
        // The typeof form pairs with itself whatever class it names.
        let a = cs("Assert.Throws(typeof(ArgumentException), () => sut.Run());");
        let b = cs("Assert.Throws(typeof(Exception), () => sut.Run());");
        assert_eq!(
            a[0].expected_exceptions[0].skeleton,
            b[0].expected_exceptions[0].skeleton
        );
        // `Should()` followed by anything else, and `Throw` off any other receiver, is not one.
        for body in [
            "result.Should().Be(1);",
            "act.Should().NotThrow();",
            "thrower.Throw<ArgumentException>();",
        ] {
            assert!(cs(body)[0].expected_exceptions.is_empty(), "{body}");
        }
    }

    #[test]
    fn php_reads_the_expected_class_message_and_code() {
        let got = php(
            "$this->expectException(\\App\\OrderException::class);\n$this->expectExceptionMessage('negative');\n$this->expectExceptionCode(3);\nsut(-1);",
        );
        assert_eq!(got.len(), 3, "{got:?}");
        assert_eq!(got[0].kind, "expectException");
        assert_eq!(
            got[0].exception_type.as_deref(),
            Some("\\App\\OrderException")
        );
        assert_eq!(got[0].matcher, None);
        assert_eq!(got[1].kind, "expectExceptionMessage");
        assert_eq!(got[1].matcher.as_deref(), Some("negative"));
        assert_eq!(got[2].kind, "expectExceptionCode");
        assert_eq!(got[2].matcher.as_deref(), Some("\u{1}3"));
        let wide = php("$this->expectException(\\Exception::class);\nsut(-1);");
        let w = widened(&got[..1], &wide);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].detail.contains("from `OrderException` to `Exception`"));
    }
}
