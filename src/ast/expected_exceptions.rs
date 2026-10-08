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

use super::ancestry::Ancestry;
use super::bounds::{text, walk};
use super::exception_tables::{self as tables, Hierarchy};
use super::TestFn;
use std::collections::HashMap;
use std::sync::Arc;
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
    /// The matcher must be the whole message (`hasMessage`, `Message.EqualTo`, a string
    /// given to `raise_error`); any other string matcher holds for a message containing it.
    pub whole_message: bool,
    /// The classes the test's file declares, each with a parent it is declared with
    /// (`class OrderError(AppError)` is `("OrderError", "AppError")`), as written.
    pub declared: Arc<Vec<(String, String)>>,
    /// The one call the site guards (`f(-1)` of `with pytest.raises(E): f(-1)`), as
    /// [`call_text`] writes it; `None` when the guarded code is anything else.
    pub guarded_call: Option<String>,
    /// How a C# `Assert.Throws` kind was spelled, where the spelling decides whether the
    /// class is exact ([`is_exact_beside`]): [`THROWS_EXACTLY`], [`THROWS`],
    /// [`THROWS_BESIDE_EXACTLY`], [`THROWS_EXCEPTION`], [`THROWS_OF_MSTEST`],
    /// [`THROWS_EXACT`]. Empty for every other expectation.
    pub form: &'static str,
}

/// `Assert.ThrowsExactly<T>`: exact, and a name MSTest has from 3.8 only.
const THROWS_EXACTLY: &str = "ThrowsExactly";
/// `Assert.Throws<T>` in a file that does not use `Assert.ThrowsExactly`.
const THROWS: &str = "Throws";
/// `Assert.Throws<T>` in a file that uses `Assert.ThrowsExactly`: MSTest 3.8 or later,
/// where it accepts subclasses.
const THROWS_BESIDE_EXACTLY: &str = "Throws beside ThrowsExactly";

/// `Assert.ThrowsException<T>`: exact, and a name of MSTest only.
const THROWS_EXCEPTION: &str = "ThrowsException";
/// `Assert.Throws<T>` in a file whose `using` directives name MSTest: it accepts
/// subclasses.
const THROWS_OF_MSTEST: &str = "Throws of MSTest";
/// `Assert.Throws<T>` in a file whose `using` directives name xUnit or NUnit: exact.
const THROWS_EXACT: &str = "Throws of xUnit or NUnit";

/// An expected exception or panic that was widened between base and head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Widened {
    /// The head line of the widened expectation; 0 when `dropped`.
    pub line: usize,
    pub skeleton: String,
    pub detail: String,
    /// The base expectation has no counterpart at head: nothing in the test checks it.
    pub dropped: bool,
    /// The guarded call of a dropped expectation ([`ExpectedException::guarded_call`]).
    pub guarded_call: Option<String>,
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

/// The node kinds of a call, across the grammars read here.
const CALL_KINDS: &[&str] = &[
    "call",
    "call_expression",
    "method_invocation",
    "invocation_expression",
    "function_call_expression",
    "member_call_expression",
    "scoped_call_expression",
];

/// A call node as text that does not depend on how it is spaced or broken over lines:
/// the text of each token of its subtree, comments left out, joined by one space. Two
/// calls with the same callee and the same arguments have the same text; the content of
/// a string literal is one token and is kept as written.
pub(super) fn call_text(call: Node, src: &str) -> String {
    let mut tokens: Vec<&str> = Vec::new();
    walk(call, &mut |n| {
        if n.kind().contains("comment") {
            return false;
        }
        if n.child_count() == 0 {
            tokens.push(text(n, src));
        }
        true
    });
    let joined = tokens.join(" ");
    super::ancestry::count(joined.len());
    joined
}

/// Every call inside `node`, as [`call_text`] writes it, in the order of the source, and
/// for each node inside `node`, by its id, where the calls inside it stand in that list.
/// The calls inside a node are one run of the list, so a reader that wants the calls of
/// a node inside `node` takes that run and does not list them again.
///
/// The tokens under `node` are read once: the text of a call is the tokens between the
/// first and the last of its own, so a call nested in another is not walked again for
/// each call that holds it. The list is still the text of every call, and a call nested
/// in `n` others is written `n + 1` times.
pub(super) fn calls_in(node: Node, src: &str) -> (Vec<String>, HashMap<usize, (usize, usize)>) {
    /// A node yet to be read, or one whose children have been.
    enum Visit<'t> {
        Enter(Node<'t>, bool),
        /// The node, where its run of calls began, and for a call outside a comment its
        /// place in the list and its first token.
        Leave(Node<'t>, usize, Option<(usize, usize)>),
    }
    let mut out: Vec<String> = Vec::new();
    let mut runs = HashMap::new();
    // The tokens outside comments, as `call_text` reads them.
    let mut tokens: Vec<&str> = Vec::new();
    let mut stack = vec![Visit::Enter(node, false)];
    while let Some(visit) = stack.pop() {
        super::ancestry::count(1);
        let (n, in_comment) = match visit {
            Visit::Enter(n, in_comment) => (n, in_comment),
            Visit::Leave(n, from, call) => {
                if let Some((at, first)) = call {
                    out[at] = tokens[first..].join(" ");
                    super::ancestry::count(out[at].len());
                }
                runs.insert(n.id(), (from, out.len()));
                continue;
            }
        };
        let comment = n.kind().contains("comment");
        let is_call = CALL_KINDS.contains(&n.kind());
        let call = (is_call && !in_comment && !comment).then_some((out.len(), tokens.len()));
        stack.push(Visit::Leave(n, out.len(), call));
        if is_call {
            // A call under a comment node is not among the tokens: it is read alone.
            out.push(match call {
                Some(_) => String::new(),
                None => call_text(n, src),
            });
        }
        let in_comment = in_comment || comment;
        if n.child_count() == 0 && !in_comment {
            tokens.push(text(n, src));
        }
        let mut cursor = n.walk();
        let children: Vec<Node> = n.children(&mut cursor).collect();
        stack.extend(
            children
                .into_iter()
                .rev()
                .map(|c| Visit::Enter(c, in_comment)),
        );
    }
    (out, runs)
}

/// The call that is all `node` does: `node` itself, or the one statement of a block, the
/// body of a lambda, an awaited or parenthesized call. `None` for code that does anything
/// else (two statements, an assignment, a call inside a larger expression).
fn sole_call(node: Node) -> Option<Node> {
    let mut current = node;
    // Each step goes to a child, so the walk ends; the bound guards a grammar surprise.
    for _ in 0..32 {
        if CALL_KINDS.contains(&current.kind()) {
            return Some(current);
        }
        current = match current.kind() {
            "arrow_function" | "function_expression" | "lambda_expression" | "lambda" => {
                current.child_by_field_name("body")?
            }
            "block"
            | "statement_block"
            | "expression_statement"
            | "parenthesized_expression"
            | "await_expression"
            | "await" => {
                let mut cursor = current.walk();
                let mut named = current
                    .named_children(&mut cursor)
                    .filter(|c| !c.kind().contains("comment"));
                let only = named.next()?;
                if named.next().is_some() {
                    return None;
                }
                only
            }
            _ => return None,
        };
    }
    None
}

/// [`sole_call`] as [`ExpectedException::guarded_call`] keeps it.
fn guarded_call(node: Option<Node>, src: &str) -> Option<String> {
    node.and_then(sole_call).map(|call| call_text(call, src))
}

/// How a grammar writes a class declaration and the parents it names.
struct ClassSyntax {
    /// Node kinds of a class declaration, its name in the `name` field.
    declarations: &'static [&'static str],
    /// Reads the parents a declaration names, as written.
    parents: fn(Node, &str) -> Vec<String>,
}

fn named_children_text(node: Node, src: &str) -> Vec<String> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|c| !c.kind().contains("comment"))
        .map(|c| text(c, src).to_string())
        .collect()
}

fn child_of_kind<'t>(node: Node<'t>, kinds: &[&str]) -> Option<Node<'t>> {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find(|c| kinds.contains(&c.kind()));
    found
}

/// `class A(B, C)`: the positional arguments; `metaclass=..` is not a parent.
fn python_parents(class: Node, src: &str) -> Vec<String> {
    let Some(bases) = class.child_by_field_name("superclasses") else {
        return Vec::new();
    };
    let mut cursor = bases.walk();
    bases
        .named_children(&mut cursor)
        .filter(|c| !matches!(c.kind(), "keyword_argument" | "comment"))
        .map(|c| text(c, src).to_string())
        .collect()
}

/// `class A extends B`: the superclass; an interface is not a class an exception extends.
fn java_parents(class: Node, src: &str) -> Vec<String> {
    class
        .child_by_field_name("superclass")
        .map(|s| named_children_text(s, src))
        .unwrap_or_default()
}

/// `class A : B, IC`: every entry of the base list (the grammar does not tell the base
/// class from an interface).
fn csharp_parents(class: Node, src: &str) -> Vec<String> {
    child_of_kind(class, &["base_list"])
        .map(|l| named_children_text(l, src))
        .unwrap_or_default()
}

/// `class A extends B`: JavaScript puts the expression in the heritage, TypeScript in
/// an `extends_clause` inside it.
fn js_parents(class: Node, src: &str) -> Vec<String> {
    let Some(heritage) = child_of_kind(class, &["class_heritage"]) else {
        return Vec::new();
    };
    let extends = child_of_kind(heritage, &["extends_clause"]);
    let value = match extends {
        Some(clause) => clause
            .child_by_field_name("value")
            .or_else(|| clause.named_child(0)),
        None => heritage.named_child(0),
    };
    value
        .map(|v| text(v, src).to_string())
        .into_iter()
        .collect()
}

/// `class A extends B`.
fn php_parents(class: Node, src: &str) -> Vec<String> {
    child_of_kind(class, &["base_clause"])
        .map(|b| named_children_text(b, src))
        .unwrap_or_default()
}

const PYTHON_CLASSES: ClassSyntax = ClassSyntax {
    declarations: &["class_definition"],
    parents: python_parents,
};
const JAVA_CLASSES: ClassSyntax = ClassSyntax {
    declarations: &["class_declaration"],
    parents: java_parents,
};
const CSHARP_CLASSES: ClassSyntax = ClassSyntax {
    declarations: &["class_declaration"],
    parents: csharp_parents,
};
const JS_CLASSES: ClassSyntax = ClassSyntax {
    declarations: &["class_declaration", "class", "abstract_class_declaration"],
    parents: js_parents,
};
const PHP_CLASSES: ClassSyntax = ClassSyntax {
    declarations: &["class_declaration"],
    parents: php_parents,
};

/// Gives every expectation of `tests` the class declarations of its file: each class the
/// file declares, with each parent the declaration names. Only the file is read; a class
/// declared in another file, or imported, stays a class whose parents are not known.
fn attach_declared(root: Node, src: &str, tests: &mut [TestFn], syntax: &ClassSyntax) {
    if tests.iter().all(|t| t.expected_exceptions.is_empty()) {
        return;
    }
    let mut declared: Vec<(String, String)> = Vec::new();
    walk(root, &mut |node| {
        if syntax.declarations.contains(&node.kind()) {
            if let Some(name) = node.child_by_field_name("name") {
                for parent in (syntax.parents)(node, src) {
                    declared.push((text(name, src).to_string(), parent));
                }
            }
        }
        true
    });
    if declared.is_empty() {
        return;
    }
    let declared = Arc::new(declared);
    for test in tests {
        for e in &mut test.expected_exceptions {
            e.declared = Arc::clone(&declared);
        }
        if let Some(e) = &mut test.should_panic {
            e.declared = Arc::clone(&declared);
        }
    }
}

/// The language an expectation kind belongs to: which hierarchy and matcher rules apply.
/// Kotlin's and Scala's forms are kept under Java's kinds: they name the same classes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    Python,
    Java,
    Js,
    CSharp,
    Php,
    Ruby,
    Cpp,
    Go,
    Swift,
    ObjC,
    Other,
}

fn family(kind: &str) -> Family {
    match kind {
        "pytest.raises" | "assertRaises" | "assertWarns" => Family::Python,
        "assertThrows" | "assertThrowsExactly" | "test_expected" | "doesNotThrow" => Family::Java,
        "toThrow" | "not.toThrow" => Family::Js,
        "Assert.Throws" | "Assert.ThrowsAny" | "Throws.Nothing" => Family::CSharp,
        "expectException"
        | "expectExceptionMessage"
        | "expectExceptionCode"
        | "expectExceptionMessageMatches" => Family::Php,
        "raise_error" | "not.raise_error" | "assert_raises" => Family::Ruby,
        "EXPECT_THROW" | "EXPECT_NO_THROW" => Family::Cpp,
        "go.Error" | "go.NoError" | "go.Panics" | "go.NotPanics" => Family::Go,
        "swift.throws" | "swift.noThrow" => Family::Swift,
        "objc.throws" | "objc.noThrow" => Family::ObjC,
        _ => Family::Other,
    }
}

/// Kinds that accept the named class only, and no subclass of it.
fn is_exact(kind: &str) -> bool {
    matches!(kind, "Assert.Throws" | "assertThrowsExactly")
}

/// Whether `e` accepts the named class only, read beside `other`, the expectation it
/// is compared with across the change.
///
/// `Assert.Throws<T>` is exact in xUnit and NUnit. In MSTest it exists from 3.8 only, and
/// accepts subclasses. Whose `Assert` a file's is comes from its `using` directives
/// ([`csharp_framework`]): in an xUnit or NUnit file `Assert.Throws<T>` is exact, in an
/// MSTest file it accepts subclasses, whatever stands on the other side.
///
/// A file whose directives name none of them (global usings, a fragment) is read by what
/// it calls: `Assert.ThrowsExactly` is a name of MSTest 3.8 and later, and
/// `Assert.ThrowsException` a name of MSTest only. There `Assert.Throws<T>` accepts
/// subclasses in a file that uses `Assert.ThrowsExactly`, and when the expectation on
/// the other side of the change is an `Assert.ThrowsExactly`, an `Assert.ThrowsException`
/// or an `Assert.Throws` that accepts subclasses: the same call means the same on both
/// sides. Otherwise it is exact.
fn is_exact_beside(e: &ExpectedException, other: &ExpectedException) -> bool {
    let mstest = |x: &ExpectedException| {
        matches!(
            x.form,
            THROWS_EXACTLY | THROWS_BESIDE_EXACTLY | THROWS_EXCEPTION | THROWS_OF_MSTEST
        )
    };
    is_exact(&e.kind)
        && !matches!(e.form, THROWS_BESIDE_EXACTLY | THROWS_OF_MSTEST)
        && !(e.form == THROWS && mstest(other))
}

/// Kinds that state the test passes when nothing (or nothing of the type) is thrown.
fn is_negated(kind: &str) -> bool {
    matches!(
        kind,
        "not.toThrow"
            | "doesNotThrow"
            | "Throws.Nothing"
            | "not.raise_error"
            | "EXPECT_NO_THROW"
            | "go.NoError"
            | "go.NotPanics"
            | "swift.noThrow"
            | "objc.noThrow"
    )
}

/// Kinds written on the test rather than in its body. Removing one changes what the test
/// is (it must now pass without the failure), which is not an expectation left unchecked.
fn is_attribute(kind: &str) -> bool {
    matches!(kind, "should_panic" | "test_expected" | "xfail")
}

/// Expectations that can stand for each other when a test is rewritten. PHP states the
/// class, the message and the code in separate calls, which never replace one another,
/// an expected warning is not an expected exception, and in Go a panic is not a returned
/// error.
fn interchangeable(b: &str, h: &str) -> bool {
    let alone = |k: &str| {
        matches!(family(k), Family::Php | Family::Other)
            || matches!(k, "assertWarns" | "go.Panics" | "go.NotPanics")
    };
    family(b) == family(h) && is_negated(b) == is_negated(h) && (b == h || !(alone(b) || alone(h)))
}

/// Ancestor rank for exception types across supported languages:
/// 2: Root exception hierarchy (`BaseException`, `Throwable`, Ruby's `Exception`).
/// 1: General exception hierarchy (`Exception`, `Error`, `System.Exception`,
///    `StandardError`, C++ `std::exception`).
/// 0: Specific exception (`ValueError`, `IllegalArgumentException`, `TypeError`, `CustomError`, etc.).
///
/// Ruby and C++ have the general names of their own language only: an `Error` there is
/// a class of the project. Go has none: a sentinel or an error type is compared by name.
/// Swift has `any Error`, as [`swift_type`] writes the protocol every error conforms to,
/// and Objective-C has `NSException`.
fn ancestor_rank(fam: Family, type_name: &str) -> usize {
    match (fam, type_name) {
        (Family::ObjC, "NSException") => 1,
        (Family::ObjC, _) => 0,
        (Family::Go, _) => 0,
        (Family::Swift, "any Error") => 1,
        (Family::Swift, _) => 0,
        (Family::Cpp, "exception") => 1,
        (Family::Cpp, _) => 0,
        (Family::Ruby, "Exception") => 2,
        (Family::Ruby, "StandardError") => 1,
        (Family::Ruby, _) => 0,
        (_, "BaseException" | "Throwable") => 2,
        (_, "Exception" | "Error" | "StandardError") => 1,
        _ => 0,
    }
}

fn hierarchy(fam: Family) -> Hierarchy {
    match fam {
        Family::Python => tables::PYTHON,
        Family::Java => tables::JAVA,
        Family::CSharp => tables::DOTNET,
        Family::Js => tables::JS,
        Family::Php => tables::PHP,
        Family::Ruby => tables::RUBY,
        Family::Cpp => tables::CPP,
        Family::Go | Family::Swift | Family::ObjC | Family::Other => &[],
    }
}

/// The qualifiers under which a language's standard classes are written. A class
/// qualified by anything else is a class of the project, whatever its last segment is.
fn standard_qualifier(fam: Family, qualifier: &str) -> bool {
    match fam {
        Family::Python => qualifier == "builtins",
        Family::Java => {
            matches!(
                qualifier,
                "java.lang"
                    | "java.lang.reflect"
                    | "java.io"
                    | "java.net"
                    | "java.nio"
                    | "java.nio.charset"
                    | "java.nio.file"
                    | "java.util"
                    | "java.util.concurrent"
                    | "java.util.regex"
                    | "java.util.zip"
                    | "java.time"
                    | "java.time.format"
                    // Kotlin's and Scala's aliases of the `java.lang` classes.
                    | "kotlin"
                    | "scala"
            )
        }
        Family::CSharp => matches!(
            qualifier.strip_prefix("global::").unwrap_or(qualifier),
            "System"
                | "System.IO"
                | "System.Collections.Generic"
                | "System.Threading"
                | "System.Threading.Tasks"
        ),
        Family::Js => matches!(qualifier, "globalThis" | "window" | "global" | "self"),
        Family::Cpp => matches!(qualifier.trim_start_matches(':'), "std" | "std::filesystem"),
        // `\LogicException` and `::StandardError` name the root namespace: no qualifier.
        Family::Php | Family::Ruby | Family::Go | Family::Swift | Family::ObjC | Family::Other => {
            false
        }
    }
}

/// One class an expectation or a class declaration names.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TypeName {
    /// The last segment: `IOException` of `java.io.IOException`.
    name: String,
    /// Written with a qualifier.
    qualified: bool,
    /// May be the standard class of this name: written bare, or under a qualifier of the
    /// standard library.
    standard: bool,
}

/// Reads a class as written (`java.io.IOException`, `\LogicException`, `errors.NotFound`,
/// `std::out_of_range`). Python keeps `IOError`, `EnvironmentError` and `WindowsError` as
/// other names of `OSError`.
fn type_name(fam: Family, written: &str) -> Option<TypeName> {
    let written = written.trim().trim_end_matches(".class").trim();
    let (qualifier, last) = match written.rfind(['.', '\\', ':']) {
        Some(at) => (
            written[..at].trim_end_matches(['.', '\\', ':']),
            written[at + 1..].trim(),
        ),
        None => ("", written),
    };
    if last.is_empty() {
        return None;
    }
    let qualified = !qualifier.is_empty();
    let standard = !qualified || standard_qualifier(fam, qualifier);
    let name = match (fam, last) {
        (Family::Python, "IOError" | "EnvironmentError" | "WindowsError") if standard => "OSError",
        _ => last,
    };
    Some(TypeName {
        name: name.to_string(),
        qualified,
        standard,
    })
}

/// The classes an expectation accepts, each once.
fn type_names(e: &ExpectedException) -> Vec<TypeName> {
    let fam = family(&e.kind);
    let mut names: Vec<TypeName> = Vec::new();
    for part in e.exception_type.iter().flat_map(|raw| raw.split(", ")) {
        if let Some(t) = type_name(fam, part) {
            if !names.iter().any(|n| n.name == t.name) {
                names.push(t);
            }
        }
    }
    names
}

/// What is known about classes when two expectations are compared: the language's
/// standard table, and the class declarations of the test's file on each side.
struct Classes<'a> {
    fam: Family,
    declared: [&'a [(String, String)]; 2],
}

impl Classes<'_> {
    /// The direct parents of `class` as one side's file sees them. A class the file
    /// declares has the parents it is declared with, also under a standard name; any
    /// other class has the standard table's, when it may be a standard class.
    fn parents(&self, class: &TypeName, declared: &[(String, String)]) -> Vec<TypeName> {
        let own: Vec<TypeName> = declared
            .iter()
            .filter(|(child, _)| !class.qualified && *child == class.name)
            .filter_map(|(_, parent)| type_name(self.fam, parent))
            .collect();
        if !own.is_empty() || !class.standard {
            return own;
        }
        hierarchy(self.fam)
            .iter()
            .filter(|(child, _)| *child == class.name)
            .map(|(_, parent)| TypeName {
                name: parent.to_string(),
                qualified: false,
                standard: true,
            })
            .collect()
    }

    /// Whether `ancestor` stands strictly above `class`, in the file of either side. The
    /// change may rewrite a class declaration: the side on which the relation holds is
    /// the one that makes a widening visible.
    fn above(&self, ancestor: &TypeName, class: &TypeName) -> bool {
        self.declared.iter().any(|declared| {
            let mut seen: Vec<String> = Vec::new();
            let mut open = vec![class.clone()];
            while let Some(current) = open.pop() {
                if seen.contains(&current.name) {
                    continue;
                }
                seen.push(current.name.clone());
                for parent in self.parents(&current, declared) {
                    if parent.name == ancestor.name {
                        return true;
                    }
                    open.push(parent);
                }
            }
            false
        })
    }

    /// Whether `ancestor` is known to accept everything `class` does and more: by the
    /// general names of [`ancestor_rank`], by the standard hierarchy, or by the classes
    /// the file declares. An exact-type assertion accepts no subclass, so for it only a
    /// move to a general name is a known widening.
    fn known_ancestor(&self, ancestor: &TypeName, class: &TypeName, exact: bool) -> bool {
        ancestor.name != class.name
            && (ancestor_rank(self.fam, &ancestor.name) > ancestor_rank(self.fam, &class.name)
                || (!exact && self.above(ancestor, class)))
    }
}

/// How the accepted classes moved between base and head.
enum TypeChange {
    Same,
    /// Head accepts a class base did not: `from .. to ..` or `now also accepts ..`.
    Widened(String),
    /// Head names no class where base did: the class base named first.
    Dropped(String),
    /// Head accepts strictly less: a known subclass, fewer classes, or a specific class
    /// where base named none.
    Narrowed,
    /// A replacement whose relation is not known.
    Unrelated,
}

fn type_change(b: &ExpectedException, h: &ExpectedException) -> TypeChange {
    let fam = family(&h.kind);
    let classes = Classes {
        fam,
        declared: [b.declared.as_slice(), h.declared.as_slice()],
    };
    let exact = is_exact_beside(h, b);
    let (bt, ht) = (type_names(b), type_names(h));
    match (bt.first(), ht.is_empty()) {
        (None, true) => return TypeChange::Same,
        (Some(first), true) => return TypeChange::Dropped(first.name.clone()),
        (None, false) => {
            return if ht.iter().all(|t| ancestor_rank(fam, &t.name) == 0) {
                TypeChange::Narrowed
            } else {
                TypeChange::Unrelated
            };
        }
        (Some(_), false) => {}
    }
    // A head class is covered when base accepted it already: the same class, or a class
    // below one base named.
    let uncovered: Vec<&TypeName> = ht
        .iter()
        .filter(|t| {
            !bt.iter()
                .any(|b| b.name == t.name || classes.known_ancestor(b, t, false))
        })
        .collect();
    if uncovered.is_empty() {
        let same = bt.len() == ht.len() && bt.iter().all(|b| ht.iter().any(|h| h.name == b.name));
        return if same {
            TypeChange::Same
        } else {
            TypeChange::Narrowed
        };
    }
    for t in &uncovered {
        if let Some(below) = bt.iter().find(|b| classes.known_ancestor(t, b, exact)) {
            return TypeChange::Widened(format!("from `{}` to `{}`", below.name, t.name));
        }
    }
    // Unknown relations: one class replaced by one other is a substitution, and is not
    // reported. More new classes than classes given up is a longer list of accepted ones.
    let given_up = bt
        .iter()
        .filter(|b| !ht.iter().any(|h| h.name == b.name))
        .count();
    if uncovered.len() > given_up {
        let added = uncovered[uncovered.len() - 1];
        return TypeChange::Widened(format!("now also accepts `{}`", added.name));
    }
    TypeChange::Unrelated
}

/// Whether `s` is free of pattern metacharacters, so it matches as the literal it is.
fn is_literal_pattern(s: &str) -> bool {
    !s.chars().any(|c| "\\^$.|?*+()[]{}".contains(c))
}

/// The pattern of a matcher written as a regular expression literal (JavaScript, Ruby).
fn js_regex_body(m: &str) -> Option<&str> {
    let rest = m.strip_prefix(OPAQUE)?.strip_prefix('/')?;
    let end = rest.rfind('/')?;
    Some(&rest[..end])
}

/// The pattern of a PHP regular expression string: what stands between its delimiters.
fn php_regex_body(m: &str) -> Option<&str> {
    let delimiter = m.chars().next().filter(|c| !c.is_alphanumeric())?;
    let rest = &m[delimiter.len_utf8()..];
    let end = rest.rfind(delimiter)?;
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
        // A whole message that is empty still constrains the message to be empty.
        _ if e.whole_message => false,
        Family::Python => any_pattern(m),
        Family::CSharp => m.is_empty() || m == "*",
        Family::Php if e.kind == "expectExceptionMessageMatches" => {
            php_regex_body(m).is_some_and(any_pattern)
        }
        _ => m.is_empty(),
    };
    (!accepts_all).then_some(m)
}

/// Whether the string a matcher holds is read as a pattern: a regular expression for
/// `pytest.raises(match=..)`, `assertRaisesRegex` and `expectExceptionMessageMatches`, a
/// wildcard for `WithMessage`.
fn matcher_is_pattern(e: &ExpectedException) -> bool {
    !e.whole_message
        && (matches!(family(&e.kind), Family::Python | Family::CSharp)
            || e.kind == "expectExceptionMessageMatches")
}

enum MatcherChange {
    Same,
    /// Head has no constraint on the message where base had one.
    Dropped,
    /// Head's message is a proper part of base's, or the same text no longer required to
    /// be the whole message, so it matches every message base did.
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
        (Some(x), Some(y)) if x == y => match (b.whole_message, h.whole_message) {
            (true, false) if !x.starts_with(OPAQUE) => MatcherChange::Loosened,
            (false, true) if !x.starts_with(OPAQUE) => MatcherChange::Tightened,
            _ => MatcherChange::Same,
        },
        (Some(x), Some(y)) => {
            // Only two plain strings are compared as text: an expression's text and a
            // pattern say nothing about which messages they match.
            let literal = |e: &ExpectedException, m: &str| {
                !m.starts_with(OPAQUE) && (!matcher_is_pattern(e) || is_literal_pattern(m))
            };
            if !literal(b, x) || !literal(h, y) {
                return MatcherChange::Unrelated;
            }
            // A whole message accepts that one message; a contained one accepts every
            // message it is a part of.
            match (b.whole_message, h.whole_message) {
                (_, false) if x.contains(y) => MatcherChange::Loosened,
                (false, _) if y.contains(x) => MatcherChange::Tightened,
                _ => MatcherChange::Unrelated,
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
        family(&b.kind) == family(&h.kind) && is_exact_beside(b, h) && !is_exact_beside(h, b);
    let matcher_weaker = matches!(matcher, MatcherChange::Dropped | MatcherChange::Loosened);
    // A matcher where base had none. It rejects some failures base accepted, and takes
    // nothing back from a type that now accepts failures base rejected.
    let matcher_gained = effective_matcher(b).is_none() && effective_matcher(h).is_some();

    // A matcher given up for a narrower class is a trade, not a widening: the new
    // expectation rejects failures the old one accepted.
    if matcher_weaker && matches!(types, TypeChange::Narrowed) && !exactness_lost {
        return None;
    }
    if matcher_weaker {
        let what = match matcher {
            MatcherChange::Dropped if h.matcher.is_some() => "now accepts any message",
            MatcherChange::Dropped => "was removed",
            _ if b.whole_message && !h.whole_message => {
                "is no longer the whole message and matches more messages"
            }
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
        TypeChange::Dropped(was) if matcher_gained => Some(format!(
            "expected exception type `{was}` was removed; a message matcher replaced it, and any class with that message passes"
        )),
        TypeChange::Dropped(_) => Some("expected exception type was removed".to_string()),
        TypeChange::Widened(tw) if matcher_gained => Some(format!(
            "expected exception type widened {tw}; the matcher added beside it does not narrow the type"
        )),
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

/// [`widened`] for the two sides of one test, less the dropped expectations whose guarded
/// call the head side now asserts the result of ([`reasserted`]).
pub fn widened_in(base: &TestFn, head: &TestFn) -> Vec<Widened> {
    let mut out = widened(&base.expected_exceptions, &head.expected_exceptions);
    out.retain(|w| {
        !(w.dropped
            && w.guarded_call
                .as_deref()
                .is_some_and(|call| reasserted(call, base, head)))
    });
    out
}

/// Whether `head` gained an equality assertion on `call`: more of its expected-value
/// assertions hold that call than `base`'s do. The call that had to fail is then
/// asserted to return a value, which is a changed behaviour stated in the test and not an
/// expectation left unchecked.
fn reasserted(call: &str, base: &TestFn, head: &TestFn) -> bool {
    let asserting = |t: &TestFn| {
        t.expectations
            .iter()
            .filter(|e| e.calls.iter().any(|c| c == call))
            .count()
    };
    asserting(head) > asserting(base)
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
            && b.whole_message == h.whole_message
            && b.form == h.form
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
                guarded_call: None,
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
                    guarded_call: None,
                });
            }
            None if !is_attribute(&b.kind) => out.push(Widened {
                line: 0,
                skeleton: b.skeleton.clone(),
                detail: match (type_names(b).first(), effective_matcher(b)) {
                    (Some(t), _) => format!("expected exception `{}` is no longer checked", t.name),
                    (None, Some(m)) => format!(
                        "expected message `{}` is no longer checked",
                        m.trim_start_matches(OPAQUE)
                    ),
                    (None, None) => "expected failure is no longer checked".to_string(),
                },
                dropped: true,
                guarded_call: b.guarded_call.clone(),
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
    if let Some(t) = super::innermost_test(tests, line) {
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
        ..Default::default()
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

/// Python: `pytest.raises(...)` (in a `with` statement or as a call), `raises(...)` when
/// the file imports it from pytest, and `self.assertRaises(...)` / `assertRaisesRegex` /
/// `assertWarns` / `assertWarnsRegex`.
pub fn python<'t>(root: Node<'t>, anc: &Ancestry<'t>, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    let bare = PytestBare {
        raises: python_bare(root, src, "raises"),
        warns: python_bare(root, src, "warns"),
    };
    let bare = &bare;
    walk(root, &mut |node| {
        match node.kind() {
            "with_statement" => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "with_clause" {
                        let mut c2 = child.walk();
                        for item in child.children(&mut c2) {
                            if item.kind() == "with_item" {
                                inspect_python_with_item(node, item, src, bare, tests);
                            }
                        }
                    } else if child.kind() == "with_item" {
                        inspect_python_with_item(node, child, src, bare, tests);
                    }
                }
                true
            }
            "call" => {
                // Standalone calls like self.assertRaises(ValueError, f, -1). The context
                // manager of a `with` item, bound with `as` or not, is read above.
                if !is_python_with_item_value(node, anc) {
                    inspect_python_standalone_call(node, src, bare, tests);
                }
                true
            }
            _ => true,
        }
    });
    attach_declared(root, src, tests, &PYTHON_CLASSES);
}

/// The bare names the file imports pytest's `raises` and `warns` under.
struct PytestBare {
    raises: Vec<String>,
    warns: Vec<String>,
}

/// The names under which the file imports pytest's `wanted` (`raises`, `warns`):
/// `from pytest import raises` binds `raises`, `from pytest import raises as throws`
/// binds `throws`. A `raises` that comes from anywhere else is not read.
fn python_bare(root: Node, src: &str, wanted: &str) -> Vec<String> {
    let mut names = Vec::new();
    walk(root, &mut |node| {
        if node.kind() != "import_from_statement" {
            return true;
        }
        let from_pytest = node
            .child_by_field_name("module_name")
            .is_some_and(|m| text(m, src) == "pytest");
        if !from_pytest {
            return false;
        }
        let mut cursor = node.walk();
        for name in node.children_by_field_name("name", &mut cursor) {
            let (imported, bound) = match name.kind() {
                "aliased_import" => (
                    name.child_by_field_name("name"),
                    name.child_by_field_name("alias"),
                ),
                _ => (Some(name), Some(name)),
            };
            if let (Some(imported), Some(bound)) = (imported, bound) {
                if text(imported, src) == wanted {
                    names.push(text(bound, src).to_string());
                }
            }
        }
        false
    });
    names
}

/// Whether `call` is the context manager of a `with` item: its value, or the value an
/// `as` binding wraps (`with f() as e:` parses as `with_item > as_pattern > call`).
fn is_python_with_item_value<'t>(call: Node<'t>, anc: &Ancestry<'t>) -> bool {
    let Some(parent) = anc.parent(call) else {
        return false;
    };
    match parent.kind() {
        "with_item" => true,
        "as_pattern" => {
            anc.parent(parent).is_some_and(|g| g.kind() == "with_item")
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
                "expected_exception" | "expected_warning" => {
                    out.exception_type = Some(python_exception_type(val, src))
                }
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

/// A Python expected-failure call, by its callee.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PythonForm {
    /// `pytest.raises(..)`, or `raises(..)` imported from pytest.
    Raises,
    /// `pytest.warns(..)`, or `warns(..)` imported from pytest: an expected warning, of
    /// the kind `assertWarns` is, with the arguments of `pytest.raises`.
    Warns,
    /// `assertRaises`, `assertRaisesRegex`, `assertWarns`, `assertWarnsRegex`: the kind,
    /// the name as written, and whether the second argument is a pattern.
    Unittest(&'static str, &'static str, bool),
}

fn python_unittest_form(name: &str) -> Option<PythonForm> {
    Some(match name {
        "assertRaises" => PythonForm::Unittest("assertRaises", "assertRaises", false),
        "assertRaisesRegex" => PythonForm::Unittest("assertRaises", "assertRaisesRegex", true),
        "assertWarns" => PythonForm::Unittest("assertWarns", "assertWarns", false),
        "assertWarnsRegex" => PythonForm::Unittest("assertWarns", "assertWarnsRegex", true),
        _ => return None,
    })
}

/// Reads the callee of a call: `pytest.raises` on the module (`pytest`, `x.pytest`), a
/// bare name the file imports from pytest (`bare`), or a unittest method on any receiver.
fn python_form(func: Node, src: &str, bare: &PytestBare) -> Option<PythonForm> {
    match func.kind() {
        "identifier" => {
            let name = text(func, src);
            if bare.raises.iter().any(|b| b == name) {
                Some(PythonForm::Raises)
            } else if bare.warns.iter().any(|b| b == name) {
                Some(PythonForm::Warns)
            } else {
                python_unittest_form(name)
            }
        }
        "attribute" => {
            let name = text(func.child_by_field_name("attribute")?, src);
            if matches!(name, "raises" | "warns") {
                let object = func.child_by_field_name("object")?;
                let module = match object.kind() {
                    "identifier" => object,
                    "attribute" => object.child_by_field_name("attribute")?,
                    _ => return None,
                };
                let form = if name == "raises" {
                    PythonForm::Raises
                } else {
                    PythonForm::Warns
                };
                (text(module, src) == "pytest").then_some(form)
            } else {
                python_unittest_form(name)
            }
        }
        _ => None,
    }
}

fn inspect_python_with_item(
    with_stmt: Node,
    item: Node,
    src: &str,
    bare: &PytestBare,
    tests: &mut [TestFn],
) {
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
    let Some(form) = python_form(func, src, bare) else {
        return;
    };
    let Some(args) = call.child_by_field_name("arguments") else {
        return;
    };
    // Skeleton: the `with` statement's text with the call's arguments replaced by (#).
    let (ws, we) = (with_stmt.start_byte(), with_stmt.end_byte());
    let (as_pos, ae_pos) = (args.start_byte(), args.end_byte());
    let masked = (as_pos >= ws && ae_pos <= we)
        .then(|| format!("{}(#){}", &src[ws..as_pos], &src[ae_pos..we]));
    let guarded = guarded_call(with_stmt.child_by_field_name("body"), src);

    match form {
        PythonForm::Raises | PythonForm::Warns => {
            let (kind, name) = if form == PythonForm::Raises {
                ("pytest.raises", "raises")
            } else {
                ("assertWarns", "warns")
            };
            let parsed = pytest_raises_arguments(args, src);
            let skeleton_raw = masked
                .unwrap_or_else(|| format!("with pytest.{name}(#): {}", text(with_stmt, src)));
            attribute(
                tests,
                ExpectedException {
                    line: call.start_position().row + 1,
                    skeleton: collapse_ws(&skeleton_raw),
                    kind: kind.to_string(),
                    exception_type: parsed.exception_type,
                    matcher: parsed.matcher,
                    guarded_call: guarded,
                    ..Default::default()
                },
            );
        }
        PythonForm::Unittest(kind, written, is_regex) => {
            let mut cursor = args.walk();
            let named: Vec<Node> = args.named_children(&mut cursor).collect();

            let exception_type = named.first().map(|a| python_exception_type(*a, src));
            let matcher = if is_regex && named.len() >= 2 {
                Some(matcher_value(named[1], src))
            } else {
                None
            };

            // The `Regex` spelling is the same assertion with a pattern: one skeleton.
            let mut raw =
                masked.unwrap_or_else(|| format!("with self.{kind}(#): {}", text(with_stmt, src)));
            if is_regex {
                raw = raw.replace(written, kind);
            }
            attribute(
                tests,
                ExpectedException {
                    line: call.start_position().row + 1,
                    skeleton: collapse_ws(&raw),
                    kind: kind.to_string(),
                    exception_type,
                    matcher,
                    guarded_call: guarded,
                    ..Default::default()
                },
            );
        }
    }
}

fn inspect_python_standalone_call(call: Node, src: &str, bare: &PytestBare, tests: &mut [TestFn]) {
    let Some(func) = call.child_by_field_name("function") else {
        return;
    };
    let Some(form) = python_form(func, src, bare) else {
        return;
    };
    let Some(args) = call.child_by_field_name("arguments") else {
        return;
    };
    let (kind, written, is_regex) = match form {
        PythonForm::Raises | PythonForm::Warns => {
            // The call form: `pytest.raises(ValueError, f, -1)`.
            let (kind, name) = if form == PythonForm::Raises {
                ("pytest.raises", "raises")
            } else {
                ("assertWarns", "warns")
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
                    skeleton: collapse_ws(&format!("pytest.{name}(#, {})", rest.join(", "))),
                    kind: kind.to_string(),
                    exception_type: parsed.exception_type,
                    matcher: parsed.matcher,
                    ..Default::default()
                },
            );
            return;
        }
        PythonForm::Unittest(kind, written, is_regex) => (kind, written, is_regex),
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
        raw = raw.replace(written, kind);
    }
    let skeleton = collapse_ws(&raw);

    attribute(
        tests,
        ExpectedException {
            line: call.start_position().row + 1,
            skeleton,
            kind: kind.to_string(),
            exception_type,
            matcher,
            ..Default::default()
        },
    );
}

/// How a name passed to `toThrow` reads: a class is written in PascalCase, and anything
/// else (a constant such as `ERR_MSG`, a variable) holds the message or the error.
fn js_name_is_class(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_uppercase()) && name.chars().any(|c| c.is_lowercase())
}

/// JavaScript / TypeScript: `expect(...).toThrow(...)` and `.toThrowError(...)` with their
/// `.not` forms, Chai's `expect(...).to.throw(...)`, and `assert.throws(...)` /
/// `assert.rejects(...)` / `assert.doesNotThrow(...)` of Node's `assert` and of Chai.
pub fn javascript<'t>(root: Node<'t>, anc: &Ancestry<'t>, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    let modules = js_bindings(root, anc, src);
    walk(root, &mut |node| {
        if node.kind() != "call_expression" {
            return true;
        }
        let Some(callee) = node.child_by_field_name("function") else {
            return true;
        };
        // `assertThrows(..)` / `assertRejects(..)` imported by name from Deno's standard
        // assertion module.
        if callee.kind() == "identifier" {
            let method = match text(callee, src) {
                "assertThrows" => "throws",
                "assertRejects" => "rejects",
                _ => return true,
            };
            let from_std = modules.iter().any(|(local, module)| {
                local == text(callee, src) && super::javascript::is_std_assert_module(module)
            });
            if from_std {
                inspect_js_assert(node, method, JsAssert::DenoStd, src, tests);
            }
            return true;
        }
        if callee.kind() != "member_expression" {
            return true;
        }
        let (Some(prop), Some(obj)) = (
            callee.child_by_field_name("property"),
            callee.child_by_field_name("object"),
        ) else {
            return true;
        };
        let method = text(prop, src);
        match method {
            "toThrow" | "toThrowError" => inspect_js_expect(node, obj, false, src, tests),
            "throw" | "throws" | "Throw" | "rejects" | "doesNotThrow" | "doesNotReject" => {
                if let Some(library) = js_assert_library(obj, src, &modules) {
                    inspect_js_assert(node, method, library, src, tests);
                } else if matches!(method, "throw" | "throws" | "Throw") {
                    inspect_js_expect(node, obj, true, src, tests);
                }
            }
            _ => {}
        }
        true
    });
    attach_declared(root, src, tests, &JS_CLASSES);
}

/// The module each name of the file is bound to by an `import` or a `require`:
/// `(local name, module)`.
pub(super) fn js_bindings<'t>(
    root: Node<'t>,
    anc: &Ancestry<'t>,
    src: &str,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    walk(root, &mut |node| match node.kind() {
        "import_statement" => {
            let module = node
                .child_by_field_name("source")
                .and_then(|s| string_literal(s, src));
            if let Some(module) = module {
                walk(node, &mut |n| match n.kind() {
                    // `import x from`, `import * as x from`: the bound identifier.
                    "identifier"
                        if anc.parent(n).is_some_and(|p| {
                            matches!(p.kind(), "import_clause" | "namespace_import")
                        }) =>
                    {
                        out.push((text(n, src).to_string(), module.clone()));
                        false
                    }
                    "import_specifier" => {
                        let bound = n
                            .child_by_field_name("alias")
                            .or_else(|| n.child_by_field_name("name"));
                        if let Some(bound) = bound {
                            out.push((text(bound, src).to_string(), module.clone()));
                        }
                        false
                    }
                    _ => true,
                });
            }
            false
        }
        "variable_declarator" => {
            let (Some(name), Some(value)) = (
                node.child_by_field_name("name"),
                node.child_by_field_name("value"),
            ) else {
                return true;
            };
            // `require("m")`, or a member of it: `require("m").strict`.
            let call = if value.kind() == "member_expression" {
                value.child_by_field_name("object").unwrap_or(value)
            } else {
                value
            };
            let required = call.kind() == "call_expression"
                && call
                    .child_by_field_name("function")
                    .is_some_and(|f| f.kind() == "identifier" && text(f, src) == "require");
            let module = required
                .then(|| call.child_by_field_name("arguments")?.named_child(0))
                .flatten()
                .and_then(|a| string_literal(a, src));
            let Some(module) = module else {
                return true;
            };
            match name.kind() {
                "identifier" => out.push((text(name, src).to_string(), module)),
                "object_pattern" => {
                    let mut cursor = name.walk();
                    for entry in name.named_children(&mut cursor) {
                        let bound = match entry.kind() {
                            "shorthand_property_identifier_pattern" => Some(entry),
                            "pair_pattern" => entry.child_by_field_name("value"),
                            _ => None,
                        };
                        if let Some(bound) = bound.filter(|b| b.kind().contains("identifier")) {
                            out.push((text(bound, src).to_string(), module.clone()));
                        }
                    }
                }
                _ => {}
            }
            false
        }
        _ => true,
    });
    out
}

/// Whose `assert` a receiver is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum JsAssert {
    /// Node's `assert` module: the argument after the code is a class, a pattern, an
    /// object or a function that validates the error; a string is the assertion's message.
    Node,
    /// Chai's `assert`: a class and a message matcher, in that order, each optional.
    Chai,
    /// `assertThrows(fn, Class, includes, message)` of Deno's standard assertion module.
    DenoStd,
}

/// Reads the receiver of `<receiver>.throws(..)`: a name the file binds to Node's
/// `assert` module or to Chai, or a member of one (`assert.strict`, `chai.assert`). A
/// name bound to nothing the file shows is Node's when it is spelled `assert`, and is
/// not an assertion otherwise (`stub.throws(..)` configures a double).
fn js_assert_library(receiver: Node, src: &str, modules: &[(String, String)]) -> Option<JsAssert> {
    let base = match receiver.kind() {
        "identifier" => receiver,
        "member_expression" => {
            let member = receiver.child_by_field_name("property")?;
            if !matches!(text(member, src), "assert" | "strict") {
                return None;
            }
            receiver
                .child_by_field_name("object")
                .filter(|o| o.kind() == "identifier")?
        }
        _ => return None,
    };
    let name = text(base, src);
    match modules.iter().find(|(local, _)| local == name) {
        Some((_, module)) => match module.as_str() {
            "chai" => Some(JsAssert::Chai),
            "assert" | "node:assert" | "assert/strict" | "node:assert/strict" => {
                Some(JsAssert::Node)
            }
            _ => None,
        },
        None => (name == "assert" && receiver.kind() == "identifier").then_some(JsAssert::Node),
    }
}

/// What the arguments after the code say about the failure.
#[derive(Default)]
struct JsConstraint {
    exception_type: Option<String>,
    matcher: Option<String>,
    whole_message: bool,
}

impl JsConstraint {
    /// Reads one argument as `toThrow` and Chai take it: a class (`TypeError`,
    /// `errors.NotFound`, `new RangeError("..")`), or a constraint on the message.
    fn read(&mut self, arg: Node, src: &str) {
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
                    if self.exception_type.is_none() {
                        self.exception_type = Some(text(arg, src).to_string());
                    }
                } else if self.matcher.is_none() {
                    self.matcher = Some(matcher_value(arg, src));
                }
            }
            "new_expression" => {
                // `toThrow(new RangeError("too big"))`: the class and its message.
                if let Some(ctor) = arg.child_by_field_name("constructor") {
                    if self.exception_type.is_none() {
                        self.exception_type = Some(text(ctor, src).to_string());
                    }
                }
                if let Some(first) = arg
                    .child_by_field_name("arguments")
                    .and_then(|a| a.named_child(0))
                {
                    if self.matcher.is_none() {
                        self.matcher = Some(matcher_value(first, src));
                    }
                }
            }
            // A string (a template without a substitution is the string it spells), a
            // regular expression, or any other expression: a constraint on the failure.
            _ => {
                if self.matcher.is_none() {
                    self.matcher = Some(matcher_value(arg, src));
                }
            }
        }
    }

    /// Reads the argument Node's `assert.throws` takes after the code. A string there is
    /// the message of the assertion itself and constrains nothing.
    fn read_node(&mut self, arg: Node, src: &str) {
        match arg.kind() {
            "string" | "template_string" | "comment" => {}
            "object" => {
                let mut cursor = arg.walk();
                let mut other = false;
                for pair in arg.named_children(&mut cursor) {
                    let (Some(key), Some(value)) = (
                        pair.child_by_field_name("key"),
                        pair.child_by_field_name("value"),
                    ) else {
                        other = true;
                        continue;
                    };
                    let key =
                        string_literal(key, src).unwrap_or_else(|| text(key, src).to_string());
                    match key.as_str() {
                        // `{ name: "RangeError" }` names the class by its `name`.
                        "name" => {
                            self.exception_type = Some(
                                string_literal(value, src)
                                    .unwrap_or_else(|| text(value, src).to_string()),
                            );
                        }
                        // A string must be the whole message; a pattern is searched for.
                        "message" => {
                            self.whole_message = string_literal(value, src).is_some();
                            self.matcher = Some(matcher_value(value, src));
                        }
                        _ => other = true,
                    }
                }
                // Other properties (`code`, ..) constrain the error too: kept as text.
                if other && self.matcher.is_none() {
                    self.matcher = Some(format!("{OPAQUE}{}", text(arg, src)));
                }
            }
            _ => self.read(arg, src),
        }
    }
}

/// `expect(code).toThrow(..)` and Chai's `expect(code).to.throw(..)`. `chai` requires the
/// chain to start at an `expect(..)` call: `stub.throws(..)` is not an expectation.
fn inspect_js_expect(node: Node, obj: Node, chai: bool, src: &str, tests: &mut [TestFn]) {
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
    // Chai's `should` style: `(() => f()).should.throw(E)`, the subject before `.should`.
    let should = chai.then(|| js_should_subject(obj, src)).flatten();
    let expect_call = (link.kind() == "call_expression")
        .then_some(link)
        .filter(|c| {
            c.child_by_field_name("function").is_some_and(|f| {
                let name = if f.kind() == "member_expression" {
                    f.child_by_field_name("property")
                } else {
                    Some(f)
                };
                name.is_some_and(|n| text(n, src) == "expect")
            })
        });
    if chai && expect_call.is_none() && should.is_none() {
        return;
    }

    let mut constraint = JsConstraint::default();
    if let Some(args) = node.child_by_field_name("arguments") {
        let mut cursor = args.walk();
        for arg in args.named_children(&mut cursor) {
            constraint.read(arg, src);
        }
    }
    let code = expect_call
        .and_then(|c| c.child_by_field_name("arguments"))
        .and_then(|a| a.named_child(0))
        .or(should);

    // Skeleton: obj text + ".toThrow(#)"
    let obj_text = text(obj, src);
    let skeleton = collapse_ws(&format!("{obj_text}.toThrow(#)"));

    attribute(
        tests,
        ExpectedException {
            line: node.start_position().row + 1,
            skeleton,
            kind: if negated { "not.toThrow" } else { "toThrow" }.to_string(),
            exception_type: constraint.exception_type,
            matcher: constraint.matcher,
            guarded_call: guarded_call(code, src),
            ..Default::default()
        },
    );
}

/// The subject of a Chai `should` chain: what stands before the first `.should` of the
/// member chain `chain` (`fn` of `fn.should.not.throw`).
fn js_should_subject<'t>(chain: Node<'t>, src: &str) -> Option<Node<'t>> {
    let mut link = chain;
    let mut subject = None;
    while link.kind() == "member_expression" {
        let object = link.child_by_field_name("object")?;
        if link
            .child_by_field_name("property")
            .is_some_and(|p| text(p, src) == "should")
        {
            subject = Some(object);
        }
        link = object;
    }
    subject
}

/// `assert.throws(code, ..)`, `assert.rejects(code, ..)` and their `doesNot` forms.
fn inspect_js_assert(node: Node, method: &str, library: JsAssert, src: &str, tests: &mut [TestFn]) {
    let Some(args) = node.child_by_field_name("arguments") else {
        return;
    };
    let mut cursor = args.walk();
    let named: Vec<Node> = args
        .named_children(&mut cursor)
        .filter(|a| a.kind() != "comment")
        .collect();
    let Some(code) = named.first() else {
        return;
    };
    let negated = matches!(method, "doesNotThrow" | "doesNotReject");
    let mut constraint = JsConstraint::default();
    match library {
        // Node rethrows an error that is not of the class given to `doesNotThrow`: the
        // class changes the report, not what passes.
        JsAssert::Node if negated => {}
        JsAssert::Node => {
            if let Some(arg) = named.get(1) {
                constraint.read_node(*arg, src);
            }
        }
        // `assert.throws(fn, Class, matcher, message)`: what follows the matcher is the
        // message of the assertion.
        JsAssert::Chai => {
            for arg in named.iter().skip(1).take(2) {
                constraint.read(*arg, src);
            }
        }
        // `assertThrows(fn, Class, includes, message)` takes the class and a text the
        // message must hold, as Chai's does; `assertThrows(fn, message)` constrains
        // nothing, the text being the message of the assertion.
        JsAssert::DenoStd => {
            let message_only = named
                .get(1)
                .is_some_and(|a| matches!(a.kind(), "string" | "template_string"));
            if !message_only {
                for arg in named.iter().skip(1).take(2) {
                    constraint.read(*arg, src);
                }
            }
        }
    }
    let name = match method {
        "rejects" | "doesNotReject" => "rejects",
        _ => "throws",
    };
    attribute(
        tests,
        ExpectedException {
            line: node.start_position().row + 1,
            skeleton: collapse_ws(&format!("assert.{name}({})#", text(*code, src))),
            kind: if negated { "not.toThrow" } else { "toThrow" }.to_string(),
            exception_type: constraint.exception_type,
            matcher: constraint.matcher,
            whole_message: constraint.whole_message,
            guarded_call: guarded_call(Some(*code), src),
            ..Default::default()
        },
    );
}

/// Java: `assertThrows(...)`, `assertThrowsExactly(...)`, `@Test(expected = ...)`, and
/// AssertJ's `assertThatThrownBy(..)`, `assertThatExceptionOfType(..)` and
/// `assertThatCode(..)` chains.
pub fn java<'t>(root: Node<'t>, anc: &Ancestry<'t>, src: &str, tests: &mut [TestFn]) {
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
                if matches!(
                    name_text,
                    "assertThatThrownBy" | "assertThatExceptionOfType" | "assertThatCode"
                ) || assertj_typed_entry(name_text).is_some()
                {
                    inspect_java_assertj(node, anc, name_text, src, tests);
                    return true;
                }
                if matches!(name_text, "catchThrowable" | "catchThrowableOfType") {
                    inspect_java_catch_throwable(node, anc, src, tests);
                    return true;
                }
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
                        ..Default::default()
                    },
                );
                true
            }
            _ => true,
        }
    });
    attach_declared(root, src, tests, &JAVA_CLASSES);
}

/// The calls chained on `root`, innermost first: each is `(method name, arguments)` of a
/// `method_invocation` whose object is the call before it.
fn java_chain<'t, 's>(
    root: Node<'t>,
    anc: &Ancestry<'t>,
    src: &'s str,
) -> Vec<(&'s str, Option<Node<'t>>)> {
    let mut links = Vec::new();
    let mut link = root;
    while let Some(outer) = anc.parent(link).filter(|p| {
        p.kind() == "method_invocation"
            && p.child_by_field_name("object")
                .is_some_and(|o| o.id() == link.id())
    }) {
        let name = outer
            .child_by_field_name("name")
            .map_or("", |n| text(n, src));
        links.push((name, outer.child_by_field_name("arguments")));
        link = outer;
    }
    links
}

/// The class a `Foo.class` argument names.
fn java_class_argument(args: Option<Node>, src: &str) -> Option<String> {
    let first = args?.named_child(0)?;
    Some(
        text(first, src)
            .trim_end_matches(".class")
            .trim()
            .to_string(),
    )
}

/// The class an AssertJ typed entry point stands for: `assertThatIllegalArgumentException()`
/// is `assertThatExceptionOfType(IllegalArgumentException.class)`.
fn assertj_typed_entry(name: &str) -> Option<&'static str> {
    Some(match name {
        "assertThatIllegalArgumentException" => "IllegalArgumentException",
        "assertThatNullPointerException" => "NullPointerException",
        "assertThatIllegalStateException" => "IllegalStateException",
        "assertThatIOException" => "IOException",
        "assertThatIndexOutOfBoundsException" => "IndexOutOfBoundsException",
        "assertThatReflectiveOperationException" => "ReflectiveOperationException",
        "assertThatRuntimeException" => "RuntimeException",
        "assertThatException" => "Exception",
        _ => return None,
    })
}

/// AssertJ: `assertThatThrownBy(code)` and `assertThatCode(code)` with `.isInstanceOf(X.class)`
/// / `.isExactlyInstanceOf(X.class)` / `.hasMessage(..)` / `.hasMessageContaining(..)`;
/// `assertThatExceptionOfType(X.class).isThrownBy(code)` with `.withMessage(..)` /
/// `.withMessageContaining(..)`, also from a typed entry point ([`assertj_typed_entry`]);
/// and `assertThatCode(code).doesNotThrowAnyException()`, which states that nothing is
/// thrown.
fn inspect_java_assertj<'t>(
    root: Node<'t>,
    anc: &Ancestry<'t>,
    entry: &str,
    src: &str,
    tests: &mut [TestFn],
) {
    let typed = assertj_typed_entry(entry);
    let entry = if typed.is_some() {
        "assertThatExceptionOfType"
    } else {
        entry
    };
    let args = root.child_by_field_name("arguments");
    let links = java_chain(root, anc, src);
    let mut constraint = AssertjConstraint::default();
    let mut code = args.map_or("", |a| text(a, src)).to_string();
    if entry == "assertThatExceptionOfType" {
        constraint.exception_type = typed
            .map(str::to_string)
            .or_else(|| java_class_argument(args, src));
        code.clear();
    }
    constraint.constrained = entry != "assertThatCode";
    for (name, link_args) in &links {
        match *name {
            "doesNotThrowAnyException" if entry == "assertThatCode" => {
                attribute(
                    tests,
                    ExpectedException {
                        line: root.start_position().row + 1,
                        skeleton: collapse_ws(&format!("assertThatCode{code}.doesNotThrow")),
                        kind: "doesNotThrow".to_string(),
                        ..Default::default()
                    },
                );
                return;
            }
            "isThrownBy" if entry == "assertThatExceptionOfType" => {
                code = link_args.map_or("", |a| text(a, src)).to_string();
            }
            _ => constraint.read(name, *link_args, src),
        }
    }
    // `assertThatCode(code)` followed by nothing read here states no expectation.
    if !constraint.constrained {
        return;
    }
    attribute(tests, constraint.expectation(root, &code));
}

/// What the calls chained on an AssertJ assertion of a thrown exception require of it.
#[derive(Default)]
struct AssertjConstraint {
    exact: bool,
    exception_type: Option<String>,
    matcher: Option<String>,
    whole_message: bool,
    /// A call read here constrains the exception.
    constrained: bool,
}

impl AssertjConstraint {
    /// Reads one chained call: `.isInstanceOf(X.class)`, `.hasMessage("..")`, ..
    fn read(&mut self, name: &str, args: Option<Node>, src: &str) {
        let first = args.and_then(|a| a.named_child(0));
        match name {
            "isInstanceOf" | "isExactlyInstanceOf" if self.exception_type.is_none() => {
                self.exception_type = java_class_argument(args, src);
                self.exact = name == "isExactlyInstanceOf";
            }
            "hasMessage" | "withMessage" if self.matcher.is_none() => {
                self.matcher = first.map(|a| matcher_value(a, src));
                self.whole_message = true;
            }
            "hasMessageContaining" | "withMessageContaining" if self.matcher.is_none() => {
                self.matcher = first.map(|a| matcher_value(a, src));
            }
            // A constraint on the message whose reach is not compared with another's.
            "hasMessageStartingWith"
            | "hasMessageEndingWith"
            | "hasMessageMatching"
            | "withMessageStartingWith"
            | "withMessageEndingWith"
            | "withMessageMatching"
                if self.matcher.is_none() =>
            {
                self.matcher = first.map(|a| format!("{OPAQUE}{name}({})", text(a, src)));
            }
            _ => return,
        }
        self.constrained = true;
    }

    /// The expectation of the site at `at` that guards `code` (the argument list of
    /// `assertThatThrownBy(..)`, as written).
    fn expectation(self, at: Node, code: &str) -> ExpectedException {
        ExpectedException {
            line: at.start_position().row + 1,
            skeleton: collapse_ws(&format!("assertThatThrownBy{code}#")),
            kind: if self.exact {
                "assertThrowsExactly"
            } else {
                "assertThrows"
            }
            .to_string(),
            exception_type: self.exception_type,
            matcher: self.matcher,
            whole_message: self.whole_message,
            ..Default::default()
        }
    }
}

/// AssertJ `Throwable thrown = catchThrowable(code)` and
/// `X e = catchThrowableOfType(code, X.class)` (the class before or after the code),
/// with the assertions the same method makes on the caught value:
/// `assertThat(thrown).isInstanceOf(X.class).hasMessage("..")`. The call alone returns
/// `null` when nothing is thrown, so it is an expectation only beside an `assertThat` on
/// the value it is bound to.
fn inspect_java_catch_throwable<'t>(
    call: Node<'t>,
    anc: &Ancestry<'t>,
    src: &str,
    tests: &mut [TestFn],
) {
    let bound = anc.parent(call).and_then(|p| match p.kind() {
        "variable_declarator" => p.child_by_field_name("name"),
        "assignment_expression" => p.child_by_field_name("left"),
        _ => None,
    });
    let Some(bound) = bound.filter(|b| b.kind() == "identifier") else {
        return;
    };
    let Some(args) = call.child_by_field_name("arguments") else {
        return;
    };
    let mut cursor = args.walk();
    let named: Vec<Node> = args.named_children(&mut cursor).collect();
    let Some(code) = named.iter().find(|a| a.kind() != "class_literal") else {
        return;
    };
    let mut constraint = AssertjConstraint {
        exception_type: named
            .iter()
            .find(|a| a.kind() == "class_literal")
            .map(|c| text(*c, src).trim_end_matches(".class").trim().to_string()),
        ..Default::default()
    };
    let mut method = call;
    while let Some(parent) = anc.parent(method) {
        method = parent;
        if method.kind() == "method_declaration" {
            break;
        }
    }
    let mut asserted = false;
    walk(method, &mut |node| {
        let on_value = node.kind() == "method_invocation"
            && node
                .child_by_field_name("name")
                .is_some_and(|n| text(n, src) == "assertThat")
            && node.child_by_field_name("arguments").is_some_and(|a| {
                a.named_child_count() == 1
                    && a.named_child(0).is_some_and(|v| {
                        v.kind() == "identifier" && text(v, src) == text(bound, src)
                    })
            });
        if on_value {
            asserted = true;
            for (name, link_args) in java_chain(node, anc, src) {
                constraint.read(name, link_args, src);
            }
        }
        true
    });
    if asserted {
        let code = format!("({})", text(*code, src));
        attribute(tests, constraint.expectation(call, &code));
    }
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
        ..Default::default()
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

/// The test framework a C# file's `using` directives name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CsharpFramework {
    XunitOrNunit,
    MsTest,
}

/// Whose `Assert` a file's is: `using Xunit;` or `using NUnit.Framework;`, or
/// `using Microsoft.VisualStudio.TestTools.UnitTesting;` (also `global using`). `None`
/// when the file names none of them, or both kinds. An alias and a `using static` bind
/// another name and are not read.
fn csharp_framework(root: Node, src: &str) -> Option<CsharpFramework> {
    let mut found: Vec<CsharpFramework> = Vec::new();
    walk(root, &mut |node| {
        if node.kind() != "using_directive" {
            return true;
        }
        // An alias has its own name first, and a `using static` names a class.
        let namespace = node.named_child(0).map_or("", |n| text(n, src));
        let framework = match collapse_ws(namespace).replace(' ', "").as_str() {
            "Xunit" | "NUnit.Framework" => Some(CsharpFramework::XunitOrNunit),
            "Microsoft.VisualStudio.TestTools.UnitTesting" => Some(CsharpFramework::MsTest),
            _ => None,
        };
        if let Some(framework) = framework.filter(|f| !found.contains(f)) {
            found.push(framework);
        }
        false
    });
    match found.as_slice() {
        [one] => Some(*one),
        _ => None,
    }
}

/// C#: `Assert.Throws<...>` / `Assert.ThrowsAny<...>` / `Assert.Catch<...>`, their async,
/// exact, `typeof` and MSTest spellings, and FluentAssertions' `.Should().Throw<...>()`.
/// A call is an expected exception only as a member of an assertion class or of a
/// `Should()` chain: `mock.Setup(..).Throws(..)` configures a double and asserts nothing.
pub fn csharp<'t>(root: Node<'t>, anc: &Ancestry<'t>, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    let framework = csharp_framework(root, src);
    // Whether the file calls `Assert.ThrowsExactly`, a name of MSTest 3.8 and later.
    let mut uses_throws_exactly = false;
    walk(root, &mut |node| {
        if node.kind() == "invocation_expression" {
            let access = node
                .child_by_field_name("function")
                .filter(|f| f.kind() == "member_access_expression");
            let named = access.and_then(|a| {
                Some((
                    a.child_by_field_name("expression")?,
                    a.child_by_field_name("name")?,
                ))
            });
            if let Some((receiver, name)) = named {
                uses_throws_exactly |= is_csharp_assert_class(receiver, src)
                    && matches!(
                        text(csharp_member(name).0, src),
                        "ThrowsExactly" | "ThrowsExactlyAsync"
                    );
            }
        }
        !uses_throws_exactly
    });
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
            inspect_csharp_fluent(node, anc, subject, text(method, src), type_args, src, tests);
            return true;
        }
        if !is_csharp_assert_class(receiver, src) {
            return true;
        }
        if matches!(text(method, src), "That" | "ThatAsync") {
            inspect_csharp_constraint(node, src, tests);
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
        let form = match text(method, src) {
            "ThrowsExactly" | "ThrowsExactlyAsync" => THROWS_EXACTLY,
            "Throws" | "ThrowsAsync" if framework == Some(CsharpFramework::MsTest) => {
                THROWS_OF_MSTEST
            }
            "Throws" | "ThrowsAsync" if framework.is_some() => THROWS_EXACT,
            "Throws" | "ThrowsAsync" if uses_throws_exactly => THROWS_BESIDE_EXACTLY,
            "Throws" | "ThrowsAsync" => THROWS,
            "ThrowsException" | "ThrowsExceptionAsync" => THROWS_EXCEPTION,
            _ => "",
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
                form,
                ..Default::default()
            },
        );
        true
    });
    attach_declared(root, src, tests, &CSHARP_CLASSES);
}

/// The expression an `argument` node wraps.
fn csharp_argument_value(arg: Node) -> Node {
    if arg.kind() == "argument" {
        arg.named_child(0).unwrap_or(arg)
    } else {
        arg
    }
}

/// One member of an NUnit constraint expression: its name, its type argument list and,
/// for a call, its arguments.
struct ConstraintLink<'t> {
    name: &'t str,
    type_args: Option<Node<'t>>,
    args: Option<Node<'t>>,
}

/// The members of `Throws.TypeOf<T>().With.Message.EqualTo("..")` after its root, in
/// the order they are written, and the root's name.
fn csharp_constraint_chain<'t>(
    expr: Node<'t>,
    src: &'t str,
) -> Option<(&'t str, Vec<ConstraintLink<'t>>)> {
    let mut links = Vec::new();
    let mut current = expr;
    // Each step goes to a child, so the walk ends; the bound guards a grammar surprise.
    for _ in 0..64 {
        let (access, args) = match current.kind() {
            "identifier" => {
                links.reverse();
                return Some((text(current, src), links));
            }
            "invocation_expression" => (
                current.child_by_field_name("function")?,
                current.child_by_field_name("arguments"),
            ),
            "member_access_expression" => (current, None),
            _ => return None,
        };
        if access.kind() != "member_access_expression" {
            return None;
        }
        let (method, type_args) = csharp_member(access.child_by_field_name("name")?);
        links.push(ConstraintLink {
            name: text(method, src),
            type_args,
            args,
        });
        current = access.child_by_field_name("expression")?;
    }
    None
}

/// NUnit: `Assert.That(code, Throws.TypeOf<T>())` (the exact class),
/// `Throws.InstanceOf<T>()` (the class or a subclass), `Throws.Exception` (any), the named
/// constraints (`Throws.ArgumentException`, ..., the exact class), `.With.Message.EqualTo(..)`
/// / `.Contains(..)`, and `Throws.Nothing`, which states that nothing is thrown.
fn inspect_csharp_constraint(call: Node, src: &str, tests: &mut [TestFn]) {
    let Some(args) = call.child_by_field_name("arguments") else {
        return;
    };
    let mut cursor = args.walk();
    let named: Vec<Node> = args
        .named_children(&mut cursor)
        .filter(|a| a.kind() != "comment")
        .map(csharp_argument_value)
        .collect();
    let (Some(code), Some(constraint)) = (named.first(), named.get(1)) else {
        return;
    };
    let Some((root, links)) = csharp_constraint_chain(*constraint, src) else {
        return;
    };
    if root != "Throws" {
        return;
    }
    let skeleton = collapse_ws(&format!("Assert.That({}, Throws#)", text(*code, src)));
    let line = call.start_position().row + 1;
    let mut kind = "Assert.ThrowsAny";
    let mut exception_type = None;
    let mut matcher = None;
    let mut whole_message = false;
    let mut on_message = false;
    for link in &links {
        let first = link
            .args
            .and_then(|a| a.named_child(0))
            .map(csharp_argument_value);
        match link.name {
            "Nothing" => {
                attribute(
                    tests,
                    ExpectedException {
                        line,
                        skeleton,
                        kind: "Throws.Nothing".to_string(),
                        ..Default::default()
                    },
                );
                return;
            }
            "TypeOf" | "InstanceOf" if exception_type.is_none() => {
                // `TypeOf<T>()`, or `TypeOf(typeof(T))`.
                exception_type = link
                    .type_args
                    .and_then(|l| l.named_child(0))
                    .or_else(|| {
                        first
                            .filter(|a| a.kind() == "typeof_expression")
                            .and_then(|t| t.named_child(0))
                    })
                    .map(|t| text(t, src).to_string());
                if link.name == "TypeOf" {
                    kind = "Assert.Throws";
                }
            }
            "ArgumentException"
            | "ArgumentNullException"
            | "InvalidOperationException"
            | "TargetInvocationException"
                if exception_type.is_none() =>
            {
                exception_type = Some(link.name.to_string());
                kind = "Assert.Throws";
            }
            "Message" => on_message = true,
            "EqualTo" if on_message && matcher.is_none() => {
                matcher = first.map(|a| matcher_value(a, src));
                whole_message = true;
            }
            "Contains" | "Contain" if on_message && matcher.is_none() => {
                matcher = first.map(|a| matcher_value(a, src));
            }
            // A constraint on the message whose reach is not compared with another's.
            other if on_message && matcher.is_none() && link.args.is_some() => {
                matcher = first.map(|a| format!("{OPAQUE}{other}({})", text(a, src)));
            }
            _ => {}
        }
    }
    attribute(
        tests,
        ExpectedException {
            line,
            skeleton,
            kind: kind.to_string(),
            exception_type,
            matcher,
            whole_message,
            ..Default::default()
        },
    );
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
fn inspect_csharp_fluent<'t>(
    call: Node<'t>,
    anc: &Ancestry<'t>,
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
    loop {
        // `(await act.Should().ThrowAsync<T>()).WithMessage(..)`: the chain goes on
        // around the awaited call.
        while let Some(wrapper) = anc
            .parent(link)
            .filter(|p| matches!(p.kind(), "await_expression" | "parenthesized_expression"))
        {
            link = wrapper;
        }
        let Some(access) = anc
            .parent(link)
            .filter(|p| p.kind() == "member_access_expression")
        else {
            break;
        };
        let Some(outer) = anc
            .parent(access)
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
            ..Default::default()
        },
    );
}

/// PHP: the class of `$this->expectException(Foo::class)`, the message, pattern or code
/// of `expectExceptionMessage(..)` / `expectExceptionMessageMatches(..)` /
/// `expectExceptionCode(..)`, and `expectExceptionObject(new Foo("message", code))`, which
/// PHPUnit reads as the three calls it stands for: its class, and its message and code
/// when the constructor is given them.
pub(super) fn php_expectations(
    call_name: &str,
    first_arg: Option<Node>,
    src: &str,
    line: usize,
) -> Vec<ExpectedException> {
    let expectation =
        |kind: &str, exception_type: Option<String>, matcher: Option<String>| ExpectedException {
            line,
            skeleton: format!("$this->{kind}#"),
            kind: kind.to_string(),
            exception_type,
            matcher,
            ..Default::default()
        };
    let Some(arg) = first_arg else {
        return vec![expectation(call_name, None, None)];
    };
    match call_name {
        "expectException" => {
            // `Foo::class` names the class by its scope; a variable is kept as written.
            let class = if arg.kind() == "class_constant_access_expression" {
                arg.named_child(0).unwrap_or(arg)
            } else {
                arg
            };
            vec![expectation(
                call_name,
                Some(text(class, src).to_string()),
                None,
            )]
        }
        "expectExceptionObject" if arg.kind() == "object_creation_expression" => {
            let mut cursor = arg.walk();
            let parts: Vec<Node> = arg.named_children(&mut cursor).collect();
            let class = parts.iter().find(|p| p.kind() != "arguments");
            let values: Vec<Node> = parts
                .iter()
                .find(|p| p.kind() == "arguments")
                .map(|a| {
                    let mut c = a.walk();
                    a.named_children(&mut c)
                        .map(|v| {
                            if v.kind() == "argument" {
                                v.named_child(0).unwrap_or(v)
                            } else {
                                v
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            let mut out = vec![expectation(
                "expectException",
                class.map(|c| text(*c, src).to_string()),
                None,
            )];
            if let Some(message) = values.first() {
                out.push(expectation(
                    "expectExceptionMessage",
                    None,
                    Some(matcher_value(*message, src)),
                ));
            }
            if let Some(code) = values.get(1) {
                out.push(expectation(
                    "expectExceptionCode",
                    None,
                    Some(matcher_value(*code, src)),
                ));
            }
            out
        }
        // An object held in a variable: something is expected, and what is not read.
        "expectExceptionObject" => vec![expectation(
            "expectException",
            None,
            Some(format!("{OPAQUE}{}", text(arg, src))),
        )],
        _ => vec![expectation(call_name, None, Some(matcher_value(arg, src)))],
    }
}

/// PHP: gives the expectations of `tests` the class declarations of their file.
pub(super) fn php_declared(root: Node, src: &str, tests: &mut [TestFn]) {
    attach_declared(root, src, tests, &PHP_CLASSES);
}

/// Whether a `binary_expression` is written with `operator`.
fn kotlin_operator(node: Node, operator: &str, src: &str) -> bool {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .any(|c| !c.is_named() && text(c, src) == operator);
    found
}

/// The name a Kotlin callee ends in: `assertThrows`, or the last part of
/// `Assertions.assertThrows`.
fn kotlin_callee_name<'t>(callee: Node<'t>, src: &'t str) -> Option<&'t str> {
    match callee.kind() {
        "identifier" => Some(text(callee, src)),
        "navigation_expression" => {
            let mut cursor = callee.walk();
            let last = callee.named_children(&mut cursor).last()?;
            (last.kind() == "identifier").then(|| text(last, src))
        }
        _ => None,
    }
}

/// The expectation kind a Kotlin function name stands for, and whether it names a class.
fn kotlin_form(name: &str) -> Option<(&'static str, bool)> {
    Some(match name {
        "assertThrows" | "assertFailsWith" | "shouldThrow" | "shouldThrowUnit" => {
            ("assertThrows", true)
        }
        "assertThrowsExactly" | "shouldThrowExactly" | "shouldThrowExactlyUnit" => {
            ("assertThrowsExactly", true)
        }
        "assertFails" | "shouldThrowAny" => ("assertThrows", false),
        // Any failure fails these, with a class named or not: the class is not read.
        "assertDoesNotThrow" | "shouldNotThrowAny" | "shouldNotThrow" => ("doesNotThrow", false),
        _ => return None,
    })
}

/// Kotlin: JUnit `assertThrows<T> { }`, kotlin.test `assertFailsWith<T> { }` /
/// `assertFailsWith(T::class) { }` / `assertFails { }`, Kotest `shouldThrow<T> { }` /
/// `shouldThrowExactly<T> { }` / `shouldThrowAny { }`, and the forms that state nothing
/// is thrown. They are kept under Java's kinds, and name the same classes.
pub fn kotlin<'t>(root: Node<'t>, anc: &Ancestry<'t>, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    let messages = KotlinMessages::new(root);
    walk(root, &mut |node| {
        let (name, class, lambda) = match node.kind() {
            // `name<T> { .. }` with no parentheses is read by the grammar as two
            // comparisons, `(name < T) > { .. }`.
            "binary_expression" => {
                let (Some(left), Some(lambda)) = (
                    node.child_by_field_name("left"),
                    node.child_by_field_name("right"),
                ) else {
                    return true;
                };
                if lambda.kind() != "lambda_literal"
                    || left.kind() != "binary_expression"
                    || !kotlin_operator(node, ">", src)
                    || !kotlin_operator(left, "<", src)
                {
                    return true;
                }
                let (Some(callee), Some(class)) = (
                    left.child_by_field_name("left"),
                    left.child_by_field_name("right"),
                ) else {
                    return true;
                };
                (kotlin_callee_name(callee, src), Some(class), lambda)
            }
            // `name(T::class) { .. }`, `name<T>("message") { .. }`, `name { .. }`.
            "call_expression" => {
                let Some(lambda) = child_of_kind(node, &["annotated_lambda"]) else {
                    return true;
                };
                let Some(callee) = node.named_child(0) else {
                    return true;
                };
                if callee.kind() != "call_expression" {
                    // `name<T> { .. }.message`: before a `.`, the grammar reads the type
                    // argument as one.
                    let class =
                        child_of_kind(node, &["type_arguments"]).and_then(|t| t.named_child(0));
                    (kotlin_callee_name(callee, src), class, lambda)
                } else {
                    let class = child_of_kind(callee, &["type_arguments"])
                        .and_then(|t| t.named_child(0))
                        .or_else(|| {
                            // `T::class`: the class before the `::`.
                            let value = child_of_kind(callee, &["value_arguments"])?
                                .named_child(0)?
                                .named_child(0)?;
                            (value.kind() == "navigation_expression"
                                && kotlin_operator(value, "::", src))
                            .then(|| value.named_child(0))
                            .flatten()
                        });
                    let name = callee
                        .named_child(0)
                        .and_then(|c| kotlin_callee_name(c, src));
                    (name, class, lambda)
                }
            }
            _ => return true,
        };
        let Some((kind, names_class)) = name.and_then(kotlin_form) else {
            return true;
        };
        let message = (kind != "doesNotThrow")
            .then(|| kotlin_message(node, anc, src, &messages))
            .flatten();
        attribute(
            tests,
            ExpectedException {
                line: node.start_position().row + 1,
                skeleton: collapse_ws(&format!("assertThrows#{}", text(lambda, src))),
                kind: kind.to_string(),
                exception_type: class
                    .filter(|_| names_class)
                    .map(|c| text(c, src).to_string()),
                whole_message: message.as_ref().is_some_and(|(_, whole)| *whole),
                matcher: message.map(|(matcher, _)| matcher),
                ..Default::default()
            },
        );
        true
    });
    attach_declared(root, src, tests, &KOTLIN_CLASSES);
}

/// One assertion a Kotlin source makes on a message: `<subject> <matcher> <value>`, or
/// `<subject>.<matcher>(<value>)`.
type KotlinMatch<'t> = (Node<'t>, Node<'t>, Node<'t>);

/// The node or the name a message assertion is made on: the expectation itself, by its
/// id, or the name it is bound to.
#[derive(PartialEq, Eq, Hash)]
enum KotlinValue<'s> {
    Node(usize),
    Name(&'s str),
}

/// The message assertions of one Kotlin tree, in the order of the source, for
/// [`kotlin_message`].
///
/// The assertion on an expectation's message is looked for in the function that holds
/// the expectation. Walking that function for each expectation costs the function for
/// each: 800 `assertThrows` in one test cost 7.6e10 instructions. The assertions are
/// listed once, with what each is made on, and an expectation asks for the first one
/// made on it inside its function.
struct KotlinMessages<'t, 's> {
    root: Node<'t>,
    listed: std::cell::OnceCell<KotlinListed<'t, 's>>,
}

struct KotlinListed<'t, 's> {
    /// Every node read as `subject matcher value`, in the order of the source.
    matches: Vec<KotlinMatch<'t>>,
    /// For what an assertion is made on, the places in `matches` of the assertions
    /// that are an answer for it, in order.
    made_on: HashMap<KotlinValue<'s>, Vec<usize>>,
    /// For the root and each function, by its id, its run of `matches`.
    runs: HashMap<usize, (usize, usize)>,
}

/// `subject matcher value` of an infix expression or of a call of a matcher.
fn kotlin_match(n: Node) -> Option<KotlinMatch> {
    let (subject, matcher, value) = match n.kind() {
        "infix_expression" if n.named_child_count() == 3 => {
            (n.named_child(0), n.named_child(1), n.named_child(2))
        }
        // `<subject>.matcher(value)`
        "call_expression" => {
            let callee = n
                .named_child(0)
                .filter(|c| c.kind() == "navigation_expression" && c.named_child_count() == 2);
            let value = child_of_kind(n, &["value_arguments"])
                .and_then(|a| a.named_child(0))
                .and_then(|a| a.named_child(0));
            (
                callee.and_then(|c| c.named_child(0)),
                callee.and_then(|c| c.named_child(1)),
                value,
            )
        }
        _ => return None,
    };
    Some((subject?, matcher?, value?))
}

impl<'t, 's> KotlinMessages<'t, 's> {
    fn new(root: Node<'t>) -> Self {
        Self {
            root,
            listed: std::cell::OnceCell::new(),
        }
    }

    fn listed(&self, src: &'s str) -> &KotlinListed<'t, 's> {
        self.listed.get_or_init(|| {
            let mut matches: Vec<KotlinMatch> = Vec::new();
            let mut made_on: HashMap<KotlinValue, Vec<usize>> = HashMap::new();
            let mut runs = HashMap::new();
            let value_of = |n: Node<'t>| {
                let name = (n.kind() == "identifier").then(|| KotlinValue::Name(text(n, src)));
                std::iter::once(KotlinValue::Node(n.id())).chain(name)
            };
            // A node, and for one already entered the place its run began at.
            let mut stack: Vec<(Node, Option<usize>)> = vec![(self.root, None)];
            while let Some((n, begun)) = stack.pop() {
                super::ancestry::count(1);
                if let Some(from) = begun {
                    runs.insert(n.id(), (from, matches.len()));
                    continue;
                }
                if n.kind() == "function_declaration" || n.id() == self.root.id() {
                    stack.push((n, Some(matches.len())));
                }
                if let Some((subject, matcher, value)) = kotlin_match(n) {
                    let matcher_name = text(matcher, src);
                    // `<value> shouldHaveMessage ..`
                    if matcher_name == "shouldHaveMessage" {
                        for on in value_of(subject) {
                            made_on.entry(on).or_default().push(matches.len());
                        }
                    }
                    // `<value>.message should.. ..`
                    let of_message = subject.kind() == "navigation_expression"
                        && subject.named_child_count() == 2
                        && subject
                            .named_child(1)
                            .is_some_and(|m| text(m, src) == "message");
                    if let Some(held) = subject
                        .named_child(0)
                        .filter(|_| of_message && matcher_name.starts_with("should"))
                    {
                        for on in value_of(held) {
                            made_on.entry(on).or_default().push(matches.len());
                        }
                    }
                    matches.push((subject, matcher, value));
                }
                let mut cursor = n.walk();
                let children: Vec<Node> = n.children(&mut cursor).collect();
                stack.extend(children.into_iter().rev().map(|c| (c, None)));
            }
            KotlinListed {
                matches,
                made_on,
                runs,
            }
        })
    }
}

/// The Kotest assertion made on the message of the exception the expectation `site`
/// returns, and whether it is on the whole message: `e.message shouldBe ".."` and
/// `e shouldHaveMessage ".."` (whole), `e.message shouldContain ".."` (a part), any other
/// `should..` matcher on the message (kept as written), each also as a call
/// (`e.message.shouldBe("..")`). `e` is the name `site` is bound to in its function
/// (`val e = shouldThrow<T> { }`), or `site` itself (`shouldThrow<T> { }.message ..`).
/// The first such assertion in the function that holds `site` (the file, outside any).
fn kotlin_message<'t, 's>(
    site: Node<'t>,
    anc: &Ancestry<'t>,
    src: &'s str,
    messages: &KotlinMessages<'t, 's>,
) -> Option<(String, bool)> {
    let bound = anc
        .parent(site)
        .filter(|p| p.kind() == "property_declaration")
        .and_then(|p| child_of_kind(p, &["variable_declaration"]))
        .and_then(|v| v.named_child(0))
        .map(|name| text(name, src));
    let mut scope = site;
    while let Some(parent) = anc.parent(scope) {
        scope = parent;
        if scope.kind() == "function_declaration" {
            break;
        }
    }
    let is_value =
        |n: Node| n.id() == site.id() || (n.kind() == "identifier" && Some(text(n, src)) == bound);
    // `<value>.message`
    let is_message = |n: Node| {
        n.kind() == "navigation_expression"
            && n.named_child_count() == 2
            && n.named_child(0).is_some_and(is_value)
            && n.named_child(1).is_some_and(|m| text(m, src) == "message")
    };
    let answer = |(subject, matcher, value): KotlinMatch| {
        let matcher = text(matcher, src);
        match matcher {
            "shouldHaveMessage" if is_value(subject) => Some((matcher_value(value, src), true)),
            "shouldBe" if is_message(subject) => Some((matcher_value(value, src), true)),
            "shouldContain" if is_message(subject) => Some((matcher_value(value, src), false)),
            _ if is_message(subject) && matcher.starts_with("should") => {
                Some((format!("{OPAQUE}{matcher}({})", text(value, src)), false))
            }
            _ => None,
        }
    };
    let listed = messages.listed(src);
    let Some((from, to)) = listed.runs.get(&scope.id()).copied() else {
        // A scope the list has no run for: read as before the list was kept.
        let mut found = None;
        walk(scope, &mut |n| {
            if found.is_none() {
                found = kotlin_match(n).and_then(answer);
            }
            found.is_none()
        });
        return found;
    };
    // The first assertion in the function made on the expectation or on its name. One
    // listed under either is an answer; it is read again here as it always was.
    let first = |on: KotlinValue| {
        super::ancestry::count(1);
        let places = listed.made_on.get(&on)?;
        places
            .get(places.partition_point(|at| *at < from))
            .copied()
            .filter(|at| *at < to)
    };
    let by_node = first(KotlinValue::Node(site.id()));
    let by_name = bound.and_then(|name| first(KotlinValue::Name(name)));
    let mut places: Vec<usize> = by_node.into_iter().chain(by_name).collect();
    places.sort_unstable();
    places.into_iter().find_map(|at| answer(listed.matches[at]))
}

/// `class A(..) : B(..), I`: the class each delegation specifier names.
fn kotlin_parents(class: Node, src: &str) -> Vec<String> {
    let Some(specifiers) = child_of_kind(class, &["delegation_specifiers"]) else {
        return Vec::new();
    };
    let mut cursor = specifiers.walk();
    specifiers
        .named_children(&mut cursor)
        .filter_map(|specifier| {
            let named = specifier.named_child(0)?;
            let class = if named.kind() == "constructor_invocation" {
                named.named_child(0)?
            } else {
                named
            };
            (class.kind() == "user_type").then(|| text(class, src).to_string())
        })
        .collect()
}

const KOTLIN_CLASSES: ClassSyntax = ClassSyntax {
    declarations: &["class_declaration"],
    parents: kotlin_parents,
};

/// What a `raise_error(..)` matcher, or the arguments of `with_message(..)`, say about
/// the failure: a constant is a class, a string is the whole message, a pattern is
/// searched for.
fn ruby_constraint(args: Option<Node>, src: &str, out: &mut ExpectedException) {
    let Some(args) = args else {
        return;
    };
    let mut cursor = args.walk();
    for arg in args.named_children(&mut cursor) {
        match arg.kind() {
            "comment" => {}
            "constant" | "scope_resolution" => {
                if out.exception_type.is_none() {
                    out.exception_type = Some(text(arg, src).to_string());
                }
            }
            _ => {
                if out.matcher.is_none() {
                    out.whole_message = string_literal(arg, src).is_some();
                    out.matcher = Some(matcher_value(arg, src));
                }
            }
        }
    }
}

/// Ruby: RSpec `expect { }.to raise_error(Class, message)` (also `raise_exception`,
/// `.with_message(..)`, and `.not_to` / `.to_not`), and Minitest / test-unit
/// `assert_raises(Class, ..) { }`.
pub fn ruby(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    walk(root, &mut |node| {
        if node.kind() != "call" {
            return true;
        }
        let method = node
            .child_by_field_name("method")
            .map_or("", |m| text(m, src));
        let receiver = node.child_by_field_name("receiver");
        let args = node.child_by_field_name("arguments");
        match (method, receiver) {
            ("to" | "not_to" | "to_not", Some(subject)) => {
                // The subject is `expect { .. }`: a call to `expect` with a block.
                let is_expect = subject.kind() == "call"
                    && subject
                        .child_by_field_name("method")
                        .is_some_and(|m| text(m, src) == "expect");
                let Some(mut matcher) = args.filter(|_| is_expect).and_then(|a| a.named_child(0))
                else {
                    return true;
                };
                let mut expected = ExpectedException::default();
                // `raise_error(A).with_message("m")`: the matcher is the receiver.
                while matcher.kind() == "call"
                    && matcher
                        .child_by_field_name("method")
                        .is_some_and(|m| text(m, src) == "with_message")
                {
                    ruby_constraint(matcher.child_by_field_name("arguments"), src, &mut expected);
                    let Some(inner) = matcher.child_by_field_name("receiver") else {
                        return true;
                    };
                    matcher = inner;
                }
                let name = match matcher.kind() {
                    "identifier" => text(matcher, src),
                    "call" if matcher.child_by_field_name("receiver").is_none() => matcher
                        .child_by_field_name("method")
                        .map_or("", |m| text(m, src)),
                    _ => "",
                };
                if !matches!(name, "raise_error" | "raise_exception") {
                    return true;
                }
                // The matcher's own message comes before one given to `with_message`.
                let mut own = ExpectedException::default();
                ruby_constraint(matcher.child_by_field_name("arguments"), src, &mut own);
                if own.matcher.is_none() {
                    own.matcher = expected.matcher;
                    own.whole_message = expected.whole_message;
                }
                let block = subject
                    .child_by_field_name("block")
                    .map_or("", |b| text(b, src));
                attribute(
                    tests,
                    ExpectedException {
                        line: node.start_position().row + 1,
                        skeleton: collapse_ws(&format!("expect {block}.to raise_error#")),
                        kind: if method == "to" {
                            "raise_error"
                        } else {
                            "not.raise_error"
                        }
                        .to_string(),
                        ..own
                    },
                );
            }
            ("assert_raises" | "assert_raise", None) => {
                // Every constant is an accepted class; a string is the assertion's message.
                let classes: Vec<&str> = args
                    .map(|a| {
                        let mut cursor = a.walk();
                        a.named_children(&mut cursor)
                            .filter(|c| matches!(c.kind(), "constant" | "scope_resolution"))
                            .map(|c| text(c, src))
                            .collect()
                    })
                    .unwrap_or_default();
                let block = node
                    .child_by_field_name("block")
                    .map_or("", |b| text(b, src));
                attribute(
                    tests,
                    ExpectedException {
                        line: node.start_position().row + 1,
                        skeleton: collapse_ws(&format!("assert_raises# {block}")),
                        kind: "assert_raises".to_string(),
                        exception_type: (!classes.is_empty()).then(|| classes.join(", ")),
                        ..Default::default()
                    },
                );
            }
            _ => {}
        }
        true
    });
    attach_declared(root, src, tests, &RUBY_CLASSES);
}

/// `class A < B`.
fn ruby_parents(class: Node, src: &str) -> Vec<String> {
    class
        .child_by_field_name("superclass")
        .map(|s| named_children_text(s, src))
        .unwrap_or_default()
}

const RUBY_CLASSES: ClassSyntax = ClassSyntax {
    declarations: &["class"],
    parents: ruby_parents,
};

/// What a Catch2 or doctest macro expects, from its name: the kind, the position of the
/// class argument and the position of the message argument. `REQUIRE_THROWS(expr)`,
/// `REQUIRE_THROWS_AS(expr, Type)`, `REQUIRE_THROWS_WITH(expr, message)`, Catch2
/// `REQUIRE_THROWS_MATCHES(expr, Type, matcher)`, doctest
/// `REQUIRE_THROWS_WITH_AS(expr, message, Type)`, and `REQUIRE_NOTHROW(expr)`; the same
/// under `CHECK_`, and with doctest's `_MESSAGE` suffix, whose further argument is the
/// assertion's own message.
fn catch_form(name: &str) -> Option<(&'static str, Option<usize>, Option<usize>)> {
    let name = name
        .strip_prefix("REQUIRE_")
        .or_else(|| name.strip_prefix("CHECK_"))?;
    Some(match name.strip_suffix("_MESSAGE").unwrap_or(name) {
        "THROWS" => ("EXPECT_THROW", None, None),
        "THROWS_AS" => ("EXPECT_THROW", Some(1), None),
        "THROWS_WITH" => ("EXPECT_THROW", None, Some(1)),
        "THROWS_MATCHES" => ("EXPECT_THROW", Some(1), Some(2)),
        "THROWS_WITH_AS" => ("EXPECT_THROW", Some(2), Some(1)),
        "NOTHROW" => ("EXPECT_NO_THROW", None, None),
        _ => return None,
    })
}

/// C++ googletest: `EXPECT_THROW(statement, Type)` / `ASSERT_THROW`, `EXPECT_ANY_THROW` /
/// `ASSERT_ANY_THROW` (any exception), and `EXPECT_NO_THROW` / `ASSERT_NO_THROW`, which
/// state that nothing is thrown. Catch2 and doctest: the macros of [`catch_form`], where
/// a string is the whole message and any other matcher is kept as written.
pub fn cpp(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    walk(root, &mut |node| {
        if node.kind() != "call_expression" {
            return true;
        }
        let Some(callee) = node
            .child_by_field_name("function")
            .filter(|f| f.kind() == "identifier")
        else {
            return true;
        };
        let (kind, class_at, message_at) = match text(callee, src) {
            "EXPECT_THROW" | "ASSERT_THROW" => ("EXPECT_THROW", Some(1), None),
            "EXPECT_ANY_THROW" | "ASSERT_ANY_THROW" => ("EXPECT_THROW", None, None),
            "EXPECT_NO_THROW" | "ASSERT_NO_THROW" => ("EXPECT_NO_THROW", None, None),
            other => match catch_form(other) {
                Some(form) => form,
                None => return true,
            },
        };
        let Some(args) = node.child_by_field_name("arguments") else {
            return true;
        };
        let mut cursor = args.walk();
        let named: Vec<Node> = args
            .named_children(&mut cursor)
            .filter(|a| a.kind() != "comment")
            .collect();
        let statement = named.first().map_or("", |s| text(*s, src));
        let message = message_at.and_then(|at| named.get(at));
        attribute(
            tests,
            ExpectedException {
                line: node.start_position().row + 1,
                skeleton: collapse_ws(&format!("EXPECT_THROW({statement}, #)")),
                kind: kind.to_string(),
                exception_type: class_at
                    .and_then(|at| named.get(at))
                    .map(|t| text(*t, src).to_string()),
                matcher: message.map(|m| matcher_value(*m, src)),
                whole_message: message.is_some_and(|m| string_literal(*m, src).is_some()),
                ..Default::default()
            },
        );
        true
    });
    attach_declared(root, src, tests, &CPP_CLASSES);
}

/// `class A : public B, C`: the classes of the base clause, without their access.
fn cpp_parents(class: Node, src: &str) -> Vec<String> {
    let Some(bases) = child_of_kind(class, &["base_class_clause"]) else {
        return Vec::new();
    };
    let mut cursor = bases.walk();
    bases
        .named_children(&mut cursor)
        .filter(|c| {
            matches!(
                c.kind(),
                "type_identifier" | "qualified_identifier" | "template_type"
            )
        })
        .map(|c| text(c, src).to_string())
        .collect()
}

const CPP_CLASSES: ClassSyntax = ClassSyntax {
    declarations: &["class_specifier", "struct_specifier"],
    parents: cpp_parents,
};

/// The package each name of a Go file is bound to by an `import`: `(local name, path)`.
/// A dot import (`import . "gopkg.in/check.v1"`) is bound to `.`.
#[derive(Debug, Default)]
pub(super) struct GoImports(Vec<(String, String)>);

/// The name a Go import path is bound to when the import gives none: its last segment,
/// without a major-version segment (`.../v2`) or suffix (`check.v1`).
fn go_default_name(path: &str) -> &str {
    let is_version =
        |s: &str| s.len() > 1 && s.starts_with('v') && s[1..].chars().all(|c| c.is_ascii_digit());
    let mut segments = path.rsplit('/');
    let mut last = segments.next().unwrap_or(path);
    if is_version(last) {
        last = segments.next().unwrap_or(last);
    }
    match last.rsplit_once('.') {
        Some((name, version)) if is_version(version) => name,
        _ => last,
    }
}

pub(super) fn go_imports(root: Node, src: &str) -> GoImports {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if node.kind() != "import_spec" {
            return true;
        }
        let Some(path) = node.child_by_field_name("path") else {
            return false;
        };
        let path = text(path, src).trim_matches(['"', '`']);
        let name = match node.child_by_field_name("name") {
            Some(name) if name.kind() == "dot" => ".",
            Some(name) => text(name, src),
            None => go_default_name(path),
        };
        out.push((name.to_string(), path.to_string()));
        false
    });
    GoImports(out)
}

impl GoImports {
    fn binds(&self, name: &str, is: fn(&str) -> bool) -> bool {
        self.0.iter().any(|(n, path)| n == name && is(path))
    }

    /// Whether `name` is testify's `assert` or `require` package in this file.
    pub(super) fn testify(&self, name: &str) -> bool {
        self.binds(name, |p| {
            p.ends_with("/testify/assert") || p.ends_with("/testify/require")
        })
    }

    /// Whether `name` is a package whose `Is` and `As` walk an error chain.
    fn errors(&self, name: &str) -> bool {
        self.binds(name, |p| {
            matches!(
                p,
                "errors" | "github.com/pkg/errors" | "golang.org/x/xerrors"
            )
        })
    }

    /// Whether `checker` (`Panics`, `check.Panics`) names a checker of gocheck: written
    /// bare under a dot import of the package, or under the name the file imports it by.
    fn gocheck_checker<'s>(&self, checker: Node, src: &'s str) -> Option<&'s str> {
        let is = |p: &str| {
            matches!(
                p,
                "gopkg.in/check.v1" | "github.com/go-check/check" | "launchpad.net/gocheck"
            )
        };
        match checker.kind() {
            "identifier" => self.binds(".", is).then(|| text(checker, src)),
            "selector_expression" => {
                let on = checker.child_by_field_name("operand")?;
                let name = checker.child_by_field_name("field")?;
                (on.kind() == "identifier" && self.binds(text(on, src), is))
                    .then(|| text(name, src))
            }
            _ => None,
        }
    }
}

/// A Go matcher argument: the content of a string literal, or behind [`OPAQUE`] the
/// text of any other expression.
fn go_matcher(node: Node, src: &str) -> String {
    let raw = text(node, src);
    match node.kind() {
        "interpreted_string_literal" => raw
            .strip_prefix('"')
            .and_then(|r| r.strip_suffix('"'))
            .unwrap_or(raw)
            .to_string(),
        "raw_string_literal" => raw.trim_matches('`').to_string(),
        _ => format!("{OPAQUE}{raw}"),
    }
}

/// A gocheck pattern (`ErrorMatches`, `PanicMatches`), which must match the whole
/// message: one with no metacharacter is that message; any other is kept as a regular
/// expression, which is told apart from another and from one that matches everything.
fn go_pattern(node: Node, src: &str, out: &mut ExpectedException) {
    let m = go_matcher(node, src);
    if m.starts_with(OPAQUE) {
        out.matcher = Some(m);
    } else if is_literal_pattern(&m) {
        out.matcher = Some(m);
        out.whole_message = true;
    } else {
        out.matcher = Some(format!("{OPAQUE}/{m}/"));
    }
}

/// What `errors.As(err, target)` names: the type `target` is declared with in the
/// enclosing function (`var pe *fs.PathError`, then `&pe`), else the target as written.
fn go_as_target<'t>(target: Node<'t>, anc: &Ancestry<'t>, src: &str) -> String {
    let written = text(target, src).to_string();
    let Some(name) = (target.kind() == "unary_expression")
        .then(|| target.child_by_field_name("operand"))
        .flatten()
        .filter(|n| n.kind() == "identifier")
    else {
        return written;
    };
    let mut function = target;
    while let Some(parent) = anc.parent(function) {
        function = parent;
        if matches!(
            function.kind(),
            "function_declaration" | "method_declaration"
        ) {
            break;
        }
    }
    let mut declared = None;
    walk(function, &mut |n| {
        if n.kind() == "var_spec" && declared.is_none() {
            let mut cursor = n.walk();
            let named = n
                .children_by_field_name("name", &mut cursor)
                .any(|c| text(c, src) == text(name, src));
            if named {
                declared = n.child_by_field_name("type").map(|t| text(t, src));
            }
        }
        declared.is_none()
    });
    declared.map_or(written, |t| t.trim_start_matches('*').to_string())
}

/// `errors.Is(err, target)` / `errors.As(err, &target)` of a package the file imports
/// as an errors package: the error and the sentinel or type it must be.
fn go_errors_call<'t>(
    call: Node<'t>,
    anc: &Ancestry<'t>,
    src: &str,
    imports: &GoImports,
) -> Option<(String, String)> {
    if call.kind() != "call_expression" {
        return None;
    }
    let callee = call.child_by_field_name("function")?;
    if callee.kind() != "selector_expression" {
        return None;
    }
    let on = callee.child_by_field_name("operand")?;
    if on.kind() != "identifier" || !imports.errors(text(on, src)) {
        return None;
    }
    let args = call.child_by_field_name("arguments")?;
    let mut cursor = args.walk();
    let named: Vec<Node> = args
        .named_children(&mut cursor)
        .filter(|a| a.kind() != "comment")
        .collect();
    let (err, target) = (named.first()?, named.get(1)?);
    let class = match text(callee.child_by_field_name("field")?, src) {
        "Is" => text(*target, src).to_string(),
        "As" => go_as_target(*target, anc, src),
        _ => return None,
    };
    Some((text(*err, src).to_string(), class))
}

fn go_error_expectation(call: Node, err: &str, kind: &str) -> ExpectedException {
    ExpectedException {
        line: call.start_position().row + 1,
        skeleton: collapse_ws(&format!("error#({err})")),
        kind: kind.to_string(),
        ..Default::default()
    }
}

/// The expected failure a testify assertion states. `values` are its arguments after
/// `t`: `ErrorIs` / `ErrorAs` (the sentinel or type), `ErrorContains` (a part of the
/// message), `EqualError` (the whole message), `Error`, `NoError`, `True(errors.Is(..))`,
/// `Panics`, `PanicsWithValue` / `PanicsWithError` (the value or the whole message) and
/// `NotPanics`, each also with a trailing `f`.
pub(super) fn go_testify<'t>(
    method: &str,
    values: &[Node<'t>],
    call: Node<'t>,
    anc: &Ancestry<'t>,
    src: &str,
    imports: &GoImports,
) -> Option<ExpectedException> {
    const FORMS: &[&str] = &[
        "Error",
        "NoError",
        "ErrorIs",
        "ErrorAs",
        "ErrorContains",
        "EqualError",
        "True",
        "Panics",
        "PanicsWithValue",
        "PanicsWithError",
        "NotPanics",
    ];
    let method = match method.strip_suffix('f') {
        Some(plain) if FORMS.contains(&plain) => plain,
        _ => method,
    };
    let first = values.first()?;
    let error = |kind: &str| go_error_expectation(call, text(*first, src), kind);
    let panics = |code: Node, kind: &str| ExpectedException {
        line: call.start_position().row + 1,
        skeleton: collapse_ws(&format!("panics#({})", text(code, src))),
        kind: kind.to_string(),
        ..Default::default()
    };
    Some(match method {
        "Error" => error("go.Error"),
        "NoError" => error("go.NoError"),
        "ErrorIs" => ExpectedException {
            exception_type: Some(text(*values.get(1)?, src).to_string()),
            ..error("go.Error")
        },
        "ErrorAs" => ExpectedException {
            exception_type: Some(go_as_target(*values.get(1)?, anc, src)),
            ..error("go.Error")
        },
        "ErrorContains" | "EqualError" => ExpectedException {
            matcher: Some(go_matcher(*values.get(1)?, src)),
            whole_message: method == "EqualError",
            ..error("go.Error")
        },
        "True" => {
            let (err, class) = go_errors_call(*first, anc, src, imports)?;
            ExpectedException {
                exception_type: Some(class),
                ..go_error_expectation(call, &err, "go.Error")
            }
        }
        "Panics" => panics(*first, "go.Panics"),
        "NotPanics" => panics(*first, "go.NotPanics"),
        "PanicsWithValue" | "PanicsWithError" => ExpectedException {
            matcher: Some(go_matcher(*first, src)),
            whole_message: true,
            ..panics(*values.get(1)?, "go.Panics")
        },
        _ => return None,
    })
}

/// The expected failure a gocheck assertion states, from the arguments of
/// `c.Assert(obtained, Checker, expected)`: `ErrorMatches` (a pattern for the whole
/// message of the error), `Panics` (the panic value) and `PanicMatches` (a pattern for
/// the whole panic message).
pub(super) fn go_gocheck(
    args: &[Node],
    call: Node,
    src: &str,
    imports: &GoImports,
) -> Option<ExpectedException> {
    let (obtained, checker) = (args.first()?, args.get(1)?);
    let checker = imports.gocheck_checker(*checker, src)?;
    let mut out = match checker {
        "ErrorMatches" => go_error_expectation(call, text(*obtained, src), "go.Error"),
        "Panics" | "PanicMatches" => ExpectedException {
            line: call.start_position().row + 1,
            skeleton: collapse_ws(&format!("panics#({})", text(*obtained, src))),
            kind: "go.Panics".to_string(),
            ..Default::default()
        },
        _ => return None,
    };
    if let Some(expected) = args.get(2) {
        if checker == "Panics" {
            out.matcher = Some(go_matcher(*expected, src));
            out.whole_message = true;
        } else {
            go_pattern(*expected, src, &mut out);
        }
    }
    Some(out)
}

/// Whether a block calls a method that fails the test (`t.Fatal`, `t.Errorf`, ..).
fn go_block_fails(block: Node, src: &str, known: &mut GoBlocks) -> bool {
    if let Some(fails) = known.get(&block.id()) {
        return *fails;
    }
    let mut fails = false;
    walk(block, &mut |n| {
        if fails || n.kind() == "func_literal" {
            return false;
        }
        // The body of an `if` inside this block is a block `go` asks about too: it is
        // read once, for itself, and its answer is this block's when it fails.
        if n.kind() == "if_statement" {
            if let Some(inner) = n.child_by_field_name("consequence") {
                if go_block_fails(inner, src, known) {
                    fails = true;
                    return false;
                }
            }
        }
        if known.contains_key(&n.id()) {
            // An `if` body already read, and it does not fail.
            return false;
        }
        if n.kind() == "call_expression" {
            let method = n
                .child_by_field_name("function")
                .filter(|f| f.kind() == "selector_expression")
                .and_then(|f| f.child_by_field_name("field"))
                .map_or("", |f| text(f, src));
            fails |= matches!(
                method,
                "Fatal" | "Fatalf" | "Error" | "Errorf" | "FailNow" | "Fail"
            );
        }
        !fails
    });
    known.insert(block.id(), fails);
    fails
}

/// Whether each block [`go_block_fails`] has read fails the test, by the id of its node.
type GoBlocks = std::collections::HashMap<usize, bool>;

/// The `errors.Is` / `errors.As` calls whose failure makes `condition` hold: `!call`
/// itself, or a side of an `||` (`err == nil || !errors.Is(err, ErrGone)`).
fn go_required_errors<'t>(
    condition: Node<'t>,
    anc: &Ancestry<'t>,
    src: &str,
    imports: &GoImports,
    out: &mut Vec<(String, String)>,
) {
    match condition.kind() {
        "parenthesized_expression" => {
            if let Some(inner) = condition.named_child(0) {
                go_required_errors(inner, anc, src, imports, out);
            }
        }
        "binary_expression" => {
            let or = condition
                .child_by_field_name("operator")
                .is_some_and(|o| text(o, src) == "||");
            if or {
                for side in ["left", "right"] {
                    if let Some(side) = condition.child_by_field_name(side) {
                        go_required_errors(side, anc, src, imports, out);
                    }
                }
            }
        }
        "unary_expression" => {
            let not = condition
                .child_by_field_name("operator")
                .is_some_and(|o| text(o, src) == "!");
            let mut call = condition.child_by_field_name("operand").filter(|_| not);
            while let Some(inner) = call.filter(|c| c.kind() == "parenthesized_expression") {
                call = inner.named_child(0);
            }
            out.extend(call.and_then(|c| go_errors_call(c, anc, src, imports)));
        }
        _ => {}
    }
}

/// Go: `errors.Is(err, target)` / `errors.As(err, &target)` in the condition that fails
/// a test (`if !errors.Is(err, ErrGone) { t.Fatal(..) }`). The testify and gocheck forms
/// are read where the pack counts them ([`go_testify`], [`go_gocheck`]), since whose
/// assertion a call is depends on the function it stands in.
pub(super) fn go<'t>(
    root: Node<'t>,
    anc: &Ancestry<'t>,
    src: &str,
    tests: &mut [TestFn],
    imports: &GoImports,
) {
    if tests.is_empty() {
        return;
    }
    // Each `if` body is read once: a body nested in another is part of it.
    let mut known = GoBlocks::new();
    walk(root, &mut |node| {
        if node.kind() != "if_statement" {
            return true;
        }
        let fails = node
            .child_by_field_name("consequence")
            .is_some_and(|c| go_block_fails(c, src, &mut known));
        let Some(condition) = node.child_by_field_name("condition").filter(|_| fails) else {
            return true;
        };
        let mut required = Vec::new();
        go_required_errors(condition, anc, src, imports, &mut required);
        for (err, class) in required {
            attribute(
                tests,
                ExpectedException {
                    exception_type: Some(class),
                    ..go_error_expectation(node, &err, "go.Error")
                },
            );
        }
        true
    });
}

/// The modules a Swift file imports (`import XCTest`, `@testable import Sut`,
/// `import struct Testing.Tag`): each identifier of each import declaration.
fn swift_imports(root: Node, src: &str) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if node.kind() != "import_declaration" {
            return true;
        }
        walk(node, &mut |n| {
            if n.kind() == "simple_identifier" {
                out.push(text(n, src).to_string());
            }
            true
        });
        false
    });
    out
}

/// The arguments of a Swift call or macro, each with its label, and its trailing closure.
fn swift_arguments<'t>(
    call: Node<'t>,
    src: &'t str,
) -> (Vec<(&'t str, Node<'t>)>, Option<Node<'t>>) {
    let mut arguments = Vec::new();
    let mut closure = None;
    let mut cursor = call.walk();
    for suffix in call
        .named_children(&mut cursor)
        .filter(|c| c.kind() == "call_suffix")
    {
        let mut inner = suffix.walk();
        for part in suffix.named_children(&mut inner) {
            match part.kind() {
                "lambda_literal" => closure = closure.or(Some(part)),
                "value_arguments" => {
                    let mut args = part.walk();
                    for arg in part.named_children(&mut args) {
                        let label = arg.child_by_field_name("name").map_or("", |l| text(l, src));
                        if let Some(value) = arg.child_by_field_name("value") {
                            arguments.push((label, value));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    (arguments, closure)
}

/// A Swift error type as it is compared: `Error`, `any Error` and `Swift.Error` are the
/// general type, written `any Error`; any other type is its name (`Client.Error` is a
/// type of the project).
fn swift_type(written: &str) -> String {
    let t = written.trim().trim_matches(['(', ')']).trim();
    match t.strip_prefix("any ").unwrap_or(t).trim() {
        "Error" | "Swift.Error" => "any Error".to_string(),
        other => other.to_string(),
    }
}

/// The type the closure of `XCTAssertThrowsError` checks the error to be: the first
/// `error as? T` / `error is T` on the closure's parameter (`$0` when it names none).
/// `error as NSError` holds for every error and names no type.
fn swift_closure_type(closure: Node, src: &str) -> Option<String> {
    let mut parameter = "$0";
    walk(closure, &mut |n| {
        if n.kind() == "lambda_parameter" {
            if let Some(name) = n.child_by_field_name("name") {
                parameter = text(name, src);
            }
            return false;
        }
        // The parameters stand before the statements.
        n.kind() != "statements"
    });
    let mut found = None;
    walk(closure, &mut |n| {
        if found.is_some() {
            return false;
        }
        let subject = match n.kind() {
            "as_expression" => n.child_by_field_name("expr"),
            "check_expression" => n.child_by_field_name("target"),
            _ => None,
        };
        if subject.is_some_and(|s| text(s, src) == parameter) {
            found = n
                .child_by_field_name("name")
                .map(|t| text(t, src))
                .filter(|t| *t != "NSError")
                .map(swift_type);
        }
        true
    });
    found
}

/// Swift: XCTest `XCTAssertThrowsError(expr) { error in .. }` (the type the closure
/// casts or checks the error to) and `XCTAssertNoThrow(expr)`, in a file that imports
/// `XCTest`; Swift Testing `#expect(throws: E.self) { }` / `#require(throws:)` (a type,
/// or an error value such as `E.negative`) and `throws: Never.self`, which states that
/// nothing is thrown, in a file that imports `Testing`.
pub fn swift<'t>(root: Node<'t>, anc: &Ancestry<'t>, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    let imports = swift_imports(root, src);
    let imported = |module: &str| imports.iter().any(|i| i == module);
    walk(root, &mut |node| {
        if !matches!(node.kind(), "call_expression" | "macro_invocation") {
            return true;
        }
        let Some(name) = node
            .named_child(0)
            .filter(|c| c.kind() == "simple_identifier")
            .map(|c| text(c, src))
        else {
            return true;
        };
        let (arguments, mut closure) = swift_arguments(node, src);
        let mut expected = ExpectedException {
            line: node.start_position().row + 1,
            kind: "swift.throws".to_string(),
            ..Default::default()
        };
        match (node.kind(), name) {
            ("call_expression", "XCTAssertThrowsError" | "XCTAssertNoThrow")
                if imported("XCTest") =>
            {
                let Some((_, code)) = arguments.first() else {
                    return true;
                };
                expected.skeleton = collapse_ws(&format!("throws({})#", text(*code, src)));
                if name == "XCTAssertNoThrow" {
                    expected.kind = "swift.noThrow".to_string();
                } else {
                    expected.exception_type = closure.and_then(|c| swift_closure_type(c, src));
                }
            }
            ("macro_invocation", "expect" | "require") if imported("Testing") => {
                let Some((_, thrown)) = arguments.iter().find(|(label, _)| *label == "throws")
                else {
                    return true;
                };
                // `let error = #expect(throws: E.self) { .. }`: the closure follows the
                // macro in the call the grammar wraps it in.
                let wrapping = anc.parent(node).filter(|p| p.kind() == "call_expression");
                closure = closure.or_else(|| wrapping.and_then(|p| swift_arguments(p, src).1));
                let code = closure
                    .or_else(|| {
                        arguments
                            .iter()
                            .find(|(label, _)| *label == "performing")
                            .map(|(_, value)| *value)
                    })
                    .map_or("", |c| text(c, src));
                expected.skeleton = collapse_ws(&format!("throws {code}#"));
                let written = text(*thrown, src);
                match written.strip_suffix(".self") {
                    Some(ty) if swift_type(ty) == "Never" => {
                        expected.kind = "swift.noThrow".to_string();
                    }
                    Some(ty) => expected.exception_type = Some(swift_type(ty)),
                    // An error value: its type is what stands before the case or the
                    // initializer's arguments, and the value is compared whole.
                    None => {
                        let ty = match thrown.kind() {
                            "navigation_expression" => thrown.child_by_field_name("target"),
                            "call_expression" => thrown.named_child(0),
                            _ => None,
                        };
                        expected.exception_type = ty.map(|t| swift_type(text(t, src)));
                        expected.matcher = Some(format!("{OPAQUE}{written}"));
                        expected.whole_message = true;
                    }
                }
            }
            _ => return true,
        }
        attribute(tests, expected);
        true
    });
}

/// The operator of a Scala `infix_expression`, and its two sides.
fn scala_infix<'t>(node: Node<'t>, src: &'t str) -> Option<(Node<'t>, &'t str, Node<'t>)> {
    if node.kind() != "infix_expression" {
        return None;
    }
    Some((
        node.child_by_field_name("left")?,
        text(node.child_by_field_name("operator")?, src),
        node.child_by_field_name("right")?,
    ))
}

/// `name[T]`: the name and the type argument of a Scala `generic_function` whose
/// function is a plain identifier.
fn scala_generic<'t>(node: Node<'t>, src: &'t str) -> Option<(&'t str, Node<'t>)> {
    if node.kind() != "generic_function" {
        return None;
    }
    let name = node
        .child_by_field_name("function")
        .filter(|f| f.kind() == "identifier")?;
    let class = node.child_by_field_name("type_arguments")?.named_child(0)?;
    Some((text(name, src), class))
}

/// A specs2 message argument (`throwA[T]("negative")`, `message = "negative"`), which
/// is a pattern searched for in the message.
fn scala_pattern(arg: Node, src: &str) -> String {
    let value = if arg.kind() == "assignment_expression" {
        arg.child_by_field_name("right").unwrap_or(arg)
    } else {
        arg
    };
    let m = matcher_value(value, src);
    if m.starts_with(OPAQUE) || is_literal_pattern(&m) {
        m
    } else {
        format!("{OPAQUE}/{m}/")
    }
}

/// Scala: ScalaTest and munit `intercept[T] { }` / `assertThrows[T] { }`, ScalaTest
/// matchers `the [T] thrownBy { } should have message ".."` (the whole message),
/// `a [T] should be thrownBy { }` / `an [T] ..`, and `noException should be thrownBy
/// { }`; specs2 `code must throwA[T]` / `throwAn[T]`, with a message pattern or an
/// exception value. They are kept under Java's kinds, and name the same classes.
pub fn scala<'t>(root: Node<'t>, anc: &Ancestry<'t>, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    walk(root, &mut |node| {
        let mut expected = ExpectedException {
            line: node.start_position().row + 1,
            kind: "assertThrows".to_string(),
            ..Default::default()
        };
        let code = match node.kind() {
            // `intercept[T] { .. }`, `assertThrows[T](..)`.
            "call_expression" => {
                let Some((name, class)) = node
                    .child_by_field_name("function")
                    .and_then(|f| scala_generic(f, src))
                else {
                    return true;
                };
                if !matches!(name, "intercept" | "assertThrows") {
                    return true;
                }
                expected.exception_type = Some(text(class, src).to_string());
                node.child_by_field_name("arguments")
            }
            "infix_expression" => {
                let Some((left, operator, right)) = scala_infix(node, src) else {
                    return true;
                };
                if operator != "thrownBy" {
                    return true;
                }
                // `a [T] should be thrownBy ..`: the subject stands before `should be`.
                let subject = match scala_infix(left, src) {
                    Some((subject, "should" | "must", be)) if text(be, src) == "be" => subject,
                    Some(_) => return true,
                    None => left,
                };
                match scala_generic(subject, src) {
                    Some(("the", class)) if subject == left => {
                        expected.exception_type = Some(text(class, src).to_string());
                        // `.. should have message "m"`.
                        let message = anc
                            .parent(node)
                            .and_then(|p| scala_infix(p, src))
                            .filter(|(l, op, have)| {
                                *l == node
                                    && matches!(*op, "should" | "must")
                                    && text(*have, src) == "have"
                            })
                            .and_then(|_| anc.parent(anc.parent(node)?))
                            .and_then(|g| scala_infix(g, src))
                            .filter(|(_, op, _)| *op == "message")
                            .map(|(_, _, value)| value);
                        if let Some(value) = message {
                            expected.matcher = Some(matcher_value(value, src));
                            expected.whole_message = true;
                        }
                    }
                    Some(("a" | "an", class)) if subject != left => {
                        expected.exception_type = Some(text(class, src).to_string());
                    }
                    None if subject != left && text(subject, src) == "noException" => {
                        expected.kind = "doesNotThrow".to_string();
                    }
                    _ => return true,
                }
                Some(right)
            }
            // `code must throwA[T]`: the grammar gives the type argument to the whole
            // `code must throwA`.
            "generic_function" => {
                let Some((code, "must" | "should", matcher)) = node
                    .child_by_field_name("function")
                    .and_then(|f| scala_infix(f, src))
                else {
                    return true;
                };
                if !matches!(text(matcher, src), "throwA" | "throwAn") {
                    return true;
                }
                expected.exception_type = node
                    .child_by_field_name("type_arguments")
                    .and_then(|t| t.named_child(0))
                    .map(|c| text(c, src).to_string());
                // `.. throwA[T]("negative")`: the call that wraps it.
                expected.matcher = anc
                    .parent(node)
                    .filter(|p| {
                        p.kind() == "call_expression"
                            && p.child_by_field_name("function") == Some(node)
                    })
                    .and_then(|p| p.child_by_field_name("arguments"))
                    .and_then(|a| a.named_child(0))
                    .map(|a| scala_pattern(a, src));
                Some(code)
            }
            _ => return true,
        };
        if node.kind() == "infix_expression" || expected.exception_type.is_some() {
            expected.skeleton = collapse_ws(&format!(
                "assertThrows#{}",
                code.map_or("", |c| text(c, src))
            ));
            attribute(tests, expected);
        }
        true
    });
    // `code must throwA(new T(".."))`: an exception value names its class.
    walk(root, &mut |node| {
        let Some((code, "must" | "should", matcher)) = scala_infix(node, src) else {
            return true;
        };
        let value = (matcher.kind() == "call_expression"
            && matcher
                .child_by_field_name("function")
                .is_some_and(|f| matches!(text(f, src), "throwA" | "throwAn")))
        .then(|| matcher.child_by_field_name("arguments")?.named_child(0))
        .flatten()
        .filter(|v| v.kind() == "instance_expression")
        .and_then(|v| v.named_child(0));
        if let Some(class) = value {
            attribute(
                tests,
                ExpectedException {
                    line: node.start_position().row + 1,
                    skeleton: collapse_ws(&format!("assertThrows#{}", text(code, src))),
                    kind: "assertThrows".to_string(),
                    exception_type: Some(text(class, src).to_string()),
                    ..Default::default()
                },
            );
        }
        true
    });
    attach_declared(root, src, tests, &SCALA_CLASSES);
}

/// `class A(..) extends B(..) with C`: every type of the `extends` clause.
fn scala_parents(class: Node, src: &str) -> Vec<String> {
    let Some(extends) = class.child_by_field_name("extend") else {
        return Vec::new();
    };
    let mut cursor = extends.walk();
    extends
        .children_by_field_name("type", &mut cursor)
        .map(|t| text(t, src).to_string())
        .collect()
}

const SCALA_CLASSES: ClassSyntax = ClassSyntax {
    declarations: &["class_definition", "object_definition"],
    parents: scala_parents,
};

/// Objective-C XCTest: `XCTAssertThrows(expr)`, `XCTAssertThrowsSpecific(expr, Class)`,
/// `XCTAssertThrowsSpecificNamed(expr, Class, name)` (the exception's name, compared
/// whole), and `XCTAssertNoThrow(expr)` / `XCTAssertNoThrowSpecific(expr, Class)` /
/// `XCTAssertNoThrowSpecificNamed(expr, Class, name)`, which state what is not thrown.
/// They are macros a header brings in, so they are read by name.
pub fn objc(root: Node, src: &str, tests: &mut [TestFn]) {
    if tests.is_empty() {
        return;
    }
    walk(root, &mut |node| {
        if node.kind() != "call_expression" {
            return true;
        }
        let Some(callee) = node
            .child_by_field_name("function")
            .filter(|f| f.kind() == "identifier")
        else {
            return true;
        };
        // The kind, and how many of the arguments after the expression are constraints.
        let (kind, constraints) = match text(callee, src) {
            "XCTAssertThrows" => ("objc.throws", 0),
            "XCTAssertThrowsSpecific" => ("objc.throws", 1),
            "XCTAssertThrowsSpecificNamed" => ("objc.throws", 2),
            "XCTAssertNoThrow" => ("objc.noThrow", 0),
            "XCTAssertNoThrowSpecific" => ("objc.noThrow", 1),
            "XCTAssertNoThrowSpecificNamed" => ("objc.noThrow", 2),
            _ => return true,
        };
        let Some(args) = node.child_by_field_name("arguments") else {
            return true;
        };
        let mut cursor = args.walk();
        let named: Vec<Node> = args
            .named_children(&mut cursor)
            .filter(|a| a.kind() != "comment")
            .collect();
        let Some(code) = named.first() else {
            return true;
        };
        let name = named.get(2).filter(|_| constraints == 2);
        attribute(
            tests,
            ExpectedException {
                line: node.start_position().row + 1,
                skeleton: collapse_ws(&format!("throws({})#", text(*code, src))),
                kind: kind.to_string(),
                exception_type: named
                    .get(1)
                    .filter(|_| constraints >= 1)
                    .map(|c| text(*c, src).to_string()),
                matcher: name.map(|n| matcher_value(*n, src)),
                whole_message: name.is_some(),
                ..Default::default()
            },
        );
        true
    });
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
            ..Default::default()
        }];
        let h = vec![ExpectedException {
            line: 5,
            skeleton: "#[should_panic]".to_string(),
            kind: "should_panic".to_string(),
            exception_type: None,
            matcher: None,
            ..Default::default()
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
            ..Default::default()
        }];
        let h = vec![ExpectedException {
            line: 10,
            skeleton: "with pytest.raises(#): f(-1)".to_string(),
            kind: "pytest.raises".to_string(),
            exception_type: Some("Exception".to_string()),
            matcher: None,
            ..Default::default()
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
            ..Default::default()
        }];
        let h = vec![ExpectedException {
            line: 12,
            skeleton: "expect(() => f()).toThrow(#)".to_string(),
            kind: "toThrow".to_string(),
            exception_type: None,
            matcher: None,
            ..Default::default()
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
            ..Default::default()
        }];
        let h = vec![ExpectedException {
            line: 20,
            skeleton: "Assert.Throws#(() => {})".to_string(),
            kind: "Assert.ThrowsAny".to_string(),
            exception_type: Some("Exception".to_string()),
            matcher: None,
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
        }];
        let h = vec![ExpectedException {
            line: 10,
            skeleton: "with pytest.raises(#): f(-1)".to_string(),
            kind: "pytest.raises".to_string(),
            exception_type: Some("ValueError".to_string()),
            matcher: Some("negative".to_string()),
            ..Default::default()
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
            ..Default::default()
        }];
        let h = vec![ExpectedException {
            line: 10,
            skeleton: "with pytest.raises(#): f(-1)".to_string(),
            kind: "pytest.raises".to_string(),
            exception_type: Some("TypeError".to_string()),
            matcher: None,
            ..Default::default()
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
            ..Default::default()
        }];
        let h = vec![ExpectedException {
            line: 15,
            skeleton: "assertThrows(#, () -> f())".to_string(),
            kind: "assertThrows".to_string(),
            exception_type: Some("Throwable".to_string()),
            matcher: None,
            ..Default::default()
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
            ..Default::default()
        }];
        let h = vec![ExpectedException {
            line: 15,
            skeleton: "assertThrows(#, () -> f())".to_string(),
            kind: "assertThrows".to_string(),
            exception_type: Some("IllegalStateException".to_string()),
            matcher: None,
            ..Default::default()
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
            ..Default::default()
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

    // ----- #594: tables and qualifiers, same-file classes, replaced sites, new forms -----

    use crate::ast::c_cpp::CppPack;
    use crate::ast::kotlin::KotlinPack;
    use crate::ast::ruby::RubyPack;

    fn moved(kind: &str, from: &str, to: &str) -> Option<String> {
        is_widened(
            &site(kind, "s", Some(from), None),
            &site(kind, "s", Some(to), None),
        )
    }

    fn py_file(src: &str) -> Vec<ExpectedException> {
        py(src)[0].expected_exceptions.clone()
    }

    fn kotlin_test(body: &str) -> Vec<ExpectedException> {
        facts(
            &KotlinPack,
            "src/test/kotlin/SutTest.kt",
            &format!("class SutTest {{\n    @Test\n    fun t() {{\n        {body}\n    }}\n}}\n"),
        )
    }

    fn rspec(body: &str) -> Vec<ExpectedException> {
        facts(
            &RubyPack,
            "spec/sut_spec.rb",
            &format!("RSpec.describe Sut do\n  it \"t\" do\n    {body}\n  end\nend\n"),
        )
    }

    fn gtest(body: &str) -> Vec<ExpectedException> {
        facts(
            &CppPack,
            "tests/sut_test.cc",
            &format!("#include <gtest/gtest.h>\nTEST(Sut, T) {{\n  {body}\n}}\n"),
        )
    }

    #[test]
    fn standard_tables_hold_the_classes_the_runtimes_added() {
        for (kind, from, to) in [
            ("pytest.raises", "PythonFinalizationError", "RuntimeError"),
            // `ExceptionGroup` has two parents.
            ("pytest.raises", "ExceptionGroup", "BaseExceptionGroup"),
            (
                "assertThrows",
                "MalformedInputException",
                "CharacterCodingException",
            ),
            ("assertThrows", "MalformedInputException", "IOException"),
            ("assertThrows", "ZipError", "VirtualMachineError"),
            ("raise_error", "KeyError", "IndexError"),
            ("raise_error", "FloatDomainError", "RangeError"),
            ("assert_raises", "NoMethodError", "NameError"),
            ("EXPECT_THROW", "std::invalid_argument", "std::logic_error"),
            ("EXPECT_THROW", "bad_array_new_length", "bad_alloc"),
            (
                "EXPECT_THROW",
                "std::filesystem::filesystem_error",
                "std::runtime_error",
            ),
            ("EXPECT_THROW", "app::Error", "std::exception"),
        ] {
            assert!(moved(kind, from, to).is_some(), "{kind}: {from} -> {to}");
            assert_eq!(moved(kind, to, from), None, "{kind}: {to} -> {from}");
        }
        for (kind, from, to) in [
            ("raise_error", "KeyError", "TypeError"),
            ("EXPECT_THROW", "std::range_error", "std::logic_error"),
            // One language's table says nothing about another's classes.
            ("EXPECT_THROW", "KeyError", "IndexError"),
        ] {
            assert_eq!(moved(kind, from, to), None, "{kind}: {from} -> {to}");
        }
    }

    #[test]
    fn a_class_under_a_foreign_qualifier_is_not_the_standard_one() {
        // Read as the standard class: bare, or under the standard library's qualifier.
        for (kind, from, to) in [
            ("pytest.raises", "TimeoutError", "OSError"),
            ("pytest.raises", "builtins.TimeoutError", "builtins.OSError"),
            (
                "assertThrows",
                "java.io.FileNotFoundException",
                "IOException",
            ),
            (
                "assertThrows",
                "kotlin.NumberFormatException",
                "IllegalArgumentException",
            ),
            (
                "Assert.ThrowsAny",
                "System.IO.FileNotFoundException",
                "IOException",
            ),
            (
                "Assert.ThrowsAny",
                "global::System.ArgumentNullException",
                "System.ArgumentException",
            ),
            (
                "expectException",
                "\\InvalidArgumentException",
                "\\LogicException",
            ),
            ("raise_error", "::KeyError", "IndexError"),
            ("EXPECT_THROW", "::std::out_of_range", "std::logic_error"),
        ] {
            assert!(moved(kind, from, to).is_some(), "{kind}: {from} -> {to}");
        }
        // A project class that shares the name: its parents are not known.
        for (kind, from, to) in [
            ("pytest.raises", "errors.TimeoutError", "OSError"),
            ("pytest.raises", "asyncio.TimeoutError", "OSError"),
            (
                "assertThrows",
                "my.pkg.FileNotFoundException",
                "IOException",
            ),
            (
                "Assert.ThrowsAny",
                "My.IO.FileNotFoundException",
                "IOException",
            ),
            (
                "expectException",
                "\\App\\InvalidArgumentException",
                "\\LogicException",
            ),
            (
                "expectException",
                "App\\InvalidArgumentException",
                "LogicException",
            ),
            ("raise_error", "Errors::KeyError", "IndexError"),
            ("EXPECT_THROW", "mylib::out_of_range", "std::logic_error"),
        ] {
            assert_eq!(moved(kind, from, to), None, "{kind}: {from} -> {to}");
        }
        // It is still the same class under either spelling, and a general name is above it.
        assert_eq!(moved("pytest.raises", "errors.NotFound", "NotFound"), None);
        assert!(moved("pytest.raises", "errors.TimeoutError", "Exception").is_some());
        // Python's alias is the standard `OSError` only where it is the standard name.
        assert_eq!(moved("pytest.raises", "IOError", "OSError"), None);
        assert_eq!(
            moved("pytest.raises", "FileNotFoundError", "mylib.IOError"),
            None
        );
        assert!(moved("pytest.raises", "FileNotFoundError", "IOError").is_some());
    }

    const PY_CLASSES: &str = "class AppError(RuntimeError):\n    pass\n\nclass OrderError(AppError, Mixin, metaclass=Meta):\n    pass\n\nclass PaymentError(AppError):\n    pass\n\nclass MissingKey(KeyError):\n    pass\n\nclass TimeoutError(Exception):\n    pass\n\n";

    fn py_raising(classes: &str, raised: &str) -> Vec<ExpectedException> {
        py_file(&format!(
            "{classes}def test_t():\n    with pytest.raises({raised}):\n        f(-1)\n"
        ))
    }

    #[test]
    fn a_class_declared_in_the_file_has_the_parents_it_names() {
        let order = py_raising(PY_CLASSES, "OrderError");
        assert_eq!(
            order[0].declared.as_slice(),
            &[
                ("AppError".to_string(), "RuntimeError".to_string()),
                ("OrderError".to_string(), "AppError".to_string()),
                ("OrderError".to_string(), "Mixin".to_string()),
                ("PaymentError".to_string(), "AppError".to_string()),
                ("MissingKey".to_string(), "KeyError".to_string()),
                ("TimeoutError".to_string(), "Exception".to_string()),
            ]
        );
        let to = |from: &str, to: &str| {
            widened(&py_raising(PY_CLASSES, from), &py_raising(PY_CLASSES, to))
        };
        // Its own base, a base two declarations up, and the standard classes above the
        // standard class it extends.
        for (from, onto) in [
            ("OrderError", "AppError"),
            ("OrderError", "RuntimeError"),
            ("OrderError", "Mixin"),
            ("MissingKey", "KeyError"),
            ("MissingKey", "LookupError"),
        ] {
            let w = to(from, onto);
            assert_eq!(w.len(), 1, "{from} -> {onto}: {w:?}");
            assert!(
                w[0].detail.contains(&format!("from `{from}` to `{onto}`")),
                "{w:?}"
            );
            assert!(to(onto, from).is_empty(), "{onto} -> {from}");
        }
        // A sibling, an unrelated standard class, and a class the file declares under a
        // standard name: the declaration is what counts, not the table.
        for (from, onto) in [
            ("OrderError", "PaymentError"),
            ("OrderError", "ValueError"),
            ("MissingKey", "IndexError"),
            ("TimeoutError", "OSError"),
        ] {
            assert!(to(from, onto).is_empty(), "{from} -> {onto}");
        }
        // A file that declares neither class knows nothing about them.
        assert!(widened(&py_raising("", "OrderError"), &py_raising("", "AppError")).is_empty());
        // A qualified name is not the file's class.
        assert!(to("errors.OrderError", "AppError").is_empty());
    }

    #[test]
    fn a_class_reparented_by_the_change_is_judged_on_the_side_that_shows_the_widening() {
        let reparented = "class AppError(RuntimeError):\n    pass\n\nclass OrderError(RuntimeError):\n    pass\n\n";
        // Base declares `OrderError(AppError)`, head re-parents it: base says a widening.
        let w = widened(
            &py_raising(PY_CLASSES, "OrderError"),
            &py_raising(reparented, "AppError"),
        );
        assert_eq!(w.len(), 1, "{w:?}");
        // The other way round, head declares the relation.
        let w = widened(
            &py_raising(reparented, "OrderError"),
            &py_raising(PY_CLASSES, "AppError"),
        );
        assert_eq!(w.len(), 1, "{w:?}");
        // On neither side: a substitution.
        assert!(widened(
            &py_raising(reparented, "OrderError"),
            &py_raising(reparented, "AppError")
        )
        .is_empty());
    }

    #[test]
    fn each_language_reads_the_parents_a_class_declaration_names() {
        let declared = |found: Vec<ExpectedException>| -> Vec<(String, String)> {
            found[0].declared.as_ref().clone()
        };
        let pair = |c: &str, p: &str| (c.to_string(), p.to_string());
        assert_eq!(
            declared(facts(
                &JavaPack,
                "src/test/java/SutTest.java",
                "class SutTest {\n    static class AppException extends RuntimeException {}\n    static class OrderException extends app.AppException implements Fault {}\n    @Test\n    void t() {\n        assertThrows(OrderException.class, () -> f());\n    }\n}\n",
            )),
            vec![
                pair("AppException", "RuntimeException"),
                pair("OrderException", "app.AppException")
            ]
        );
        assert_eq!(
            declared(facts(
                &CSharpPack,
                "tests/T.cs",
                "public class AppException : System.Exception {}\npublic class OrderException : AppException, IFault {}\npublic class T {\n    [Fact]\n    public void Run() {\n        Assert.ThrowsAny<OrderException>(() => sut.Run());\n    }\n}\n",
            )),
            vec![
                pair("AppException", "System.Exception"),
                pair("OrderException", "AppException"),
                pair("OrderException", "IFault"),
                // The test class extends nothing.
            ]
        );
        for path in ["tests/sut.test.js", "tests/sut.test.ts"] {
            assert_eq!(
                declared(facts(
                    &JavaScriptPack,
                    path,
                    "class AppError extends Error {}\nclass OrderError extends errors.AppError {}\nconst Late = class extends AppError {};\ntest(\"t\", () => {\n  expect(() => f()).toThrow(OrderError);\n});\n",
                )),
                vec![
                    pair("AppError", "Error"),
                    pair("OrderError", "errors.AppError")
                ],
                "{path}"
            );
        }
        assert_eq!(
            declared(facts(
                &PhpPack,
                "tests/SutTest.php",
                "<?php\nclass AppException extends \\RuntimeException {}\nclass OrderException extends AppException implements Fault {}\nclass SutTest extends TestCase {\n    public function testT(): void {\n        $this->expectException(OrderException::class);\n    }\n}\n",
            )),
            vec![
                pair("AppException", "\\RuntimeException"),
                pair("OrderException", "AppException"),
                pair("SutTest", "TestCase")
            ]
        );
        assert_eq!(
            declared(facts(
                &KotlinPack,
                "src/test/kotlin/SutTest.kt",
                "open class AppException(m: String) : RuntimeException(m)\nclass OrderException(m: String) : AppException(m), Fault\nclass SutTest {\n    @Test\n    fun t() {\n        assertFailsWith<OrderException> { f() }\n    }\n}\n",
            )),
            vec![
                pair("AppException", "RuntimeException"),
                pair("OrderException", "AppException"),
                pair("OrderException", "Fault")
            ]
        );
        assert_eq!(
            declared(facts(
                &RubyPack,
                "spec/sut_spec.rb",
                "class AppError < StandardError; end\nclass OrderError < Errors::AppError; end\nclass Plain; end\nRSpec.describe Sut do\n  it \"t\" do\n    expect { f }.to raise_error(OrderError)\n  end\nend\n",
            )),
            vec![
                pair("AppError", "StandardError"),
                pair("OrderError", "Errors::AppError")
            ]
        );
        assert_eq!(
            declared(facts(
                &CppPack,
                "tests/sut_test.cc",
                "#include <gtest/gtest.h>\nclass AppError : public std::runtime_error {};\nstruct OrderError : virtual AppError, private Fault {};\nTEST(Sut, T) {\n  EXPECT_THROW(f(), OrderError);\n}\n",
            )),
            vec![
                pair("AppError", "std::runtime_error"),
                pair("OrderError", "AppError"),
                pair("OrderError", "Fault")
            ]
        );
    }

    #[test]
    fn a_whole_message_and_a_contained_one_are_compared_by_what_they_accept() {
        let with = |matcher: &str, whole: bool| ExpectedException {
            whole_message: whole,
            ..site("assertThrows", "s", Some("E"), Some(matcher))
        };
        let change = |b: &ExpectedException, h: &ExpectedException| is_widened(b, h);
        // The same text, no longer the whole message; a part of it, contained.
        assert!(change(
            &with("negative value", true),
            &with("negative value", false)
        )
        .is_some());
        assert!(change(&with("negative value", true), &with("value", false)).is_some());
        assert!(change(&with("negative value", false), &with("value", false)).is_some());
        // The other way, and two whole messages that differ.
        assert_eq!(
            change(
                &with("negative value", false),
                &with("negative value", true)
            ),
            None
        );
        assert_eq!(
            change(&with("value", false), &with("negative value", true)),
            None
        );
        assert_eq!(
            change(&with("negative value", true), &with("value", true)),
            None
        );
        assert_eq!(
            change(&with("negative value", true), &with("negative value", true)),
            None
        );
        // A whole message that is empty is a constraint; a contained empty one is none.
        assert!(change(&with("", true), &with("", false)).is_some());
        // The two are not the same expectation to the pairing either.
        let w = widened(
            &[with("negative value", true)],
            &[with("negative value", false)],
        );
        assert_eq!(w.len(), 1, "{w:?}");
    }

    #[test]
    fn a_matcher_gained_beside_a_wider_or_missing_type_is_named_in_the_message() {
        let gained = is_widened(
            &site("pytest.raises", "s", Some("ValueError"), None),
            &site("pytest.raises", "s", Some("Exception"), Some("neg")),
        );
        assert_eq!(
            gained.as_deref(),
            Some("expected exception type widened from `ValueError` to `Exception`; the matcher added beside it does not narrow the type")
        );
        let replaced = is_widened(
            &site("toThrow", "s", Some("TypeError"), None),
            &site("toThrow", "s", None, Some("negative")),
        );
        assert_eq!(
            replaced.as_deref(),
            Some("expected exception type `TypeError` was removed; a message matcher replaced it, and any class with that message passes")
        );
        // Controls: no matcher gained, and the trade the other way.
        assert_eq!(
            is_widened(
                &site("toThrow", "s", Some("TypeError"), None),
                &site("toThrow", "s", None, None),
            )
            .as_deref(),
            Some("expected exception type was removed")
        );
        assert_eq!(
            is_widened(
                &site("pytest.raises", "s", Some("Exception"), Some("neg")),
                &site("pytest.raises", "s", Some("ValueError"), None),
            ),
            None
        );
    }

    #[test]
    fn call_text_is_the_call_whatever_its_spacing() {
        let calls = |body: &str| -> Vec<String> {
            let tests = py(&format!("def test_t():\n    {body}\n"));
            tests[0]
                .expectations
                .iter()
                .flat_map(|e| e.calls.clone())
                .collect()
        };
        let plain = calls("assert f(-1, key=\"a b\") == 0");
        assert_eq!(plain, vec!["f ( - 1 , key = \" a b \" )"]);
        assert_eq!(calls("assert f( -1,\n        key = \"a b\" ) == 0"), plain);
        assert_eq!(calls("assert f(-1, key=\"a b\")  ==  0  # f(1)"), plain);
        assert_eq!(
            calls("assert f(-1,  # negative\n        key=\"a b\") == 0"),
            plain
        );
        // Other arguments, another callee, a string that differs only in its spacing.
        assert_ne!(calls("assert f(1, key=\"a b\") == 0"), plain);
        assert_ne!(calls("assert g(-1, key=\"a b\") == 0"), plain);
        assert_ne!(calls("assert f(-1, key=\"ab\") == 0"), plain);
        // Every call of the assertion is listed, the outer and the inner.
        assert_eq!(
            calls("assert f(g(1)) == 0"),
            vec!["f ( g ( 1 ) )", "g ( 1 )"]
        );
    }

    #[test]
    fn a_site_guards_one_call_or_none() {
        let guarded = |body: &str| py_body(body)[0].guarded_call.clone();
        assert_eq!(
            guarded("with pytest.raises(ValueError):\n    f(-1)").as_deref(),
            Some("f ( - 1 )")
        );
        assert_eq!(
            guarded("with pytest.raises(ValueError) as e:\n    await f(-1)  # why").as_deref(),
            Some("f ( - 1 )")
        );
        for body in [
            "with pytest.raises(ValueError):\n    setup()\n    f(-1)",
            "with pytest.raises(ValueError):\n    x = f(-1)",
            "with pytest.raises(ValueError):\n    f(-1) + 1",
            "pytest.raises(ValueError, f, -1)",
        ] {
            assert_eq!(guarded(body), None, "{body}");
        }
        let js_guarded = |body: &str| js(body)[0].guarded_call.clone();
        assert_eq!(
            js_guarded("expect(() => f(-1)).toThrow(RangeError);").as_deref(),
            Some("f ( - 1 )")
        );
        assert_eq!(
            js_guarded("expect(() => { f(-1); }).not.toThrow();").as_deref(),
            Some("f ( - 1 )")
        );
        assert_eq!(
            js_guarded("expect(async () => await f(-1)).rejects.toThrow();").as_deref(),
            Some("f ( - 1 )")
        );
        assert_eq!(js_guarded("expect(run).toThrow();"), None);
        assert_eq!(
            js_guarded("expect(() => { setup(); f(-1); }).toThrow();"),
            None
        );
    }

    #[test]
    fn a_dropped_site_is_not_reported_when_head_asserts_on_the_call_it_guarded() {
        let test = |body: &str| {
            let indented: String = body.lines().map(|l| format!("    {l}\n")).collect();
            py(&format!("def test_t():\n{indented}")).remove(0)
        };
        let base = test("with pytest.raises(ValueError):\n    f(-1)\nassert g() == 1");
        let dropped = |head: &str| widened_in(&base, &test(head));
        assert!(dropped("assert f(-1) == 0\nassert g() == 1").is_empty());
        assert!(dropped("assert 0 == f( -1 )\nassert g() == 1").is_empty());
        assert!(dropped("self.assertEqual(f(-1), 0)\nassert g() == 1").is_empty());
        for head in [
            // Other arguments, another callee, no assertion on the call, and an
            // assertion that is not an equality with a literal.
            "assert f(1) == 0\nassert g() == 1",
            "assert h(-1) == 0\nassert g() == 1",
            "f(-1)\nassert g() == 1",
            "assert f(-1)\nassert g() == 1",
            "assert f(-1) == want\nassert g() == 1",
        ] {
            let w = dropped(head);
            assert_eq!(w.len(), 1, "{head}: {w:?}");
            assert!(w[0].dropped, "{head}");
            assert_eq!(w[0].guarded_call.as_deref(), Some("f ( - 1 )"));
        }
        // The base side already asserted on that call: head gained nothing.
        let asserted = test("with pytest.raises(ValueError):\n    f(-1)\nassert f(-1) == 0");
        assert_eq!(
            widened_in(&asserted, &test("assert f(-1) == 0\nassert g() == 1")).len(),
            1
        );
        // A widened site is not a dropped one: the allowance leaves it reported.
        let w = widened_in(
            &base,
            &test("with pytest.raises(Exception):\n    f(-1)\nassert f(-1) == 0\nassert g() == 1"),
        );
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(!w[0].dropped);
    }

    #[test]
    fn a_dropped_site_names_what_is_no_longer_checked() {
        let dropped = |b: ExpectedException| widened(&[b], &[]).remove(0).detail;
        assert_eq!(
            dropped(site("pytest.raises", "s", Some("ValueError"), Some("neg"))),
            "expected exception `ValueError` is no longer checked"
        );
        assert_eq!(
            dropped(site("expectExceptionMessage", "s", None, Some("neg"))),
            "expected message `neg` is no longer checked"
        );
        assert_eq!(
            dropped(site("toThrow", "s", None, None)),
            "expected failure is no longer checked"
        );
    }

    #[test]
    fn two_rewritten_sites_that_trade_strictness_leave_every_base_site_covered() {
        let base = vec![raises("a", "ValueError"), raises("b", "Exception")];
        let traded = vec![raises("c", "Exception"), raises("d", "ValueError")];
        assert!(widened(&base, &traded).is_empty());
        // Control: with nothing given back, one base site is left without a head site.
        let widened_only = vec![raises("c", "Exception"), raises("d", "Exception")];
        assert_eq!(widened(&base, &widened_only).len(), 1);
    }

    #[test]
    fn python_reads_bare_raises_only_when_pytest_exports_it() {
        let body = "def test_t():\n    with raises(ValueError, match=\"neg\"):\n        f(-1)\n    raises(KeyError, g, 1)\n";
        let read = py_file(&format!("from pytest import raises\n{body}"));
        assert_eq!(read.len(), 2, "{read:?}");
        assert_eq!(read[0].kind, "pytest.raises");
        assert_eq!(read[0].exception_type.as_deref(), Some("ValueError"));
        assert_eq!(read[0].matcher.as_deref(), Some("neg"));
        assert_eq!(read[0].skeleton, "with raises(#): f(-1)");
        assert_eq!(read[1].exception_type.as_deref(), Some("KeyError"));
        assert_eq!(read[1].skeleton, "pytest.raises(#, g, 1)");

        let aliased = py_file(
            "from pytest import (\n    fixture,\n    raises as throws,\n)\ndef test_t():\n    with throws(ValueError):\n        f(-1)\n    with raises(KeyError):\n        g()\n",
        );
        assert_eq!(aliased.len(), 1, "{aliased:?}");
        assert_eq!(aliased[0].exception_type.as_deref(), Some("ValueError"));

        for import in [
            "",
            "from mylib import raises\n",
            "import pytest\n",
            "from pytest import fixture\n",
            "from pytest.helpers import raises\n",
        ] {
            let tests = py(&format!("{import}{body}"));
            assert!(tests[0].expected_exceptions.is_empty(), "{import:?}");
        }
    }

    #[test]
    fn python_reads_assert_warns_apart_from_assert_raises() {
        let unit = |body: &str| {
            py_file(&format!(
                "class TestC(unittest.TestCase):\n    def test_t(self):\n        {body}\n"
            ))
        };
        let warns =
            unit("with self.assertWarnsRegex(DeprecationWarning, \"old\"):\n            f()");
        assert_eq!(warns.len(), 1, "{warns:?}");
        assert_eq!(warns[0].kind, "assertWarns");
        assert_eq!(
            warns[0].exception_type.as_deref(),
            Some("DeprecationWarning")
        );
        assert_eq!(warns[0].matcher.as_deref(), Some("old"));
        assert_eq!(warns[0].skeleton, "with self.assertWarns(#): f()");
        let call = unit("self.assertWarns(DeprecationWarning, f, 1)");
        assert_eq!(call[0].kind, "assertWarns");
        assert_eq!(call[0].skeleton, "self.assertWarns(#, f, 1)");
        // An expected warning does not stand for an expected exception.
        let raises = unit("with self.assertRaises(Warning):\n            g()");
        assert_eq!(raises[0].kind, "assertRaises");
        let w = widened(&warns, &raises);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].dropped, "{w:?}");
        // A method that only ends in the name is not the assertion.
        assert!(unit("self.myassertRaises(ValueError, f)").is_empty());
    }

    #[test]
    fn java_reads_assertj_chains() {
        let thrown = java_test(
            "assertThatThrownBy(() -> sut.run(1)).isInstanceOf(IllegalStateException.class).hasMessageContaining(\"neg\");",
        );
        assert_eq!(thrown.len(), 1, "{thrown:?}");
        assert_eq!(thrown[0].kind, "assertThrows");
        assert_eq!(
            thrown[0].exception_type.as_deref(),
            Some("IllegalStateException")
        );
        assert_eq!(thrown[0].matcher.as_deref(), Some("neg"));
        assert!(!thrown[0].whole_message);
        assert_eq!(thrown[0].skeleton, "assertThatThrownBy(() -> sut.run(1))#");
        assert_eq!(thrown[0].line, 4);

        let exact = java_test(
            "Assertions.assertThatThrownBy(() -> sut.run(1))\n            .hasMessage(\"neg\")\n            .isExactlyInstanceOf(IllegalStateException.class);",
        );
        assert_eq!(exact[0].kind, "assertThrowsExactly");
        assert_eq!(exact[0].matcher.as_deref(), Some("neg"));
        assert!(exact[0].whole_message);
        assert_eq!(exact[0].skeleton, thrown[0].skeleton);

        let bare = java_test("assertThatThrownBy(() -> sut.run(1));");
        assert_eq!(bare.len(), 1);
        assert_eq!(
            (bare[0].exception_type.clone(), bare[0].matcher.clone()),
            (None, None)
        );

        let of_type = java_test(
            "assertThatExceptionOfType(java.io.IOException.class).isThrownBy(() -> sut.run(1)).withMessage(\"neg\");",
        );
        assert_eq!(of_type[0].kind, "assertThrows");
        assert_eq!(
            of_type[0].exception_type.as_deref(),
            Some("java.io.IOException")
        );
        assert!(of_type[0].whole_message);
        assert_eq!(of_type[0].skeleton, thrown[0].skeleton);

        let prefix =
            java_test("assertThatThrownBy(() -> sut.run(1)).hasMessageStartingWith(\"neg\");");
        assert_eq!(
            prefix[0].matcher.as_deref(),
            Some("\u{1}hasMessageStartingWith(\"neg\")")
        );

        let none = java_test("assertThatCode(() -> sut.run(1)).doesNotThrowAnyException();");
        assert_eq!(none.len(), 1, "{none:?}");
        assert_eq!(none[0].kind, "doesNotThrow");
        // `assertThatCode` with a class is the same statement as `assertThatThrownBy`.
        let code = java_test(
            "assertThatCode(() -> sut.run(1)).isInstanceOf(IllegalStateException.class);",
        );
        assert_eq!(code[0].kind, "assertThrows");
        // No expectation: an AssertJ chain on a value, and `assertThatCode` alone.
        assert!(java_test("assertThat(sut.run(1)).isInstanceOf(Result.class);").is_empty());
        assert!(java_test("assertThatCode(() -> sut.run(1));").is_empty());
    }

    #[test]
    fn javascript_reads_node_assert_and_chai() {
        let file = |prelude: &str, body: &str| {
            facts(
                &JavaScriptPack,
                "tests/sut.test.js",
                &format!("{prelude}\ntest(\"t\", () => {{\n  {body}\n}});\n"),
            )
        };
        let node = "const assert = require(\"node:assert\");";
        let class = file(
            node,
            "assert.throws(() => f(-1), RangeError, \"must reject\");",
        );
        assert_eq!(class.len(), 1, "{class:?}");
        assert_eq!(class[0].kind, "toThrow");
        assert_eq!(class[0].exception_type.as_deref(), Some("RangeError"));
        assert_eq!(class[0].matcher, None);
        assert_eq!(class[0].skeleton, "assert.throws(() => f(-1))#");

        let object = file(
            node,
            "assert.throws(() => f(-1), { name: \"RangeError\", message: \"neg\" });",
        );
        assert_eq!(object[0].exception_type.as_deref(), Some("RangeError"));
        assert_eq!(object[0].matcher.as_deref(), Some("neg"));
        assert!(object[0].whole_message);
        let coded = file(node, "assert.throws(() => f(-1), { code: \"E_NEG\" });");
        assert_eq!(
            coded[0].matcher.as_deref(),
            Some("\u{1}{ code: \"E_NEG\" }")
        );
        let pattern = file(node, "assert.throws(() => f(-1), /neg/);");
        assert_eq!(pattern[0].matcher.as_deref(), Some("\u{1}/neg/"));
        // A string after the code is the assertion's message.
        let message = file(node, "assert.throws(() => f(-1), \"must reject\");");
        assert_eq!(
            (
                message[0].exception_type.clone(),
                message[0].matcher.clone()
            ),
            (None, None)
        );

        for prelude in [
            "import assert from \"node:assert/strict\";",
            "import * as assert from \"assert\";",
            "import { strict as assert } from \"assert\";",
            "const assert = require(\"assert\").strict;",
            "const { strict: assert } = require(\"node:assert\");",
            // Bound to nothing the file shows: read as Node's.
            "",
        ] {
            let got = file(prelude, "await assert.rejects(f(-1), RangeError);");
            assert_eq!(got.len(), 1, "{prelude}: {got:?}");
            assert_eq!(got[0].exception_type.as_deref(), Some("RangeError"));
            assert_eq!(got[0].skeleton, "assert.rejects(f(-1))#");
        }
        let negated = file(node, "assert.doesNotThrow(() => f(1), TypeError);");
        assert_eq!(negated[0].kind, "not.toThrow");
        assert_eq!(negated[0].exception_type, None);

        // Chai's `assert`: the class, then the message matcher.
        for prelude in [
            "import { assert } from \"chai\";",
            "const { assert } = require(\"chai\");",
            "const assert = require(\"chai\").assert;",
        ] {
            let got = file(
                prelude,
                "assert.throws(() => f(-1), RangeError, \"neg\", \"why\");",
            );
            assert_eq!(got.len(), 1, "{prelude}: {got:?}");
            assert_eq!(got[0].exception_type.as_deref(), Some("RangeError"));
            assert_eq!(got[0].matcher.as_deref(), Some("neg"), "{prelude}");
        }
        let member = file(
            "const chai = require(\"chai\");",
            "chai.assert.throws(() => f(-1), \"neg\");",
        );
        assert_eq!(member[0].matcher.as_deref(), Some("neg"));
        let chai_negated = file(
            "import { assert } from \"chai\";",
            "assert.doesNotThrow(() => f(1), TypeError);",
        );
        assert_eq!(chai_negated[0].kind, "not.toThrow");
        assert_eq!(chai_negated[0].exception_type.as_deref(), Some("TypeError"));

        // Chai's `expect`.
        let expect = js("expect(() => f(-1)).to.throw(RangeError, \"neg\");");
        assert_eq!(expect.len(), 1, "{expect:?}");
        assert_eq!(expect[0].kind, "toThrow");
        assert_eq!(expect[0].exception_type.as_deref(), Some("RangeError"));
        assert_eq!(expect[0].matcher.as_deref(), Some("neg"));
        assert_eq!(expect[0].skeleton, "expect(() => f(-1)).to.toThrow(#)");
        for (body, kind) in [
            ("expect(() => f(1)).to.not.throw();", "not.toThrow"),
            ("expect(() => f(1)).not.to.throw();", "not.toThrow"),
            ("chai.expect(() => f(-1)).to.throws(RangeError);", "toThrow"),
        ] {
            let got = js(body);
            assert_eq!(got.len(), 1, "{body}: {got:?}");
            assert_eq!(got[0].kind, kind, "{body}");
        }

        // Not an expectation: a double told to throw, another module's `assert`, and a
        // receiver that is no assertion library.
        for (prelude, body) in [
            ("", "stub.throws(new RangeError(\"neg\"));"),
            ("", "sinon.stub(api, \"get\").throws(new Error());"),
            ("", "mock.rejects(new Error());"),
            (
                "const assert = require(\"./my-assert\");",
                "assert.throws(() => f(-1), RangeError);",
            ),
            ("", "verify.throws(() => f(-1), RangeError);"),
        ] {
            let tests = JavaScriptPack
                .extract(
                    "tests/sut.test.js",
                    &format!("{prelude}\ntest(\"t\", () => {{\n  {body}\n}});\n"),
                    &AssertVocabulary::default(),
                )
                .expect("extract succeeds")
                .tests;
            assert!(tests[0].expected_exceptions.is_empty(), "{prelude} {body}");
        }
    }

    #[test]
    fn csharp_reads_nunit_constraints_and_an_awaited_fluent_chain() {
        let only = |body: &str| {
            let t = cs(body);
            assert_eq!(
                t[0].expected_exceptions.len(),
                1,
                "{body}: {:?}",
                t[0].expected_exceptions
            );
            t[0].expected_exceptions[0].clone()
        };
        let exact = only(
            "Assert.That(() => sut.Run(), Throws.TypeOf<ArgumentException>().With.Message.EqualTo(\"neg\"));",
        );
        assert_eq!(exact.kind, "Assert.Throws");
        assert_eq!(exact.exception_type.as_deref(), Some("ArgumentException"));
        assert_eq!(exact.matcher.as_deref(), Some("neg"));
        assert!(exact.whole_message);
        assert_eq!(exact.skeleton, "Assert.That(() => sut.Run(), Throws#)");

        for (body, kind, class) in [
            (
                "Assert.That(() => sut.Run(), Throws.InstanceOf<ArgumentException>());",
                "Assert.ThrowsAny",
                Some("ArgumentException"),
            ),
            (
                "Assert.That(() => sut.Run(), Throws.Exception.TypeOf<System.IO.IOException>());",
                "Assert.Throws",
                Some("System.IO.IOException"),
            ),
            (
                "Assert.That(() => sut.Run(), Throws.TypeOf(typeof(ArgumentException)));",
                "Assert.Throws",
                Some("ArgumentException"),
            ),
            (
                "Assert.That(() => sut.Run(), Throws.ArgumentNullException);",
                "Assert.Throws",
                Some("ArgumentNullException"),
            ),
            (
                "Assert.That(() => sut.Run(), Throws.Exception);",
                "Assert.ThrowsAny",
                None,
            ),
            (
                "Assert.That(() => sut.Run(), Throws.Nothing);",
                "Throws.Nothing",
                None,
            ),
            (
                "await Assert.ThatAsync(() => sut.RunAsync(), Throws.InstanceOf<ArgumentException>());",
                "Assert.ThrowsAny",
                Some("ArgumentException"),
            ),
        ] {
            let got = only(body);
            assert_eq!(got.kind, kind, "{body}");
            assert_eq!(got.exception_type.as_deref(), class, "{body}");
            assert!(got.skeleton.ends_with("Throws#)"), "{body}");
        }
        let contains = only(
            "Assert.That(() => sut.Run(), Throws.InstanceOf<ArgumentException>().With.Message.Contains(\"neg\"));",
        );
        assert_eq!(contains.matcher.as_deref(), Some("neg"));
        assert!(!contains.whole_message);
        let starts = only(
            "Assert.That(() => sut.Run(), Throws.Exception.With.Message.StartsWith(\"neg\"));",
        );
        assert_eq!(starts.matcher.as_deref(), Some("\u{1}StartsWith(\"neg\")"));

        // A constraint that is not about a failure states no expected exception.
        for body in [
            "Assert.That(sut.Run(), Is.EqualTo(1));",
            "Assert.That(sut.Run(), Has.Count.EqualTo(1));",
            "Assert.That(() => sut.Run(), MyThrows.TypeOf<ArgumentException>());",
        ] {
            assert!(cs(body)[0].expected_exceptions.is_empty(), "{body}");
        }

        let awaited =
            only("(await act.Should().ThrowAsync<ArgumentException>()).WithMessage(\"*neg*\");");
        assert_eq!(awaited.kind, "Assert.ThrowsAny");
        assert_eq!(awaited.matcher.as_deref(), Some("*neg*"));
        let chained = only(
            "(await act.Should().ThrowExactlyAsync<ArgumentException>()).And.Message.Should().Be(\"x\");",
        );
        assert_eq!(chained.kind, "Assert.Throws");
        assert_eq!(chained.matcher, None);
        let plain = only("await act.Should().ThrowAsync<ArgumentException>();");
        assert_eq!(plain.matcher, None);
        assert_eq!(plain.skeleton, awaited.skeleton);
    }

    #[test]
    fn php_reads_a_message_pattern_and_an_exception_object() {
        let pattern = php("$this->expectExceptionMessageMatches('/neg \\d+/');\nsut(-1);");
        assert_eq!(pattern.len(), 1, "{pattern:?}");
        assert_eq!(pattern[0].kind, "expectExceptionMessageMatches");
        assert_eq!(pattern[0].matcher.as_deref(), Some("/neg \\d+/"));
        let any = |m: &str| site("expectExceptionMessageMatches", "s", None, Some(m));
        for wildcard in ["/.*/", "/^.*$/s", "#.*#", "~~", "//"] {
            assert!(
                is_widened(&pattern[0], &any(wildcard)).is_some(),
                "{wildcard}"
            );
        }
        // Another pattern, and a string that only looks like a part of the first.
        for other in ["/neg/", "/neg [0-9]+/", "/x.*/"] {
            assert_eq!(is_widened(&pattern[0], &any(other)), None, "{other}");
        }

        let object =
            php("$this->expectExceptionObject(new \\App\\OrderException(\"neg\", 3));\nsut(-1);");
        let kinds: Vec<&str> = object.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec![
                "expectException",
                "expectExceptionMessage",
                "expectExceptionCode"
            ]
        );
        assert_eq!(
            object[0].exception_type.as_deref(),
            Some("\\App\\OrderException")
        );
        assert_eq!(object[1].matcher.as_deref(), Some("neg"));
        assert_eq!(object[2].matcher.as_deref(), Some("\u{1}3"));
        // The object says what the three calls say: rewriting one as the other is silent.
        let calls = php(
            "$this->expectException(\\App\\OrderException::class);\n$this->expectExceptionMessage(\"neg\");\n$this->expectExceptionCode(3);\nsut(-1);",
        );
        assert!(widened(&object, &calls).is_empty());
        assert!(widened(&calls, &object).is_empty());
        let class_only =
            php("$this->expectExceptionObject(new \\App\\OrderException());\nsut(-1);");
        assert_eq!(class_only.len(), 1, "{class_only:?}");
        assert_eq!(widened(&object, &class_only).len(), 2);
        let held = php("$this->expectExceptionObject($expected);\nsut(-1);");
        assert_eq!(held.len(), 1, "{held:?}");
        assert_eq!(held[0].matcher.as_deref(), Some("\u{1}$expected"));
    }

    #[test]
    fn kotlin_reads_junit_kotlin_test_and_kotest_forms() {
        for (body, kind, class) in [
            (
                "assertThrows<IllegalStateException> { sut.run(1) }",
                "assertThrows",
                Some("IllegalStateException"),
            ),
            (
                "assertFailsWith<java.io.IOException> { sut.run(1) }",
                "assertThrows",
                Some("java.io.IOException"),
            ),
            (
                "assertFailsWith(IllegalStateException::class) { sut.run(1) }",
                "assertThrows",
                Some("IllegalStateException"),
            ),
            (
                "assertFailsWith<IllegalStateException>(\"must reject\") { sut.run(1) }",
                "assertThrows",
                Some("IllegalStateException"),
            ),
            (
                "val e = shouldThrow<IllegalStateException> { sut.run(1) }",
                "assertThrows",
                Some("IllegalStateException"),
            ),
            (
                "shouldThrowExactly<IllegalStateException> { sut.run(1) }",
                "assertThrowsExactly",
                Some("IllegalStateException"),
            ),
            (
                "Assertions.assertThrowsExactly<IllegalStateException> { sut.run(1) }",
                "assertThrowsExactly",
                Some("IllegalStateException"),
            ),
            ("assertFails { sut.run(1) }", "assertThrows", None),
            ("shouldThrowAny { sut.run(1) }", "assertThrows", None),
            ("assertDoesNotThrow { sut.run(1) }", "doesNotThrow", None),
            ("shouldNotThrowAny { sut.run(1) }", "doesNotThrow", None),
            (
                "shouldNotThrow<IllegalStateException> { sut.run(1) }",
                "doesNotThrow",
                None,
            ),
        ] {
            let got = kotlin_test(body);
            assert_eq!(got.len(), 1, "{body}: {got:?}");
            assert_eq!(got[0].kind, kind, "{body}");
            assert_eq!(got[0].exception_type.as_deref(), class, "{body}");
            assert_eq!(got[0].skeleton, "assertThrows#{ sut.run(1) }", "{body}");
            assert_eq!(got[0].line, 4, "{body}");
        }
        // Not an expectation: a comparison, a generic call that is none of the forms,
        // and a call with a trailing lambda.
        for body in [
            "val ok = a < b",
            "val made = build<Order> { sut.run(1) }",
            "repeat(3) { sut.run(1) }",
            "assertTrue(sut.run(1) > limit)",
        ] {
            let tests = KotlinPack
                .extract(
                    "src/test/kotlin/SutTest.kt",
                    &format!(
                        "class SutTest {{\n    @Test\n    fun t() {{\n        {body}\n    }}\n}}\n"
                    ),
                    &AssertVocabulary::default(),
                )
                .expect("extract succeeds")
                .tests;
            assert!(tests[0].expected_exceptions.is_empty(), "{body}");
        }
    }

    #[test]
    fn ruby_reads_rspec_and_minitest_forms() {
        let class = rspec("expect { sut.run(1) }.to raise_error(ArgumentError, \"neg\")");
        assert_eq!(class.len(), 1, "{class:?}");
        assert_eq!(class[0].kind, "raise_error");
        assert_eq!(class[0].exception_type.as_deref(), Some("ArgumentError"));
        assert_eq!(class[0].matcher.as_deref(), Some("neg"));
        assert!(class[0].whole_message);
        assert_eq!(class[0].skeleton, "expect { sut.run(1) }.to raise_error#");
        assert_eq!(class[0].line, 3);

        for (body, kind, ty, matcher) in [
            (
                "expect { sut.run(1) }.to raise_error",
                "raise_error",
                None,
                None,
            ),
            (
                "expect { sut.run(1) }.to raise_exception(Errors::Bad)",
                "raise_error",
                Some("Errors::Bad"),
                None,
            ),
            (
                "expect { sut.run(1) }.to raise_error(/neg/)",
                "raise_error",
                None,
                Some("\u{1}/neg/"),
            ),
            (
                "expect { sut.run(1) }.to raise_error(ArgumentError).with_message(/neg/)",
                "raise_error",
                Some("ArgumentError"),
                Some("\u{1}/neg/"),
            ),
            (
                "expect { sut.run(1) }.not_to raise_error",
                "not.raise_error",
                None,
                None,
            ),
            (
                "expect { sut.run(1) }.to_not raise_error(ArgumentError)",
                "not.raise_error",
                Some("ArgumentError"),
                None,
            ),
        ] {
            let got = rspec(body);
            assert_eq!(got.len(), 1, "{body}: {got:?}");
            assert_eq!(got[0].kind, kind, "{body}");
            assert_eq!(got[0].exception_type.as_deref(), ty, "{body}");
            assert_eq!(got[0].matcher.as_deref(), matcher, "{body}");
        }
        let block = rspec("expect do\n      sut.run(1)\n    end.to raise_error(ArgumentError)");
        assert_eq!(block.len(), 1, "{block:?}");
        assert_eq!(block[0].exception_type.as_deref(), Some("ArgumentError"));

        // Not an expectation: another matcher, and a value rather than a block's failure.
        for body in [
            "expect { sut.run(1) }.to change { n }",
            "expect(sut.run(1)).to eq(1)",
            "allow(sut).to receive(:run).and_raise(ArgumentError)",
            // A subject that is not `expect`.
            "retrying { sut.run(1) }.to raise_error(ArgumentError)",
        ] {
            let tests = RubyPack
                .extract(
                    "spec/sut_spec.rb",
                    &format!("RSpec.describe Sut do\n  it \"t\" do\n    {body}\n  end\nend\n"),
                    &AssertVocabulary::default(),
                )
                .expect("extract succeeds")
                .tests;
            assert!(tests[0].expected_exceptions.is_empty(), "{body}");
        }

        let minitest = |body: &str| {
            facts(
                &RubyPack,
                "test/sut_test.rb",
                &format!("class SutTest < Minitest::Test\n  def test_t\n    {body}\n  end\nend\n"),
            )
        };
        let raises = minitest("assert_raises(KeyError, TypeError, \"must reject\") { sut.run(1) }");
        assert_eq!(raises.len(), 1, "{raises:?}");
        assert_eq!(raises[0].kind, "assert_raises");
        assert_eq!(
            raises[0].exception_type.as_deref(),
            Some("KeyError, TypeError")
        );
        assert_eq!(raises[0].skeleton, "assert_raises# { sut.run(1) }");
        let bare = minitest("assert_raises { sut.run(1) }");
        assert_eq!(bare[0].exception_type, None);
    }

    #[test]
    fn cpp_reads_googletest_throw_macros() {
        for (body, kind, class) in [
            (
                "EXPECT_THROW(sut.run(1), std::invalid_argument);",
                "EXPECT_THROW",
                Some("std::invalid_argument"),
            ),
            (
                "ASSERT_THROW(sut.run(1), app::Error<int>);",
                "EXPECT_THROW",
                Some("app::Error<int>"),
            ),
            ("EXPECT_ANY_THROW(sut.run(1));", "EXPECT_THROW", None),
            ("ASSERT_ANY_THROW(sut.run(1));", "EXPECT_THROW", None),
            ("EXPECT_NO_THROW(sut.run(1));", "EXPECT_NO_THROW", None),
            ("ASSERT_NO_THROW(sut.run(1));", "EXPECT_NO_THROW", None),
        ] {
            let got = gtest(body);
            assert_eq!(got.len(), 1, "{body}: {got:?}");
            assert_eq!(got[0].kind, kind, "{body}");
            assert_eq!(got[0].exception_type.as_deref(), class, "{body}");
            assert_eq!(got[0].skeleton, "EXPECT_THROW(sut.run(1), #)", "{body}");
            assert_eq!(got[0].line, 3, "{body}");
        }
        for body in [
            "EXPECT_EQ(sut.run(1), 1);",
            "sut.EXPECT_THROW(1, 2);",
            "expect_throw(sut.run(1), std::invalid_argument);",
        ] {
            let tests = CppPack
                .extract(
                    "tests/sut_test.cc",
                    &format!("#include <gtest/gtest.h>\nTEST(Sut, T) {{\n  {body}\n}}\n"),
                    &AssertVocabulary::default(),
                )
                .expect("extract succeeds")
                .tests;
            assert!(tests[0].expected_exceptions.is_empty(), "{body}");
        }
    }

    #[test]
    fn expectations_of_one_family_and_sign_stand_for_each_other() {
        assert!(interchangeable("assertThrows", "assertThrowsExactly"));
        assert!(interchangeable("raise_error", "assert_raises"));
        assert!(interchangeable("pytest.raises", "assertRaises"));
        for (b, h) in [
            ("assertWarns", "assertRaises"),
            ("pytest.raises", "assertWarns"),
            ("assertThrows", "doesNotThrow"),
            ("raise_error", "not.raise_error"),
            ("EXPECT_THROW", "EXPECT_NO_THROW"),
            ("Assert.Throws", "Throws.Nothing"),
            ("expectExceptionMessage", "expectExceptionMessageMatches"),
            ("assertThrows", "raise_error"),
        ] {
            assert!(!interchangeable(b, h), "{b} / {h}");
        }
        assert!(interchangeable("assertWarns", "assertWarns"));
    }

    const GO_HEAD: &str = "package sut\n\nimport (\n\t\"errors\"\n\t\"testing\"\n\n\t\"github.com/stretchr/testify/assert\"\n\t\"github.com/stretchr/testify/require\"\n)\n\n";

    fn go_file(src: &str) -> Vec<ExpectedException> {
        crate::ast::go::GoPack
            .extract("sut_test.go", src, &AssertVocabulary::default())
            .expect("extract succeeds")
            .tests
            .into_iter()
            .flat_map(|t| t.expected_exceptions)
            .collect()
    }

    fn go(body: &str) -> Vec<ExpectedException> {
        go_file(&format!(
            "{GO_HEAD}func TestRejects(t *testing.T) {{\n\terr := f(-1)\n\t{body}\n}}\n"
        ))
    }

    /// `(kind, type, matcher, whole message)` of the one expectation of `got`.
    fn fact(got: &[ExpectedException]) -> (&str, Option<&str>, Option<&str>, bool) {
        assert_eq!(got.len(), 1, "{got:?}");
        (
            got[0].kind.as_str(),
            got[0].exception_type.as_deref(),
            got[0].matcher.as_deref(),
            got[0].whole_message,
        )
    }

    #[test]
    fn go_reads_testify_error_and_panic_assertions() {
        for (body, want) in [
            ("require.Error(t, err)", ("go.Error", None, None, false)),
            (
                "assert.Errorf(t, err, \"why %d\", 1)",
                ("go.Error", None, None, false),
            ),
            ("require.NoError(t, err)", ("go.NoError", None, None, false)),
            (
                "require.ErrorIs(t, err, ErrGone)",
                ("go.Error", Some("ErrGone"), None, false),
            ),
            (
                "assert.ErrorIs(t, err, fs.ErrNotExist, \"why\")",
                ("go.Error", Some("fs.ErrNotExist"), None, false),
            ),
            (
                "var pe *fs.PathError\n\trequire.ErrorAs(t, err, &pe)",
                ("go.Error", Some("fs.PathError"), None, false),
            ),
            (
                "require.ErrorAs(t, err, target)",
                ("go.Error", Some("target"), None, false),
            ),
            (
                "require.ErrorContains(t, err, \"negative\")",
                ("go.Error", None, Some("negative"), false),
            ),
            (
                "require.EqualError(t, err, `negative value`)",
                ("go.Error", None, Some("negative value"), true),
            ),
            (
                "require.EqualError(t, err, want)",
                ("go.Error", None, Some("\u{1}want"), true),
            ),
            (
                "require.True(t, errors.Is(err, ErrGone))",
                ("go.Error", Some("ErrGone"), None, false),
            ),
            (
                "assert.Panics(t, func() { f(-1) })",
                ("go.Panics", None, None, false),
            ),
            (
                "assert.PanicsWithValue(t, \"boom\", func() { f(-1) })",
                ("go.Panics", None, Some("boom"), true),
            ),
            (
                "assert.PanicsWithError(t, \"boom\", func() { f(-1) })",
                ("go.Panics", None, Some("boom"), true),
            ),
            (
                "assert.NotPanics(t, func() { f(-1) })",
                ("go.NotPanics", None, None, false),
            ),
        ] {
            assert_eq!(fact(&go(body)), want, "{body}");
        }
        // One site of one error has one skeleton, whatever the assertion is.
        let skeleton = |body: &str| go(body)[0].skeleton.clone();
        assert_eq!(skeleton("require.ErrorIs(t, err, ErrGone)"), "error#(err)");
        assert_eq!(
            skeleton("require.NoError(t, err)"),
            skeleton("if !errors.Is(err, ErrGone) {\n\t\tt.Fatal(err)\n\t}")
        );
        assert_eq!(
            skeleton("assert.PanicsWithValue(t, \"boom\", func() { f(-1) })"),
            skeleton("assert.Panics(t, func() { f(-1) })")
        );
        // Not an expected failure: a `testing.T` method, an equality, a negated walk.
        for body in [
            "t.Error(err)",
            "t.Errorf(\"got %v\", err)",
            "require.Equal(t, 1, n)",
            "require.True(t, ok)",
            "require.False(t, errors.Is(err, ErrGone))",
            "other.ErrorIs(t, err, ErrGone)",
        ] {
            assert!(go(body).is_empty(), "{body}");
        }
    }

    #[test]
    fn go_reads_errors_is_and_as_in_a_failing_condition() {
        for (body, want) in [
            (
                "if !errors.Is(err, ErrGone) {\n\t\tt.Fatalf(\"got %v\", err)\n\t}",
                Some("ErrGone"),
            ),
            (
                "if err == nil || !errors.Is(err, io.EOF) {\n\t\tt.Errorf(\"got %v\", err)\n\t}",
                Some("io.EOF"),
            ),
            (
                "var pe *PathError\n\tif !(errors.As(err, &pe)) {\n\t\tt.Fatal(err)\n\t}",
                Some("PathError"),
            ),
            (
                "if _, err := g(); !errors.Is(err, ErrGone) {\n\t\tt.Fatal(err)\n\t}",
                Some("ErrGone"),
            ),
        ] {
            let got = go(body);
            assert_eq!(got.len(), 1, "{body}");
            assert_eq!(fact(&got), ("go.Error", want, None, false), "{body}");
            assert_eq!(got[0].line, body.lines().count() + 10, "{body}");
        }
        for body in [
            // The condition holds when the error IS the sentinel.
            "if errors.Is(err, ErrGone) {\n\t\tt.Fatal(err)\n\t}",
            // Both must fail for the test to fail.
            "if ok && !errors.Is(err, ErrGone) {\n\t\tt.Fatal(err)\n\t}",
            // Nothing fails the test.
            "if !errors.Is(err, ErrGone) {\n\t\tlog.Print(err)\n\t}",
            "if !errors.Is(err, ErrGone) {\n\t\tdefer func() { t.Fatal(err) }()\n\t}",
            "if !mine.Is(err, ErrGone) {\n\t\tt.Fatal(err)\n\t}",
        ] {
            assert!(go(body).is_empty(), "{body}");
        }
    }

    #[test]
    fn go_reads_whose_assertion_it_is_from_the_imports_and_the_function() {
        let with = |imports: &str, body: &str| {
            go_file(&format!(
                "package sut\n\nimport (\n{imports}\n)\n\nfunc TestRejects(t *testing.T) {{\n\t{body}\n}}\n"
            ))
        };
        // A testify package under another name; an errors package under another name.
        assert_eq!(
            fact(&with(
                "\ttr \"github.com/stretchr/testify/require\"",
                "tr.ErrorIs(t, err, ErrGone)"
            )),
            ("go.Error", Some("ErrGone"), None, false)
        );
        assert_eq!(
            fact(&with(
                "\tstderrors \"errors\"",
                "if !stderrors.Is(err, ErrGone) {\n\t\tt.Fatal(err)\n\t}"
            )),
            ("go.Error", Some("ErrGone"), None, false)
        );
        // A name the file binds to another package, or to nothing, is not read.
        assert!(with(
            "\t\"example.com/own/require\"",
            "require.ErrorIs(t, err, ErrGone)"
        )
        .is_empty());
        assert!(with("\t\"testing\"", "require.ErrorIs(t, err, ErrGone)").is_empty());
        assert!(with(
            "\t\"example.com/own/errors\"",
            "if !errors.Is(err, ErrGone) {\n\t\tt.Fatal(err)\n\t}"
        )
        .is_empty());

        // A suite's receiver, its `Require()`, and a local bound to an assertion object.
        let suite = |body: &str| {
            go_file(&format!(
                "package sut\n\nimport (\n\t\"testing\"\n\n\t\"github.com/stretchr/testify/require\"\n\t\"github.com/stretchr/testify/suite\"\n)\n\ntype S struct {{\n\tsuite.Suite\n}}\n\nfunc (s *S) TestRejects() {{\n\t{body}\n}}\n\nfunc TestS(t *testing.T) {{\n\tsuite.Run(t, new(S))\n}}\n"
            ))
        };
        for body in [
            "s.ErrorIs(err, ErrGone)",
            "s.Require().ErrorIs(err, ErrGone)",
            "s.Assert().ErrorIs(err, ErrGone)",
            "r := s.Require()\n\tr.ErrorIs(err, ErrGone)",
            "a := require.New(s.T())\n\ta.ErrorIs(err, ErrGone)",
        ] {
            assert_eq!(
                fact(&suite(body)),
                ("go.Error", Some("ErrGone"), None, false),
                "{body}"
            );
        }
        assert_eq!(
            fact(&suite("s.Error(err)")),
            ("go.Error", None, None, false)
        );
        assert_eq!(
            fact(&suite("s.Require().Error(err)")),
            ("go.Error", None, None, false)
        );
        assert!(suite("v.Require().ErrorIs(err, ErrGone)").is_empty());
        assert!(suite("s.db.ErrorIs(err, ErrGone)").is_empty());
    }

    #[test]
    fn go_reads_gocheck_checkers_as_expected_failures() {
        let gocheck = |import: &str, body: &str| {
            go_file(&format!(
                "package sut\n\nimport (\n\t{import}\n)\n\ntype S struct{{}}\n\nfunc (s *S) TestRejects(c *C) {{\n\t{body}\n}}\n"
            ))
        };
        let dot = ". \"gopkg.in/check.v1\"";
        for (body, want) in [
            (
                "c.Assert(err, ErrorMatches, \"negative value\")",
                ("go.Error", None, Some("negative value"), true),
            ),
            (
                "c.Check(err, ErrorMatches, \"negative .*\")",
                ("go.Error", None, Some("\u{1}/negative .*/"), false),
            ),
            (
                "c.Assert(func() { f(-1) }, PanicMatches, `boom`)",
                ("go.Panics", None, Some("boom"), true),
            ),
            (
                "c.Assert(func() { f(-1) }, Panics, \"boom\")",
                ("go.Panics", None, Some("boom"), true),
            ),
            (
                "c.Assert(func() { f(-1) }, Panics, ErrBoom)",
                ("go.Panics", None, Some("\u{1}ErrBoom"), true),
            ),
        ] {
            assert_eq!(fact(&gocheck(dot, body)), want, "{body}");
        }
        assert!(gocheck(dot, "c.Assert(n, Equals, 1)").is_empty());
        assert!(gocheck(dot, "c.Assert(err, IsNil)").is_empty());
        // Under the name the file imports the package by, and by no other.
        for import in [
            "\"gopkg.in/check.v1\"",
            "check \"github.com/go-check/check\"",
        ] {
            assert_eq!(
                fact(&gocheck(import, "c.Assert(err, check.ErrorMatches, \"x\")")),
                ("go.Error", None, Some("x"), true),
                "{import}"
            );
        }
        assert!(gocheck(
            "\"gopkg.in/check.v1\"",
            "c.Assert(err, ErrorMatches, \"x\")"
        )
        .is_empty());
        assert!(gocheck(
            "\"example.com/own/check\"",
            "c.Assert(err, check.ErrorMatches, \"x\")"
        )
        .is_empty());
        assert!(gocheck(dot, "c.Assert(err, gc.ErrorMatches, \"x\")").is_empty());
    }

    #[test]
    fn go_expectations_are_compared_by_name_presence_and_message() {
        let wider = |b: &str, h: &str| widened(&go(b), &go(h));
        let one = |b: &str, h: &str| {
            let got = wider(b, h);
            assert_eq!(got.len(), 1, "{b} -> {h}: {got:?}");
            got[0].detail.clone()
        };
        assert_eq!(
            one("require.ErrorIs(t, err, ErrGone)", "require.Error(t, err)"),
            "expected exception type was removed"
        );
        assert_eq!(
            one(
                "require.ErrorContains(t, err, \"negative\")",
                "require.Error(t, err)"
            ),
            "expected pattern or message matcher was removed"
        );
        assert_eq!(
            one(
                "require.EqualError(t, err, \"negative value\")",
                "require.ErrorContains(t, err, \"negative value\")"
            ),
            "expected pattern or message matcher is no longer the whole message and matches more messages"
        );
        assert!(one(
            "require.EqualError(t, err, \"negative value\")",
            "require.ErrorContains(t, err, \"negative\")"
        )
        .contains("matches more messages"));
        assert_eq!(
            one(
                "assert.PanicsWithValue(t, \"boom\", func() { f(-1) })",
                "assert.Panics(t, func() { f(-1) })"
            ),
            "expected pattern or message matcher was removed"
        );
        assert_eq!(
            one(
                "require.ErrorIs(t, err, ErrGone)\n\trequire.ErrorContains(t, err, \"negative\")",
                "require.ErrorIs(t, err, ErrGone)"
            ),
            "expected message `negative` is no longer checked"
        );
        // A name Go has no relation for: `Error` and `Exception` are sentinels like any.
        for (b, h) in [
            (
                "require.ErrorIs(t, err, ErrGone)",
                "require.ErrorIs(t, err, ErrOther)",
            ),
            (
                "require.ErrorIs(t, err, ErrGone)",
                "require.ErrorIs(t, err, Error)",
            ),
            (
                "require.ErrorIs(t, err, ErrGone)",
                "require.ErrorIs(t, err, pkg.Exception)",
            ),
            ("require.Error(t, err)", "require.ErrorIs(t, err, ErrGone)"),
            ("require.Error(t, err)", "require.NoError(t, err)"),
            ("require.NoError(t, err)", "require.Error(t, err)"),
            (
                "require.ErrorContains(t, err, \"negative\")",
                "require.EqualError(t, err, \"negative\")",
            ),
            (
                "assert.Panics(t, func() { f(-1) })",
                "assert.NotPanics(t, func() { f(-1) })",
            ),
            (
                "assert.Panics(t, func() { f(-1) })",
                "assert.PanicsWithError(t, \"boom\", func() { f(-1) })",
            ),
        ] {
            assert!(wider(b, h).is_empty(), "{b} -> {h}");
        }
        // A panic and a returned error do not stand for each other.
        assert!(!interchangeable("go.Panics", "go.Error"));
        assert!(!interchangeable("go.NotPanics", "go.NoError"));
        assert!(!interchangeable("go.Error", "go.NoError"));
        assert!(interchangeable("go.Error", "go.Error"));
    }

    fn swift_file(src: &str) -> Vec<ExpectedException> {
        crate::ast::swift::SwiftPack
            .extract(
                "Tests/SutTests/SutTests.swift",
                src,
                &AssertVocabulary::default(),
            )
            .expect("extract succeeds")
            .tests
            .into_iter()
            .flat_map(|t| t.expected_exceptions)
            .collect()
    }

    fn xctest(body: &str) -> Vec<ExpectedException> {
        swift_file(&format!(
            "import XCTest\n@testable import Sut\n\nfinal class SutTests: XCTestCase {{\n    func testRejects() throws {{\n        {body}\n    }}\n}}\n"
        ))
    }

    fn swift_testing(body: &str) -> Vec<ExpectedException> {
        swift_file(&format!(
            "import Testing\n@testable import Sut\n\n@Test func rejects() throws {{\n    {body}\n}}\n"
        ))
    }

    #[test]
    fn swift_reads_xctest_throwing_assertions_and_the_type_their_closure_checks() {
        for (body, want) in [
            ("XCTAssertThrowsError(try f(-1))", ("swift.throws", None)),
            ("XCTAssertThrowsError(try f(-1), \"why\")", ("swift.throws", None)),
            (
                "XCTAssertThrowsError(try f(-1)) { error in\n  XCTAssertEqual(error as? SutError, .negative)\n}",
                ("swift.throws", Some("SutError")),
            ),
            (
                "XCTAssertThrowsError(try f(-1)) { error in\n  XCTAssertTrue(error is SutError)\n}",
                ("swift.throws", Some("SutError")),
            ),
            (
                "XCTAssertThrowsError(try f(-1), \"why\") { (e) in\n  XCTAssertTrue(e is Client.Error)\n}",
                ("swift.throws", Some("Client.Error")),
            ),
            (
                "XCTAssertThrowsError(try f(-1)) {\n  XCTAssertEqual($0 as? SutError, SutError.negative)\n}",
                ("swift.throws", Some("SutError")),
            ),
            (
                "XCTAssertThrowsError(try f(-1)) { error in\n  XCTAssertTrue(error is any Error)\n}",
                ("swift.throws", Some("any Error")),
            ),
            // Every error bridges to `NSError`, and a cast of another value names nothing.
            (
                "XCTAssertThrowsError(try f(-1)) { error in\n  XCTAssertEqual((error as NSError).code, 3)\n}",
                ("swift.throws", None),
            ),
            (
                "XCTAssertThrowsError(try f(-1)) { error in\n  XCTAssertTrue(other is SutError)\n}",
                ("swift.throws", None),
            ),
            ("XCTAssertNoThrow(try f(1))", ("swift.noThrow", None)),
        ] {
            let got = xctest(body);
            assert_eq!(got.len(), 1, "{body}: {got:?}");
            assert_eq!(
                (got[0].kind.as_str(), got[0].exception_type.as_deref()),
                want,
                "{body}"
            );
            assert_eq!(got[0].line, 6, "{body}");
            assert_eq!(got[0].matcher, None, "{body}");
        }
        assert_eq!(
            xctest("XCTAssertThrowsError(try f(-1)) { _ in }")[0].skeleton,
            xctest("XCTAssertNoThrow(try  f(-1))")[0].skeleton
        );
        for body in [
            "XCTAssertEqual(try f(1), 1)",
            "sut.XCTAssertThrowsError(try f(-1))",
        ] {
            assert!(xctest(body).is_empty(), "{body}");
        }
        // A file that does not import XCTest calls a function of its own.
        assert!(swift_file(
            "import Testing\n\n@Test func rejects() {\n    XCTAssertThrowsError(try f(-1))\n}\n"
        )
        .is_empty());
    }

    #[test]
    fn swift_reads_the_throws_argument_of_expect_and_require() {
        for (body, want) in [
            (
                "#expect(throws: SutError.self) { try f(-1) }",
                ("swift.throws", Some("SutError"), None),
            ),
            (
                "#expect(throws: SutError.self, \"why\") {\n    try f(-1)\n}",
                ("swift.throws", Some("SutError"), None),
            ),
            (
                "try #require(throws: Client.Error.self) { try f(-1) }",
                ("swift.throws", Some("Client.Error"), None),
            ),
            (
                "#expect(throws: (any Error).self) { try f(-1) }",
                ("swift.throws", Some("any Error"), None),
            ),
            (
                "#expect(throws: Error.self) { try f(-1) }",
                ("swift.throws", Some("any Error"), None),
            ),
            (
                "#expect(throws: SutError.negative) { try f(-1) }",
                (
                    "swift.throws",
                    Some("SutError"),
                    Some("\u{1}SutError.negative"),
                ),
            ),
            (
                "#expect(throws: SutError(code: 3)) { try f(-1) }",
                (
                    "swift.throws",
                    Some("SutError"),
                    Some("\u{1}SutError(code: 3)"),
                ),
            ),
            (
                "let error = #expect(throws: SutError.self) { try f(-1) }",
                ("swift.throws", Some("SutError"), None),
            ),
            (
                "#expect(throws: Never.self) { try f(1) }",
                ("swift.noThrow", None, None),
            ),
        ] {
            let got = swift_testing(body);
            assert_eq!(got.len(), 1, "{body}: {got:?}");
            assert_eq!(
                (
                    got[0].kind.as_str(),
                    got[0].exception_type.as_deref(),
                    got[0].matcher.as_deref()
                ),
                want,
                "{body}"
            );
        }
        // The closure is the site, bound to a name or not.
        assert_eq!(
            swift_testing("let error = #expect(throws: SutError.self) { try f(-1) }")[0].skeleton,
            swift_testing("#expect(throws: Never.self) { try f(-1) }")[0].skeleton
        );
        for body in [
            "#expect(f(1) == 1)",
            "#expect(thrown: SutError.self) { try f(-1) }",
        ] {
            assert!(swift_testing(body).is_empty(), "{body}");
        }
        assert!(swift_file(
            "import XCTest\n\nfinal class SutTests: XCTestCase {\n    func testRejects() {\n        #expect(throws: SutError.self) { try f(-1) }\n    }\n}\n"
        )
        .is_empty());
    }

    #[test]
    fn swift_error_types_are_compared_by_name_with_any_error_above_them() {
        let wider = |b: &str, h: &str| widened(&swift_testing(b), &swift_testing(h));
        let one = |b: &str, h: &str| {
            let got = wider(b, h);
            assert_eq!(got.len(), 1, "{b} -> {h}: {got:?}");
            got[0].detail.clone()
        };
        assert_eq!(
            one(
                "#expect(throws: SutError.self) { try f(-1) }",
                "#expect(throws: (any Error).self) { try f(-1) }"
            ),
            "expected exception type widened from `SutError` to `any Error`"
        );
        assert_eq!(
            one(
                "#expect(throws: SutError.negative) { try f(-1) }",
                "#expect(throws: SutError.self) { try f(-1) }"
            ),
            "expected pattern or message matcher was removed"
        );
        for (b, h) in [
            // Another type, a type of the project named `Error`, a narrower expectation.
            (
                "#expect(throws: SutError.self) { try f(-1) }",
                "#expect(throws: OtherError.self) { try f(-1) }",
            ),
            (
                "#expect(throws: SutError.self) { try f(-1) }",
                "#expect(throws: Client.Error.self) { try f(-1) }",
            ),
            (
                "#expect(throws: (any Error).self) { try f(-1) }",
                "#expect(throws: SutError.self) { try f(-1) }",
            ),
            (
                "#expect(throws: SutError.self) { try f(-1) }",
                "#expect(throws: SutError.negative) { try f(-1) }",
            ),
            // A sign flip.
            (
                "#expect(throws: SutError.self) { try f(-1) }",
                "#expect(throws: Never.self) { try f(-1) }",
            ),
        ] {
            assert!(wider(b, h).is_empty(), "{b} -> {h}");
        }
        let x = |b: &str, h: &str| widened(&xctest(b), &xctest(h));
        let typed =
            "XCTAssertThrowsError(try f(-1)) { error in\n  XCTAssertTrue(error is SutError)\n}";
        assert_eq!(
            x(typed, "XCTAssertThrowsError(try f(-1))")[0].detail,
            "expected exception type was removed"
        );
        assert!(x("XCTAssertThrowsError(try f(-1))", typed).is_empty());
        assert!(x(typed, "XCTAssertNoThrow(try f(-1))").is_empty());
        assert!(!interchangeable("swift.throws", "swift.noThrow"));
        assert!(interchangeable("swift.throws", "swift.throws"));
    }

    fn scala_file(src: &str) -> Vec<ExpectedException> {
        crate::ast::scala::ScalaPack
            .extract(
                "src/test/scala/SutSpec.scala",
                src,
                &AssertVocabulary::default(),
            )
            .expect("extract succeeds")
            .tests
            .into_iter()
            .flat_map(|t| t.expected_exceptions)
            .collect()
    }

    fn scalatest(body: &str) -> Vec<ExpectedException> {
        scala_file(&format!(
            "import org.scalatest.funsuite.AnyFunSuite\n\nclass SutSpec extends AnyFunSuite with Matchers {{\n  test(\"rejects\") {{\n    {body}\n  }}\n}}\n"
        ))
    }

    fn specs2(body: &str) -> Vec<ExpectedException> {
        scala_file(&format!(
            "import org.specs2.mutable.Specification\n\nclass SutSpec extends Specification {{\n  \"sut\" should {{\n    \"reject\" in {{\n      {body}\n    }}\n  }}\n}}\n"
        ))
    }

    #[test]
    fn scala_reads_scalatest_and_specs2_expected_exceptions() {
        let iae = Some("IllegalArgumentException");
        for (got, want) in [
            (
                scalatest("intercept[IllegalArgumentException] { f(-1) }"),
                ("assertThrows", iae, None, false),
            ),
            (
                scalatest("val e = intercept[java.io.IOException] {\n      f(-1)\n    }"),
                ("assertThrows", Some("java.io.IOException"), None, false),
            ),
            (
                scalatest("assertThrows[IllegalArgumentException](f(-1))"),
                ("assertThrows", iae, None, false),
            ),
            (
                scalatest("the [IllegalArgumentException] thrownBy { f(-1) } should have message \"negative\""),
                ("assertThrows", iae, Some("negative"), true),
            ),
            (
                scalatest("val e = the [IllegalArgumentException] thrownBy f(-1)"),
                ("assertThrows", iae, None, false),
            ),
            (
                scalatest("a [IllegalArgumentException] should be thrownBy { f(-1) }"),
                ("assertThrows", iae, None, false),
            ),
            (
                scalatest("an [IllegalArgumentException] should be thrownBy f(-1)"),
                ("assertThrows", iae, None, false),
            ),
            (
                scalatest("noException should be thrownBy { f(1) }"),
                ("doesNotThrow", None, None, false),
            ),
            (
                specs2("f(-1) must throwA[IllegalArgumentException]"),
                ("assertThrows", iae, None, false),
            ),
            (
                specs2("f(-1) must throwAn[IllegalArgumentException](\"negative\")"),
                ("assertThrows", iae, Some("negative"), false),
            ),
            (
                specs2("f(-1) must throwAn[IllegalArgumentException](message = \"neg.*\")"),
                ("assertThrows", iae, Some("\u{1}/neg.*/"), false),
            ),
            (
                specs2("f(-1) must throwA(new IllegalArgumentException(\"negative\"))"),
                ("assertThrows", iae, None, false),
            ),
        ] {
            assert_eq!(fact(&got), want, "{got:?}");
        }
        // One guarded code is one site, whichever form states it.
        assert_eq!(
            scalatest("intercept[IllegalArgumentException] { f(-1) }")[0].skeleton,
            scalatest("noException should be thrownBy { f(-1) }")[0].skeleton
        );
        for body in [
            "assert(f(1) == 1)",
            "sut.intercept[IllegalArgumentException] { f(-1) }",
            "intercept(f(-1))",
            "retry[IllegalArgumentException] { f(-1) }",
            "f(1) must beLike[IllegalArgumentException]",
            "a [Sut] should be definedAt 1",
            "the [Sut] producedBy { f(-1) } should have message \"negative\"",
            "f(1) must beA[Sut]",
            "f(1) must not(throwA[Exception])",
        ] {
            assert!(scalatest(body).is_empty(), "{body}");
        }
    }

    #[test]
    fn scala_expectations_use_the_java_table_and_the_classes_of_the_file() {
        let wider = |b: &str, h: &str| widened(&scalatest(b), &scalatest(h));
        let one = |b: &str, h: &str| {
            let got = wider(b, h);
            assert_eq!(got.len(), 1, "{b} -> {h}: {got:?}");
            got[0].detail.clone()
        };
        assert_eq!(
            one(
                "intercept[scala.NumberFormatException] { f(-1) }",
                "intercept[IllegalArgumentException] { f(-1) }"
            ),
            "expected exception type widened from `NumberFormatException` to `IllegalArgumentException`"
        );
        assert!(one(
            "the [IllegalArgumentException] thrownBy { f(-1) } should have message \"negative\"",
            "an [IllegalArgumentException] should be thrownBy { f(-1) }"
        )
        .contains("matcher was removed"));
        for (b, h) in [
            (
                "intercept[IllegalArgumentException] { f(-1) }",
                "intercept[NumberFormatException] { f(-1) }",
            ),
            (
                "intercept[my.IllegalArgumentException] { f(-1) }",
                "intercept[RuntimeException] { f(-1) }",
            ),
            (
                "intercept[IllegalArgumentException] { f(-1) }",
                "noException should be thrownBy { f(-1) }",
            ),
        ] {
            assert!(wider(b, h).is_empty(), "{b} -> {h}");
        }
        let declared = |body: &str| {
            scala_file(&format!(
                "class AppError(m: String) extends RuntimeException(m) with Marker\nclass OrderError(m: String) extends AppError(m)\nclass PaymentError(m: String) extends AppError(m)\n\nclass SutSpec extends AnyFunSuite {{\n  test(\"rejects\") {{\n    {body}\n  }}\n}}\n"
            ))
        };
        let moved = |h: &str| widened(&declared("intercept[OrderError] { f(-1) }"), &declared(h));
        assert_eq!(moved("intercept[AppError] { f(-1) }").len(), 1);
        assert_eq!(moved("intercept[RuntimeException] { f(-1) }").len(), 1);
        assert!(moved("intercept[PaymentError] { f(-1) }").is_empty());
    }

    fn objc(body: &str) -> Vec<ExpectedException> {
        crate::ast::objc::ObjcPack
            .extract(
                "Tests/SutTests.m",
                &format!(
                    "#import <XCTest/XCTest.h>\n\n@interface SutTests : XCTestCase\n@end\n\n@implementation SutTests\n- (void)testRejects {{\n    {body}\n}}\n@end\n"
                ),
                &AssertVocabulary::default(),
            )
            .expect("extract succeeds")
            .tests
            .into_iter()
            .flat_map(|t| t.expected_exceptions)
            .collect()
    }

    #[test]
    fn objc_reads_xctest_throwing_assertions() {
        for (body, want) in [
            ("XCTAssertThrows([sut run:-1]);", ("objc.throws", None, None, false)),
            (
                "XCTAssertThrows([sut run:-1], @\"why %d\", 1);",
                ("objc.throws", None, None, false),
            ),
            (
                "XCTAssertThrowsSpecific([sut run:-1], SutException);",
                ("objc.throws", Some("SutException"), None, false),
            ),
            (
                "XCTAssertThrowsSpecific([sut run:-1], SutException, @\"why\");",
                ("objc.throws", Some("SutException"), None, false),
            ),
            (
                "XCTAssertThrowsSpecificNamed([sut run:-1], NSException, NSInvalidArgumentException, @\"why\");",
                (
                    "objc.throws",
                    Some("NSException"),
                    Some("\u{1}NSInvalidArgumentException"),
                    true,
                ),
            ),
            (
                "XCTAssertThrowsSpecificNamed([sut run:-1], SutException, @\"Negative\");",
                ("objc.throws", Some("SutException"), Some("Negative"), true),
            ),
            ("XCTAssertNoThrow([sut run:1]);", ("objc.noThrow", None, None, false)),
            (
                "XCTAssertNoThrowSpecific([sut run:1], SutException);",
                ("objc.noThrow", Some("SutException"), None, false),
            ),
        ] {
            let got = objc(body);
            assert_eq!(fact(&got), want, "{body}");
            assert_eq!(got[0].skeleton, got[0].skeleton.replace("  ", " "), "{body}");
            assert_eq!(got[0].line, 8, "{body}");
        }
        assert_eq!(
            objc("XCTAssertThrowsSpecific([sut  run:-1], SutException);")[0].skeleton,
            objc("XCTAssertNoThrow([sut run:-1]);")[0].skeleton
        );
        for body in [
            "XCTAssertEqual([sut run:1], 1);",
            "[helper XCTAssertThrows:value];",
            "helper->XCTAssertThrows([sut run:-1]);",
        ] {
            assert!(objc(body).is_empty(), "{body}");
        }
    }

    #[test]
    fn objc_classes_are_compared_by_name_with_nsexception_above_them() {
        let wider = |b: &str, h: &str| widened(&objc(b), &objc(h));
        let one = |b: &str, h: &str| {
            let got = wider(b, h);
            assert_eq!(got.len(), 1, "{b} -> {h}: {got:?}");
            got[0].detail.clone()
        };
        assert_eq!(
            one(
                "XCTAssertThrowsSpecific([sut run:-1], SutException);",
                "XCTAssertThrowsSpecific([sut run:-1], NSException);"
            ),
            "expected exception type widened from `SutException` to `NSException`"
        );
        assert_eq!(
            one(
                "XCTAssertThrowsSpecific([sut run:-1], SutException);",
                "XCTAssertThrows([sut run:-1]);"
            ),
            "expected exception type was removed"
        );
        assert_eq!(
            one(
                "XCTAssertThrowsSpecificNamed([sut run:-1], NSException, NSRangeException);",
                "XCTAssertThrowsSpecific([sut run:-1], NSException);"
            ),
            "expected pattern or message matcher was removed"
        );
        // "Does not throw X" passes on any other failure: naming a class is the widening.
        assert!(one(
            "XCTAssertNoThrow([sut run:1]);",
            "XCTAssertNoThrowSpecific([sut run:1], SutException);"
        )
        .starts_with("negated expectation now names a type"));
        for (b, h) in [
            (
                "XCTAssertThrowsSpecific([sut run:-1], SutException);",
                "XCTAssertThrowsSpecific([sut run:-1], OtherException);",
            ),
            (
                "XCTAssertThrowsSpecific([sut run:-1], SutException);",
                "XCTAssertThrowsSpecific([sut run:-1], Exception);",
            ),
            (
                "XCTAssertThrows([sut run:-1]);",
                "XCTAssertThrowsSpecific([sut run:-1], SutException);",
            ),
            (
                "XCTAssertNoThrowSpecific([sut run:1], SutException);",
                "XCTAssertNoThrow([sut run:1]);",
            ),
            (
                "XCTAssertThrows([sut run:-1]);",
                "XCTAssertNoThrow([sut run:-1]);",
            ),
        ] {
            assert!(wider(b, h).is_empty(), "{b} -> {h}");
        }
    }

    fn catch2(body: &str) -> Vec<ExpectedException> {
        crate::ast::c_cpp::CppPack
            .extract(
                "tests/sut_test.cpp",
                &format!("#include <catch2/catch_test_macros.hpp>\nTEST_CASE(\"rejects\") {{\n  {body}\n}}\n"),
                &AssertVocabulary::default(),
            )
            .expect("extract succeeds")
            .tests
            .into_iter()
            .flat_map(|t| t.expected_exceptions)
            .collect()
    }

    #[test]
    fn cpp_reads_catch2_and_doctest_throwing_macros() {
        let ia = Some("std::invalid_argument");
        for (body, want) in [
            (
                "REQUIRE_THROWS(f(-1));",
                ("EXPECT_THROW", None, None, false),
            ),
            ("CHECK_THROWS(f(-1));", ("EXPECT_THROW", None, None, false)),
            (
                "REQUIRE_THROWS_AS(f(-1), std::invalid_argument);",
                ("EXPECT_THROW", ia, None, false),
            ),
            (
                "CHECK_THROWS_AS(f(-1), std::invalid_argument);",
                ("EXPECT_THROW", ia, None, false),
            ),
            (
                "REQUIRE_THROWS_WITH(f(-1), \"negative\");",
                ("EXPECT_THROW", None, Some("negative"), true),
            ),
            (
                "CHECK_THROWS_WITH(f(-1), ContainsSubstring(\"negative\"));",
                (
                    "EXPECT_THROW",
                    None,
                    Some("\u{1}ContainsSubstring(\"negative\")"),
                    false,
                ),
            ),
            (
                "REQUIRE_THROWS_MATCHES(f(-1), std::invalid_argument, Message(\"negative\"));",
                (
                    "EXPECT_THROW",
                    ia,
                    Some("\u{1}Message(\"negative\")"),
                    false,
                ),
            ),
            (
                "CHECK_THROWS_WITH_AS(f(-1), \"negative\", std::invalid_argument);",
                ("EXPECT_THROW", ia, Some("negative"), true),
            ),
            (
                "REQUIRE_THROWS_AS_MESSAGE(f(-1), std::invalid_argument, \"why\");",
                ("EXPECT_THROW", ia, None, false),
            ),
            (
                "CHECK_THROWS_WITH_MESSAGE(f(-1), \"negative\", \"why\");",
                ("EXPECT_THROW", None, Some("negative"), true),
            ),
            (
                "CHECK_THROWS_MESSAGE(f(-1), \"why\");",
                ("EXPECT_THROW", None, None, false),
            ),
            (
                "REQUIRE_NOTHROW(f(1));",
                ("EXPECT_NO_THROW", None, None, false),
            ),
            (
                "CHECK_NOTHROW_MESSAGE(f(1), \"why\");",
                ("EXPECT_NO_THROW", None, None, false),
            ),
        ] {
            let got = catch2(body);
            assert_eq!(fact(&got), want, "{body}");
            assert_eq!(
                got[0].skeleton.replace("f(1)", "f(-1)"),
                "EXPECT_THROW(f(-1), #)",
                "{body}"
            );
        }
        for body in [
            "REQUIRE(f(1) == 1);",
            "WARN_THROWS(f(-1));",
            "REQUIRE_THROWS_LATER(f(-1));",
            "REQUIRE_FALSE(f(1));",
        ] {
            assert!(catch2(body).is_empty(), "{body}");
        }
        let wider = |b: &str, h: &str| widened(&catch2(b), &catch2(h)).len();
        assert_eq!(
            wider(
                "REQUIRE_THROWS_AS(f(-1), std::invalid_argument);",
                "REQUIRE_THROWS_AS(f(-1), std::logic_error);"
            ),
            1
        );
        assert_eq!(
            wider(
                "REQUIRE_THROWS_AS(f(-1), std::invalid_argument);",
                "REQUIRE_THROWS(f(-1));"
            ),
            1
        );
        assert_eq!(
            wider(
                "REQUIRE_THROWS_WITH(f(-1), \"negative\");",
                "CHECK_THROWS(f(-1));"
            ),
            1
        );
        assert_eq!(
            wider(
                "CHECK_THROWS_WITH_AS(f(-1), \"negative\", std::invalid_argument);",
                "CHECK_THROWS_AS(f(-1), std::invalid_argument);"
            ),
            1
        );
        assert_eq!(
            wider(
                "EXPECT_THROW(f(-1), std::invalid_argument);",
                "REQUIRE_THROWS_AS(f(-1), std::invalid_argument);"
            ),
            0
        );
        assert_eq!(
            wider(
                "REQUIRE_THROWS(f(-1));",
                "REQUIRE_THROWS_AS(f(-1), std::invalid_argument);"
            ),
            0
        );
        assert_eq!(
            wider("REQUIRE_THROWS(f(-1));", "REQUIRE_NOTHROW(f(-1));"),
            0
        );
    }

    #[test]
    fn java_reads_assertj_typed_entry_points_as_the_class_they_name() {
        for (body, want) in [
            (
                "assertThatIllegalArgumentException().isThrownBy(() -> f(-1));",
                ("assertThrows", Some("IllegalArgumentException"), None, false),
            ),
            (
                "assertThatNullPointerException().isThrownBy(() -> f(-1)).withMessage(\"negative\");",
                ("assertThrows", Some("NullPointerException"), Some("negative"), true),
            ),
            (
                "assertThatIllegalStateException().isThrownBy(() -> f(-1)).withMessageContaining(\"neg\");",
                ("assertThrows", Some("IllegalStateException"), Some("neg"), false),
            ),
            (
                "assertThatIOException().isThrownBy(() -> f(-1));",
                ("assertThrows", Some("IOException"), None, false),
            ),
        ] {
            assert_eq!(fact(&java_test(body)), want, "{body}");
        }
        // The same site as the entry point that takes the class.
        assert_eq!(
            java_test("assertThatIllegalArgumentException().isThrownBy(() -> f(-1));")[0].skeleton,
            java_test("assertThatExceptionOfType(Exception.class).isThrownBy(() -> f(-1));")[0]
                .skeleton
        );
        assert!(java_test("assertThatSutException().isThrownBy(() -> f(-1));").is_empty());
        let wider = |b: &str, h: &str| widened(&java_test(b), &java_test(h)).len();
        assert_eq!(
            wider(
                "assertThatExceptionOfType(NumberFormatException.class).isThrownBy(() -> f(-1));",
                "assertThatIllegalArgumentException().isThrownBy(() -> f(-1));"
            ),
            1
        );
        assert_eq!(
            wider(
                "assertThatIllegalArgumentException().isThrownBy(() -> f(-1)).withMessage(\"negative\");",
                "assertThatIllegalArgumentException().isThrownBy(() -> f(-1));"
            ),
            1
        );
        assert_eq!(
            wider(
                "assertThatIllegalArgumentException().isThrownBy(() -> f(-1));",
                "assertThatExceptionOfType(NumberFormatException.class).isThrownBy(() -> f(-1));"
            ),
            0
        );
    }

    #[test]
    fn js_reads_chai_should_style_from_the_subject_before_should() {
        for (body, want) in [
            (
                "(() => f(-1)).should.throw(RangeError, \"negative\");",
                ("toThrow", Some("RangeError"), Some("negative")),
            ),
            (
                "fn.should.throw(RangeError);",
                ("toThrow", Some("RangeError"), None),
            ),
            ("fn.should.throw();", ("toThrow", None, None)),
            ("fn.should.not.throw();", ("not.toThrow", None, None)),
            (
                "fn.should.Throw(/negative/);",
                ("toThrow", None, Some("\u{1}/negative/")),
            ),
        ] {
            let got = js(body);
            assert_eq!(got.len(), 1, "{body}: {got:?}");
            assert_eq!(
                (
                    got[0].kind.as_str(),
                    got[0].exception_type.as_deref(),
                    got[0].matcher.as_deref()
                ),
                want,
                "{body}"
            );
        }
        assert_eq!(
            js("(() => f(-1)).should.throw(RangeError);")[0]
                .guarded_call
                .as_deref(),
            Some("f ( - 1 )")
        );
        // No `should` before the call: a stub told to throw, an object's own method.
        for body in [
            "stub.throws(new RangeError());",
            "fn.shouldnt.throw(RangeError);",
            "sut.throw(RangeError);",
        ] {
            assert!(js(body).is_empty(), "{body}");
        }
        let wider = |b: &str, h: &str| widened(&js(b), &js(h)).len();
        assert_eq!(
            wider(
                "fn.should.throw(RangeError, \"negative\");",
                "fn.should.throw(RangeError);"
            ),
            1
        );
        assert_eq!(
            wider("fn.should.throw(RangeError);", "fn.should.throw(Error);"),
            1
        );
        assert_eq!(
            wider("fn.should.throw(RangeError);", "fn.should.throw();"),
            1
        );
        assert_eq!(
            wider("fn.should.not.throw();", "fn.should.not.throw(RangeError);"),
            1
        );
        assert_eq!(
            wider("fn.should.throw();", "fn.should.throw(RangeError);"),
            0
        );
        // As with `expect(fn).to.throw()` -> `expect(fn).to.not.throw()`: the chain is the
        // site, so the negated chain is another site and the expectation is gone.
        assert_eq!(wider("fn.should.throw();", "fn.should.not.throw();"), 1);
        assert_eq!(
            wider("expect(fn).to.throw();", "expect(fn).to.not.throw();"),
            1
        );
    }

    #[test]
    fn java_reads_catch_throwable_with_the_assertions_on_the_caught_value() {
        let iae = Some("IllegalArgumentException");
        for (body, want) in [
            (
                "Throwable thrown = catchThrowable(() -> f(-1));\n        assertThat(thrown).isInstanceOf(IllegalArgumentException.class).hasMessage(\"negative\");",
                ("assertThrows", iae, Some("negative"), true),
            ),
            (
                "Throwable thrown = catchThrowable(() -> f(-1));\n        assertThat(thrown).isExactlyInstanceOf(IllegalArgumentException.class);\n        assertThat(thrown).hasMessageContaining(\"neg\");",
                ("assertThrowsExactly", iae, Some("neg"), false),
            ),
            (
                "IllegalArgumentException e = catchThrowableOfType(() -> f(-1), IllegalArgumentException.class);\n        assertThat(e).hasMessage(\"negative\");",
                ("assertThrows", iae, Some("negative"), true),
            ),
            (
                "var e = catchThrowableOfType(IllegalArgumentException.class, () -> f(-1));\n        assertThat(e).isNotNull();",
                ("assertThrows", iae, None, false),
            ),
            (
                "Throwable thrown;\n        thrown = catchThrowable(() -> f(-1));\n        assertThat(thrown).isNotNull();",
                ("assertThrows", None, None, false),
            ),
        ] {
            let got = java_test(body);
            assert_eq!(fact(&got), want, "{body}");
            assert_eq!(got[0].skeleton, "assertThatThrownBy(() -> f(-1))#", "{body}");
        }
        // The same site as `assertThatThrownBy` on the same code.
        assert_eq!(
            java_test("assertThatThrownBy(() -> f(-1)).isInstanceOf(Exception.class);")[0].skeleton,
            "assertThatThrownBy(() -> f(-1))#"
        );
        for body in [
            // Nothing asserts on the value: `null` passes.
            "Throwable thrown = catchThrowable(() -> f(-1));",
            "Throwable thrown = catchThrowable(() -> f(-1));\n        assertThat(other).isInstanceOf(IllegalArgumentException.class);",
            "Throwable thrown = catchThrowable(() -> f(-1));\n        assertThat(thrown.getCause()).isNull();",
            "use(catchThrowable(() -> f(-1)));",
        ] {
            assert!(java_test(body).is_empty(), "{body}");
        }
        let wider = |b: &str, h: &str| widened(&java_test(b), &java_test(h)).len();
        let caught = |assertion: &str| {
            format!("Throwable thrown = catchThrowable(() -> f(-1));\n        assertThat(thrown){assertion};")
        };
        assert_eq!(
            wider(
                &caught(".isInstanceOf(NumberFormatException.class)"),
                &caught(".isInstanceOf(IllegalArgumentException.class)")
            ),
            1
        );
        assert_eq!(
            wider(
                &caught(".isInstanceOf(IllegalArgumentException.class).hasMessage(\"negative\")"),
                &caught(".isInstanceOf(IllegalArgumentException.class)")
            ),
            1
        );
        assert_eq!(
            wider(
                "assertThatThrownBy(() -> f(-1)).isInstanceOf(NumberFormatException.class);",
                &caught(".isInstanceOf(RuntimeException.class)")
            ),
            1
        );
        assert_eq!(
            wider(
                &caught(".isInstanceOf(IllegalArgumentException.class)"),
                &caught(".isInstanceOf(NumberFormatException.class)")
            ),
            0
        );
    }

    /// The assertion on a message is the first one made on the expectation, or on the
    /// name it is bound to, in the function that holds the expectation.
    #[test]
    fn kotlin_reads_the_first_message_assertion_in_the_function_of_the_expectation() {
        let matchers = |body: &str| -> Vec<Option<String>> {
            kotlin_test(body).into_iter().map(|e| e.matcher).collect()
        };
        let site = |name: &str, arg: i32| {
            format!("val {name} = shouldThrow<IllegalArgumentException> {{ f({arg}) }}\n        ")
        };
        let some = |m: &str| Some(m.to_string());
        // Two expectations and an assertion for each: each reads its own.
        assert_eq!(
            matchers(&format!(
                "{}{}b.message shouldBe \"second\"\n        a.message shouldContain \"first\"",
                site("a", 1),
                site("b", 2)
            )),
            vec![some("first"), some("second")]
        );
        // Two assertions on one name: the first one is read, whichever form it has.
        assert_eq!(
            matchers(&format!(
                "{}a shouldHaveMessage \"whole\"\n        a.message shouldContain \"part\"",
                site("a", 1)
            )),
            vec![some("whole")]
        );
        assert_eq!(
            matchers(&format!(
                "{}a.message shouldEndWith \"part\"\n        a shouldHaveMessage \"whole\"",
                site("a", 1)
            )),
            vec![some("\u{1}shouldEndWith(\"part\")")]
        );
        // An assertion written before the expectation is in its function all the same.
        assert_eq!(
            matchers(&format!(
                "a.message shouldBe \"before\"\n        {}",
                site("a", 1)
            )),
            vec![some("before")]
        );
        // An assertion in another function is not: the name there is another one.
        let two_functions = KotlinPack
            .extract(
                "src/test/kotlin/SutTest.kt",
                "class SutTest {\n    @Test\n    fun t() {\n        val a = shouldThrow<IllegalArgumentException> { f(1) }\n    }\n    @Test\n    fun u() {\n        val a = shouldThrow<IllegalArgumentException> { f(2) }\n        a.message shouldBe \"second\"\n    }\n}\n",
                &crate::ast::AssertVocabulary::default(),
            )
            .unwrap();
        assert_eq!(
            two_functions
                .tests
                .iter()
                .flat_map(|t| t.expected_exceptions.iter())
                .map(|e| e.matcher.clone())
                .collect::<Vec<_>>(),
            vec![None, some("second")]
        );
        // A matcher that is not on a message, on the same name, is passed over.
        assert_eq!(
            matchers(&format!(
                "{}a shouldBe other\n        a.message shouldBe \"read\"",
                site("a", 1)
            )),
            vec![some("read")]
        );
    }

    /// Expectations in one Kotlin function cost steps in proportion to their number.
    /// Before #672 the function was walked for each expectation with no assertion on its
    /// message, so four times the expectations cost sixteen times the steps.
    #[test]
    fn kotlin_expectations_cost_steps_in_proportion_to_their_number() {
        let steps_of = |n: usize| -> u64 {
            let body: String = (0..n)
                .map(|i| format!("        assertThrows<E> {{ f({i}) }}\n"))
                .collect();
            let src = format!("class SutTest {{\n    @Test\n    fun t() {{\n{body}    }}\n}}\n");
            let (facts, counted) = crate::ast::ancestry::steps(|| {
                KotlinPack.extract(
                    "src/test/kotlin/SutTest.kt",
                    &src,
                    &crate::ast::AssertVocabulary::default(),
                )
            });
            let held: usize = facts
                .unwrap()
                .tests
                .iter()
                .map(|t| t.expected_exceptions.len())
                .sum();
            assert_eq!(held, n);
            counted
        };
        let (few, many) = (steps_of(40), steps_of(160));
        assert!(many < 5 * few, "{few} steps for 40, {many} for 160");
    }

    #[test]
    fn kotlin_reads_the_kotest_assertion_on_the_message_of_the_returned_exception() {
        let iae = Some("IllegalArgumentException");
        let bound = |after: &str| {
            kotlin_test(&format!(
                "val e = shouldThrow<IllegalArgumentException> {{ f(-1) }}\n        {after}"
            ))
        };
        for (got, want) in [
            (bound("e.message shouldBe \"negative\""), ("assertThrows", iae, Some("negative"), true)),
            (bound("e shouldHaveMessage \"negative\""), ("assertThrows", iae, Some("negative"), true)),
            (bound("e.message shouldContain \"neg\""), ("assertThrows", iae, Some("neg"), false)),
            (bound("e.message.shouldBe(\"negative\")"), ("assertThrows", iae, Some("negative"), true)),
            (
                bound("e.message shouldStartWith \"neg\""),
                ("assertThrows", iae, Some("\u{1}shouldStartWith(\"neg\")"), false),
            ),
            (
                kotlin_test("shouldThrow<IllegalArgumentException> { f(-1) }.message shouldBe \"negative\""),
                ("assertThrows", iae, Some("negative"), true),
            ),
            (
                kotlin_test("val e = shouldThrowExactly<IllegalArgumentException> {\n            f(-1)\n        }\n        e.message shouldBe \"negative\""),
                ("assertThrowsExactly", iae, Some("negative"), true),
            ),
            // No assertion on the message of this value.
            (bound("other.message shouldBe \"negative\""), ("assertThrows", iae, None, false)),
            (bound("e.cause shouldBe null"), ("assertThrows", iae, None, false)),
            (bound("n shouldBe 0"), ("assertThrows", iae, None, false)),
        ] {
            assert_eq!(fact(&got), want, "{got:?}");
        }
        // The site is the guarded block, with a message assertion or without.
        assert_eq!(
            bound("e.message shouldBe \"negative\"")[0].skeleton,
            kotlin_test("shouldThrow<IllegalArgumentException> { f(-1) }")[0].skeleton
        );
        let wider = |b: &[ExpectedException], h: &[ExpectedException]| widened(b, h).len();
        assert_eq!(
            wider(
                &bound("e.message shouldBe \"negative\""),
                &bound("n shouldBe 0")
            ),
            1
        );
        assert_eq!(
            wider(
                &bound("e.message shouldBe \"negative\""),
                &bound("e.message shouldContain \"negative\"")
            ),
            1
        );
        assert_eq!(
            wider(
                &bound("n shouldBe 0"),
                &bound("e.message shouldBe \"negative\"")
            ),
            0
        );
    }

    /// The expectations of one C# test with `body`, in a file with the `using` lines.
    fn cs_using(using: &str, body: &str) -> Vec<ExpectedException> {
        let src = format!(
            "{using}\npublic class T {{\n    [TestMethod]\n    public void Run() {{\n        {body}\n    }}\n}}\n"
        );
        CSharpPack
            .extract("tests/T.cs", &src, &AssertVocabulary::default())
            .expect("extract succeeds")
            .tests[0]
            .expected_exceptions
            .clone()
    }

    const MSTEST: &str = "using Microsoft.VisualStudio.TestTools.UnitTesting;";

    #[test]
    fn csharp_throws_is_exact_or_not_by_the_using_directives_of_its_file() {
        let legacy = "Assert.ThrowsException<ArgumentException>(() => sut.Run());";
        let throws = "Assert.Throws<ArgumentException>(() => sut.Run());";
        let throws_null = "Assert.Throws<ArgumentNullException>(() => sut.Run());";
        let exactly = "Assert.ThrowsExactly<ArgumentException>(() => sut.Run());";
        let lost =
            "expected exception is no longer checked as the exact type, so a subclass passes";
        let change = |b: (&str, &str), h: (&str, &str)| -> Vec<String> {
            widened(&cs_using(b.0, b.1), &cs_using(h.0, h.1))
                .into_iter()
                .map(|w| w.detail)
                .collect()
        };
        // (a) The head file is xUnit's or NUnit's: `Assert.Throws<T>` is exact.
        for exact in [
            "using Xunit;",
            "using NUnit.Framework;",
            "global using Xunit;",
        ] {
            assert!(
                change((MSTEST, legacy), (exact, throws)).is_empty(),
                "{exact}"
            );
            assert!(
                change((MSTEST, exactly), (exact, throws)).is_empty(),
                "{exact}"
            );
            assert!(
                change((exact, throws_null), (exact, throws)).is_empty(),
                "{exact}"
            );
        }
        // (b) The head file is MSTest's: `Assert.Throws<T>` accepts subclasses, with
        // `Assert.ThrowsExactly` elsewhere in the file or without.
        assert_eq!(change((MSTEST, legacy), (MSTEST, throws)), vec![lost]);
        assert_eq!(change((MSTEST, exactly), (MSTEST, throws)), vec![lost]);
        assert_eq!(
            change(("using Xunit;", throws), (MSTEST, throws)),
            vec![lost]
        );
        assert_eq!(
            change((MSTEST, throws_null), (MSTEST, throws)),
            vec!["expected exception type widened from `ArgumentNullException` to `ArgumentException`"]
        );
        assert!(change((MSTEST, legacy), (MSTEST, exactly)).is_empty());
        assert!(change((MSTEST, throws), (MSTEST, legacy)).is_empty());
        assert!(change((MSTEST, throws), (MSTEST, throws)).is_empty());
        // (c) No directive names a framework, or two do, or one binds another name: the
        // calls of the file decide.
        for unknown in [
            "",
            "using System;",
            "using Xunit;\nusing Microsoft.VisualStudio.TestTools.UnitTesting;",
            "using Assert = Xunit.Assert;",
            "using static Microsoft.VisualStudio.TestTools.UnitTesting.Assert;",
        ] {
            assert_eq!(
                change((unknown, legacy), (unknown, throws)),
                vec![lost],
                "{unknown}"
            );
            assert_eq!(
                change((unknown, exactly), (unknown, throws)),
                vec![lost],
                "{unknown}"
            );
            assert!(
                change((unknown, throws_null), (unknown, throws)).is_empty(),
                "{unknown}"
            );
            assert!(
                change((unknown, legacy), (unknown, exactly)).is_empty(),
                "{unknown}"
            );
        }
        assert_eq!(
            widened(
                &cs_using(
                    "",
                    "await Assert.ThrowsExceptionAsync<ArgumentException>(() => sut.Run());"
                ),
                &cs_using(
                    "",
                    "await Assert.ThrowsAsync<ArgumentException>(() => sut.Run());"
                )
            )
            .len(),
            1
        );
    }
}
