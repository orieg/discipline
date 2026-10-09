//! The same reading of a conditional skip for C#, Ruby, PHP, Swift, Scala, C / C++ and
//! Objective-C.
//!
//! Each grammar names its nodes differently, so a node is first read into a [`Shape`]: an
//! environment read, a negation, `and` / `or`, a comparison with a constant, a name, a
//! call of a function of the file, or something this module does not decide. The value
//! of a condition is then computed from shapes alone, with the table the parent module
//! uses: true in CI, true outside CI, or involving a CI variable undecidedly.
//!
//! [`read_skip`] answers for one skip: the conditions that enclose it (`if`, `unless`,
//! `guard`, a ternary, each with its `else` branch negated) and the condition the skip
//! takes itself (`XCTSkipIf(..)`, `assume(..)`).

use super::{
    and, combine, exact_mention, flip, is_ci_env_read_name, mention, one, verdict_of, Ci,
    CiVerdict, SkipCondition, Truth, Val,
};
use crate::ast::ancestry::{Above, Ancestry};
use std::collections::{BTreeSet, HashMap, HashSet};
use tree_sitter::Node;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grammar {
    CSharp,
    Ruby,
    Php,
    Swift,
    Scala,
    /// C and C++.
    C,
    ObjC,
}

/// What one skip does to its test, and the condition to report it under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkipRead {
    /// The condition as reported; empty when the skip is unconditional.
    pub text: String,
    pub outcome: SkipCondition,
}

enum Lit {
    Falsy,
    Truthy,
    Str(String),
    Num(String),
}

enum Shape<'t> {
    /// A read of the named environment variable.
    Env(String),
    Not(Node<'t>),
    And(Node<'t>, Node<'t>),
    Or(Node<'t>, Node<'t>),
    /// A comparison of two operands for equality (`true`) or inequality.
    Cmp(Node<'t>, Node<'t>, bool),
    /// A wrapper that keeps what its operand says: parentheses, a cast, `.to_s`.
    Inner(Node<'t>),
    /// A wrapper that inverts it: `.nil?`, `string.IsNullOrEmpty(x)`, `.isEmpty`.
    Flip(Node<'t>),
    Lit(Lit),
    /// A variable, a constant, or a call without arguments that may be either.
    Name(String),
    /// A call of a function or method by its bare name.
    Call(String),
    Comment,
    Other,
}

#[derive(Clone, Copy)]
enum Bind<'t> {
    Value(Node<'t>),
    Func(Node<'t>),
}

type Binds<'t> = HashMap<String, Vec<Bind<'t>>>;

/// Conditions that hold together where a node runs, or with `negated` fail together.
struct Guard<'t> {
    conds: Vec<Node<'t>>,
    negated: bool,
}

const MAX_DEPTH: usize = 8;

fn named<'t>(n: Node<'t>) -> Vec<Node<'t>> {
    let mut cursor = n.walk();
    n.named_children(&mut cursor)
        .filter(|c| !c.kind().contains("comment"))
        .collect()
}

fn same(a: Option<Node>, b: Node) -> bool {
    a.is_some_and(|a| a.id() == b.id())
}

struct Reader<'t, 's> {
    g: Grammar,
    src: &'s [u8],
    module: Binds<'t>,
}

impl<'t, 's> Reader<'t, 's> {
    fn text(&self, n: Node) -> &'s str {
        n.utf8_text(self.src).unwrap_or("")
    }

    fn field_text(&self, n: Node, field: &str) -> &'s str {
        n.child_by_field_name(field)
            .map(|c| self.text(c))
            .unwrap_or("")
    }

    /// The content of a plain string literal: no interpolation, no escape.
    fn string(&self, n: Node<'t>) -> Option<String> {
        if !matches!(
            n.kind(),
            "string_literal"
                | "verbatim_string_literal"
                | "string"
                | "encapsed_string"
                | "line_string_literal"
        ) {
            return None;
        }
        let parts = named(n);
        if parts.iter().any(|c| {
            !matches!(
                c.kind(),
                "string_content" | "string_literal_content" | "line_str_text" | "string_fragment"
            )
        }) {
            return None;
        }
        if !parts.is_empty() {
            return Some(parts.iter().map(|c| self.text(*c)).collect());
        }
        let raw = self.text(n).trim().trim_start_matches('@');
        let quote = raw.chars().next()?;
        if !matches!(quote, '"' | '\'') || raw.contains('\\') {
            return None;
        }
        let inner = raw.strip_prefix(quote)?.strip_suffix(quote)?;
        Some(inner.to_string())
    }

    /// The arguments of a call, in order.
    fn args(&self, call: Node<'t>) -> Vec<Node<'t>> {
        if call.kind() == "message_expression" {
            let receiver = call.child_by_field_name("receiver");
            let mut cursor = call.walk();
            let methods: Vec<Node> = call.children_by_field_name("method", &mut cursor).collect();
            return named(call)
                .into_iter()
                .filter(|c| !same(receiver, *c) && !methods.iter().any(|m| m.id() == c.id()))
                .collect();
        }
        let list = call.child_by_field_name("arguments").or_else(|| {
            let suffix = named(call)
                .into_iter()
                .find(|c| c.kind() == "call_suffix")?;
            named(suffix)
                .into_iter()
                .find(|c| c.kind() == "value_arguments")
        });
        list.map(named).unwrap_or_default()
    }

    fn string_arg(&self, call: Node<'t>) -> Option<String> {
        let first = *self.args(call).first()?;
        self.string(self.unwrap(first))
    }

    /// The expression under an argument wrapper.
    fn unwrap(&self, n: Node<'t>) -> Node<'t> {
        match n.kind() {
            "argument" => named(n).last().copied().unwrap_or(n),
            "value_argument" => n.child_by_field_name("value").unwrap_or(n),
            _ => n,
        }
    }

    fn is_objc_process_info(&self, n: Node<'t>) -> bool {
        match n.kind() {
            "message_expression" => {
                self.field_text(n, "receiver") == "NSProcessInfo"
                    && self.field_text(n, "method") == "processInfo"
            }
            "field_expression" => {
                self.field_text(n, "argument") == "NSProcessInfo"
                    && self.field_text(n, "field") == "processInfo"
            }
            _ => false,
        }
    }

    fn is_objc_environment(&self, n: Node<'t>) -> bool {
        let (owner, member) = match n.kind() {
            "message_expression" => (n.child_by_field_name("receiver"), "method"),
            "field_expression" => (n.child_by_field_name("argument"), "field"),
            _ => return false,
        };
        self.field_text(n, member) == "environment"
            && owner.is_some_and(|o| self.is_objc_process_info(o))
    }

    /// Whether a C-family callee is `getenv`, also as `std::getenv` or `::getenv`.
    fn is_c_getenv(&self, callee: Node<'t>) -> bool {
        match callee.kind() {
            "identifier" => matches!(self.text(callee), "getenv" | "secure_getenv"),
            "qualified_identifier" => {
                self.field_text(callee, "name") == "getenv"
                    && matches!(self.field_text(callee, "scope"), "std" | "")
            }
            _ => false,
        }
    }

    /// The variable an environment read names, when `n` is one.
    fn env_read(&self, n: Node<'t>) -> Option<String> {
        match (self.g, n.kind()) {
            (Grammar::CSharp, "invocation_expression") => {
                let func = n.child_by_field_name("function")?;
                if func.kind() != "member_access_expression"
                    || self.field_text(func, "name") != "GetEnvironmentVariable"
                    || !matches!(
                        self.field_text(func, "expression"),
                        "Environment" | "System.Environment"
                    )
                {
                    return None;
                }
                self.string_arg(n)
            }
            (Grammar::Ruby, "element_reference") => {
                if self.field_text(n, "object") != "ENV" {
                    return None;
                }
                let object = n.child_by_field_name("object");
                let keys: Vec<Node> = named(n).into_iter().filter(|c| !same(object, *c)).collect();
                match keys.as_slice() {
                    [key] => self.string(*key),
                    _ => None,
                }
            }
            (Grammar::Ruby, "call") => {
                if self.field_text(n, "receiver") != "ENV"
                    || !matches!(
                        self.field_text(n, "method"),
                        "fetch" | "key?" | "has_key?" | "include?" | "member?" | "[]"
                    )
                {
                    return None;
                }
                self.string_arg(n)
            }
            (Grammar::Php, "function_call_expression") => {
                let func = n.child_by_field_name("function")?;
                let name = match func.kind() {
                    "name" => self.text(func),
                    "qualified_name" => self.text(*named(func).last()?),
                    _ => return None,
                };
                if name != "getenv" {
                    return None;
                }
                self.string_arg(n)
            }
            (Grammar::Php, "subscript_expression") => {
                let parts = named(n);
                let [object, key] = parts.as_slice() else {
                    return None;
                };
                if !matches!(self.text(*object), "$_ENV" | "$_SERVER") {
                    return None;
                }
                self.string(*key)
            }
            (Grammar::Swift, "call_expression") => {
                let callee = *named(n).first()?;
                let subscript = self.text(n)[callee.end_byte() - n.start_byte()..]
                    .trim_start()
                    .starts_with('[');
                let on_environment = callee.kind() == "navigation_expression"
                    && subscript
                    && matches!(
                        self.text(callee),
                        "ProcessInfo.processInfo.environment"
                            | "Foundation.ProcessInfo.processInfo.environment"
                    );
                let getenv = callee.kind() == "simple_identifier"
                    && !subscript
                    && self.text(callee) == "getenv";
                if !on_environment && !getenv {
                    return None;
                }
                self.string_arg(n)
            }
            (Grammar::Scala, "call_expression") => {
                let func = n.child_by_field_name("function")?;
                if func.kind() != "field_expression" {
                    return None;
                }
                let whole = self.text(func);
                let owner = self.field_text(func, "value");
                let member = self.field_text(func, "field");
                let on_environment =
                    matches!(owner, "sys.env" | "scala.sys.env" | "System.getenv()")
                        && matches!(
                            member,
                            "get"
                                | "contains"
                                | "getOrElse"
                                | "apply"
                                | "isDefinedAt"
                                | "getOrDefault"
                                | "containsKey"
                        );
                if matches!(whole, "sys.env" | "scala.sys.env" | "System.getenv") || on_environment
                {
                    return self.string_arg(n);
                }
                // A system property is a CI variable only when its name is one.
                let on_properties = matches!(owner, "sys.props" | "scala.sys.props")
                    && matches!(member, "get" | "contains" | "getOrElse" | "apply");
                if whole == "System.getProperty" || on_properties {
                    let name = self.string_arg(n)?;
                    return Some(mention(&name).unwrap_or("").to_string());
                }
                None
            }
            (Grammar::C | Grammar::ObjC, "call_expression") => {
                let func = n.child_by_field_name("function")?;
                if !self.is_c_getenv(func) {
                    return None;
                }
                self.string_arg(n)
            }
            (Grammar::ObjC, "subscript_expression") => {
                let object = n.child_by_field_name("argument")?;
                if !self.is_objc_environment(object) {
                    return None;
                }
                self.string(n.child_by_field_name("index")?)
            }
            (Grammar::ObjC, "message_expression") => {
                let receiver = n.child_by_field_name("receiver")?;
                if !self.is_objc_environment(receiver)
                    || !matches!(
                        self.field_text(n, "method"),
                        "objectForKey" | "valueForKey" | "objectForKeyedSubscript"
                    )
                {
                    return None;
                }
                self.string_arg(n)
            }
            _ => None,
        }
    }

    /// A node with a left operand, an operator and a right operand.
    fn binary(&self, n: Node<'t>) -> Shape<'t> {
        let pair = |l: &str, r: &str| Some((n.child_by_field_name(l)?, n.child_by_field_name(r)?));
        let Some((left, right)) = pair("left", "right").or_else(|| pair("lhs", "rhs")) else {
            return Shape::Other;
        };
        if left.end_byte() > right.start_byte() {
            return Shape::Other;
        }
        let op = std::str::from_utf8(&self.src[left.end_byte()..right.start_byte()])
            .unwrap_or("")
            .trim();
        match op {
            "&&" | "and" => Shape::And(left, right),
            "||" | "or" | "??" | "?:" => Shape::Or(left, right),
            "==" | "===" => Shape::Cmp(left, right, true),
            "!=" | "!==" | "<>" => Shape::Cmp(left, right, false),
            _ => Shape::Other,
        }
    }

    fn only_child(&self, n: Node<'t>) -> Shape<'t> {
        match named(n).as_slice() {
            [inner] => Shape::Inner(*inner),
            _ => Shape::Other,
        }
    }

    fn last_child(&self, n: Node<'t>) -> Shape<'t> {
        named(n).last().map_or(Shape::Other, |c| Shape::Inner(*c))
    }

    fn negation(&self, n: Node<'t>, operand: Option<Node<'t>>) -> Shape<'t> {
        let negates = n
            .child(0)
            .is_some_and(|c| matches!(self.text(c).trim(), "!" | "not"));
        match operand.or_else(|| named(n).last().copied()) {
            Some(operand) if negates => Shape::Not(operand),
            _ => Shape::Other,
        }
    }

    fn boolean(&self, n: Node<'t>) -> Shape<'t> {
        Shape::Lit(if self.text(n).trim().eq_ignore_ascii_case("true") {
            Lit::Truthy
        } else {
            Lit::Falsy
        })
    }

    fn shape(&self, n: Node<'t>) -> Shape<'t> {
        let kind = n.kind();
        if kind.contains("comment") {
            return Shape::Comment;
        }
        if let Some(var) = self.env_read(n) {
            return Shape::Env(var);
        }
        if let Some(s) = self.string(n) {
            return Shape::Lit(Lit::Str(s));
        }
        match self.g {
            Grammar::CSharp => self.shape_csharp(n),
            Grammar::Ruby => self.shape_ruby(n),
            Grammar::Php => self.shape_php(n),
            Grammar::Swift => self.shape_swift(n),
            Grammar::Scala => self.shape_scala(n),
            Grammar::C | Grammar::ObjC => self.shape_c(n),
        }
    }

    fn shape_csharp(&self, n: Node<'t>) -> Shape<'t> {
        match n.kind() {
            "parenthesized_expression"
            | "argument"
            | "arrow_expression_clause"
            | "cast_expression" => self.last_child(n),
            "postfix_unary_expression" if self.text(n).trim_end().ends_with('!') => {
                self.last_child(n)
            }
            "prefix_unary_expression" | "unary_expression" => self.negation(n, None),
            "binary_expression" => self.binary(n),
            "is_pattern_expression" => {
                let (Some(value), Some(mut pattern)) = (
                    n.child_by_field_name("expression"),
                    n.child_by_field_name("pattern"),
                ) else {
                    return Shape::Other;
                };
                let mut equal = true;
                if pattern.kind() == "negated_pattern" {
                    equal = false;
                    match named(pattern).first() {
                        Some(inner) => pattern = *inner,
                        None => return Shape::Other,
                    }
                }
                match (pattern.kind(), named(pattern).first()) {
                    ("constant_pattern", Some(constant)) => Shape::Cmp(value, *constant, equal),
                    _ => Shape::Other,
                }
            }
            "null_literal" => Shape::Lit(Lit::Falsy),
            "boolean_literal" => self.boolean(n),
            "integer_literal" => Shape::Lit(Lit::Num(self.text(n).trim().to_string())),
            "identifier" => Shape::Name(self.text(n).to_string()),
            "member_access_expression" => {
                let Some(owner) = n.child_by_field_name("expression") else {
                    return Shape::Other;
                };
                let member = self.field_text(n, "name");
                if self.text(owner) == "this" {
                    Shape::Name(member.to_string())
                } else if matches!(member, "HasValue" | "Value") {
                    Shape::Inner(owner)
                } else {
                    Shape::Other
                }
            }
            "invocation_expression" => {
                let Some(func) = n.child_by_field_name("function") else {
                    return Shape::Other;
                };
                let first = self.args(n).first().copied();
                match func.kind() {
                    "identifier" if self.text(func) == "nameof" => {
                        first.map_or(Shape::Other, Shape::Inner)
                    }
                    "identifier" => Shape::Call(self.text(func).to_string()),
                    "member_access_expression" => {
                        let Some(owner) = func.child_by_field_name("expression") else {
                            return Shape::Other;
                        };
                        let member = self.field_text(func, "name");
                        match (self.text(owner), member, first) {
                            (
                                "string" | "String",
                                "IsNullOrEmpty" | "IsNullOrWhiteSpace",
                                Some(arg),
                            ) => Shape::Flip(arg),
                            ("bool" | "Boolean" | "Convert", "Parse" | "ToBoolean", Some(arg)) => {
                                Shape::Inner(arg)
                            }
                            ("this", _, _) => Shape::Call(member.to_string()),
                            (_, "Equals", Some(arg)) => Shape::Cmp(owner, arg, true),
                            (
                                _,
                                "ToLower" | "ToLowerInvariant" | "ToUpper" | "ToUpperInvariant"
                                | "Trim" | "ToString",
                                _,
                            ) => Shape::Inner(owner),
                            _ => Shape::Other,
                        }
                    }
                    _ => Shape::Other,
                }
            }
            _ => Shape::Other,
        }
    }

    fn shape_ruby(&self, n: Node<'t>) -> Shape<'t> {
        match n.kind() {
            "parenthesized_statements" => self.only_child(n),
            "unary" => self.negation(n, n.child_by_field_name("operand")),
            "binary" => self.binary(n),
            "nil" | "false" => Shape::Lit(Lit::Falsy),
            "true" => Shape::Lit(Lit::Truthy),
            // Every number is true in Ruby.
            "integer" => Shape::Lit(Lit::Num(self.text(n).trim().to_string())),
            "identifier" | "constant" | "instance_variable" | "global_variable"
            | "class_variable" => Shape::Name(self.text(n).to_string()),
            "call" => {
                let method = self.field_text(n, "method");
                let args = self.args(n);
                match n.child_by_field_name("receiver") {
                    None => {
                        if args.is_empty() {
                            Shape::Name(method.to_string())
                        } else {
                            Shape::Call(method.to_string())
                        }
                    }
                    Some(receiver) if self.text(receiver) == "self" => {
                        Shape::Call(method.to_string())
                    }
                    Some(receiver) => match (method, args.first()) {
                        ("nil?" | "empty?" | "blank?", _) => Shape::Flip(receiver),
                        (
                            "present?" | "to_s" | "downcase" | "upcase" | "strip" | "freeze"
                            | "dup" | "itself",
                            _,
                        ) => Shape::Inner(receiver),
                        ("eql?" | "equal?" | "casecmp?", Some(arg)) => {
                            Shape::Cmp(receiver, *arg, true)
                        }
                        _ => Shape::Other,
                    },
                }
            }
            _ => Shape::Other,
        }
    }

    fn shape_php(&self, n: Node<'t>) -> Shape<'t> {
        match n.kind() {
            "parenthesized_expression" | "argument" => self.last_child(n),
            "cast_expression" => n
                .child_by_field_name("value")
                .map_or(Shape::Other, Shape::Inner),
            "unary_op_expression" => self.negation(n, n.child_by_field_name("argument")),
            "binary_expression" => self.binary(n),
            "boolean" => self.boolean(n),
            "null" => Shape::Lit(Lit::Falsy),
            "integer" => Shape::Lit(Lit::Num(self.text(n).trim().to_string())),
            "variable_name" | "name" => Shape::Name(self.text(n).to_string()),
            "function_call_expression" => {
                let Some(func) = n.child_by_field_name("function") else {
                    return Shape::Other;
                };
                let name = match func.kind() {
                    "name" => self.text(func),
                    "qualified_name" => named(func).last().map(|l| self.text(*l)).unwrap_or(""),
                    _ => return Shape::Other,
                };
                let first = self.args(n).first().copied();
                match (name.to_ascii_lowercase().as_str(), first) {
                    ("empty" | "is_null", Some(arg)) => Shape::Flip(arg),
                    (
                        "isset" | "boolval" | "strval" | "strtolower" | "strtoupper" | "trim"
                        | "filter_var",
                        Some(arg),
                    ) => Shape::Inner(arg),
                    _ => Shape::Call(name.to_string()),
                }
            }
            "member_call_expression" if self.field_text(n, "object") == "$this" => {
                Shape::Call(self.field_text(n, "name").to_string())
            }
            "scoped_call_expression"
                if matches!(self.field_text(n, "scope"), "self" | "static") =>
            {
                Shape::Call(self.field_text(n, "name").to_string())
            }
            "class_constant_access_expression" => match named(n).as_slice() {
                [scope, name] if matches!(self.text(*scope), "self" | "static") => {
                    Shape::Name(self.text(*name).to_string())
                }
                _ => Shape::Other,
            },
            _ => Shape::Other,
        }
    }

    fn shape_swift(&self, n: Node<'t>) -> Shape<'t> {
        match n.kind() {
            "tuple_expression" => self.only_child(n),
            "value_argument" => n
                .child_by_field_name("value")
                .map_or(Shape::Other, Shape::Inner),
            "try_expression" | "await_expression" => self.last_child(n),
            "prefix_expression" => match (
                n.child_by_field_name("operation"),
                n.child_by_field_name("target"),
            ) {
                (Some(op), Some(target)) if self.text(op).trim() == "!" => Shape::Not(target),
                _ => Shape::Other,
            },
            "infix_expression"
            | "equality_expression"
            | "conjunction_expression"
            | "disjunction_expression"
            | "nil_coalescing_expression" => self.binary(n),
            "nil" => Shape::Lit(Lit::Falsy),
            "boolean_literal" => self.boolean(n),
            "integer_literal" => Shape::Lit(Lit::Num(self.text(n).trim().to_string())),
            "simple_identifier" => Shape::Name(self.text(n).to_string()),
            "navigation_expression" => {
                let Some(target) = n.child_by_field_name("target") else {
                    return Shape::Other;
                };
                let member = self.field_text(n, "suffix").trim_start_matches('.');
                if self.text(target) == "self" || self.text(target) == "Self" {
                    Shape::Name(member.to_string())
                } else if member == "isEmpty" {
                    Shape::Flip(target)
                } else {
                    Shape::Other
                }
            }
            "call_expression" => match named(n).first() {
                Some(callee) if callee.kind() == "simple_identifier" => {
                    Shape::Call(self.text(*callee).to_string())
                }
                Some(callee) if callee.kind() == "navigation_expression" => {
                    let member = self.field_text(*callee, "suffix").trim_start_matches('.');
                    match callee.child_by_field_name("target") {
                        Some(target) if matches!(self.text(target), "self" | "Self") => {
                            Shape::Call(member.to_string())
                        }
                        Some(target) if matches!(member, "lowercased" | "uppercased") => {
                            Shape::Inner(target)
                        }
                        _ => Shape::Other,
                    }
                }
                _ => Shape::Other,
            },
            _ => Shape::Other,
        }
    }

    fn shape_scala(&self, n: Node<'t>) -> Shape<'t> {
        match n.kind() {
            "parenthesized_expression" => self.only_child(n),
            "prefix_expression" => self.negation(n, None),
            "infix_expression" => self.binary(n),
            "null_literal" => Shape::Lit(Lit::Falsy),
            "boolean_literal" => self.boolean(n),
            "integer_literal" => Shape::Lit(Lit::Num(self.text(n).trim().to_string())),
            "identifier" => Shape::Name(self.text(n).to_string()),
            "field_expression" => {
                let Some(owner) = n.child_by_field_name("value") else {
                    return Shape::Other;
                };
                match self.field_text(n, "field") {
                    member if self.text(owner) == "this" => Shape::Name(member.to_string()),
                    "isDefined" | "nonEmpty" | "isPresent" | "get" | "toLowerCase"
                    | "toUpperCase" | "trim" | "toBoolean" | "orNull" => Shape::Inner(owner),
                    "isEmpty" => Shape::Flip(owner),
                    _ => Shape::Other,
                }
            }
            "call_expression" => {
                let Some(func) = n.child_by_field_name("function") else {
                    return Shape::Other;
                };
                let first = self.args(n).first().copied();
                match func.kind() {
                    "identifier" => Shape::Call(self.text(func).to_string()),
                    "field_expression" => {
                        let Some(owner) = func.child_by_field_name("value") else {
                            return Shape::Other;
                        };
                        match (self.field_text(func, "field"), first) {
                            (member, _) if self.text(owner) == "this" => {
                                Shape::Call(member.to_string())
                            }
                            ("contains" | "equals" | "equalsIgnoreCase", Some(arg)) => {
                                Shape::Cmp(owner, arg, true)
                            }
                            ("toLowerCase" | "toUpperCase" | "trim" | "get", _) => {
                                Shape::Inner(owner)
                            }
                            _ => Shape::Other,
                        }
                    }
                    _ => Shape::Other,
                }
            }
            _ => Shape::Other,
        }
    }

    fn shape_c(&self, n: Node<'t>) -> Shape<'t> {
        match n.kind() {
            "parenthesized_expression" => self.only_child(n),
            "condition_clause" => n
                .child_by_field_name("value")
                .map_or(Shape::Other, Shape::Inner),
            "cast_expression" => n
                .child_by_field_name("value")
                .map_or(Shape::Other, Shape::Inner),
            "unary_expression" => self.negation(n, n.child_by_field_name("argument")),
            "preproc_defined" => named(n).first().map_or(Shape::Other, |c| Shape::Inner(*c)),
            "binary_expression" => self.binary(n),
            "null" | "nullptr" | "false" => Shape::Lit(Lit::Falsy),
            "true" => Shape::Lit(Lit::Truthy),
            "number_literal" => Shape::Lit(Lit::Num(self.text(n).trim().to_string())),
            "identifier" => match self.text(n) {
                "NULL" | "nil" | "Nil" | "NO" => Shape::Lit(Lit::Falsy),
                "YES" => Shape::Lit(Lit::Truthy),
                name => Shape::Name(name.to_string()),
            },
            "call_expression" => match n.child_by_field_name("function") {
                Some(func) if func.kind() == "identifier" => {
                    Shape::Call(self.text(func).to_string())
                }
                _ => Shape::Other,
            },
            "message_expression" => {
                let Some(receiver) = n.child_by_field_name("receiver") else {
                    return Shape::Other;
                };
                let method = self.field_text(n, "method");
                let args = self.args(n);
                match (method, args.first()) {
                    (_, None) if self.text(receiver) == "self" => Shape::Name(method.to_string()),
                    ("isEqualToString" | "isEqual", Some(arg)) => Shape::Cmp(receiver, *arg, true),
                    ("boolValue" | "length" | "lowercaseString" | "uppercaseString", None) => {
                        Shape::Inner(receiver)
                    }
                    _ => Shape::Other,
                }
            }
            "field_expression" if self.g == Grammar::ObjC => {
                let Some(owner) = n.child_by_field_name("argument") else {
                    return Shape::Other;
                };
                match self.field_text(n, "field") {
                    member if self.text(owner) == "self" => Shape::Name(member.to_string()),
                    "boolValue" | "length" | "lowercaseString" | "uppercaseString" => {
                        Shape::Inner(owner)
                    }
                    _ => Shape::Other,
                }
            }
            _ => Shape::Other,
        }
    }

    /// `Some(true)` when comparing equal to `n` means the variable is unset or off.
    fn literal_is_falsy(&self, n: Node<'t>) -> Option<bool> {
        match self.shape(n) {
            Shape::Lit(Lit::Falsy) => Some(true),
            Shape::Lit(Lit::Truthy) => Some(false),
            Shape::Lit(Lit::Str(s)) => Some(matches!(
                s.to_ascii_lowercase().as_str(),
                "" | "0" | "false" | "no" | "off"
            )),
            Shape::Lit(Lit::Num(s)) => Some(s == "0"),
            Shape::Inner(inner) => self.literal_is_falsy(inner),
            _ => None,
        }
    }

    fn undecided(&self, n: Node<'t>, locals: &Binds<'t>, depth: usize) -> Val {
        let mut vars = BTreeSet::new();
        for child in named(n) {
            if self.is_function(child) {
                continue;
            }
            if let Some(c) = self.eval(child, locals, depth + 1) {
                vars.extend(c.vars);
            }
        }
        if vars.is_empty() {
            None
        } else {
            Some(Ci {
                truth: Truth::Mixed,
                vars,
            })
        }
    }

    fn bound<'b>(&'b self, name: &str, locals: &'b Binds<'t>) -> Option<&'b Vec<Bind<'t>>> {
        locals.get(name).or_else(|| self.module.get(name))
    }

    fn functions_named(&self, name: &str, locals: &Binds<'t>) -> Vec<Node<'t>> {
        self.bound(name, locals)
            .map(|binds| {
                binds
                    .iter()
                    .filter_map(|b| match b {
                        Bind::Func(f) => Some(*f),
                        Bind::Value(_) => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether `e` is a call or a member this file does not define.
    fn unresolved(&self, e: Node<'t>, locals: &Binds<'t>) -> bool {
        match self.shape(e) {
            Shape::Other => true,
            Shape::Call(name) => self.functions_named(&name, locals).is_empty(),
            Shape::Inner(inner) | Shape::Not(inner) | Shape::Flip(inner) => {
                self.unresolved(inner, locals)
            }
            _ => false,
        }
    }

    fn eval_name(&self, name: &str, locals: &Binds<'t>, depth: usize) -> Val {
        let spelled = name.trim_start_matches(['$', '@']);
        match self.bound(name, locals) {
            Some(binds) => combine(
                binds
                    .iter()
                    .map(|b| match b {
                        Bind::Func(f) => self.eval_function(*f, depth + 1),
                        Bind::Value(e) => {
                            let value = self.eval(*e, locals, depth + 1);
                            match exact_mention(spelled) {
                                Some(var) if value.is_none() && self.unresolved(*e, locals) => {
                                    one(Truth::Mixed, var)
                                }
                                _ => value,
                            }
                        }
                    })
                    .collect(),
            ),
            None => mention(spelled).and_then(|var| one(Truth::InCi, var)),
        }
    }

    fn eval_function(&self, func: Node<'t>, depth: usize) -> Val {
        if depth > MAX_DEPTH {
            return None;
        }
        let mut locals = Binds::new();
        self.collect(func, &mut locals, false, &mut HashSet::new());
        let returns = self.returns(func);
        let decided = combine(
            returns
                .iter()
                .map(|r| self.eval(*r, &locals, depth + 1))
                .collect(),
        );
        decided.or_else(|| self.undecided(func, &locals, depth + 1))
    }

    fn eval(&self, n: Node<'t>, locals: &Binds<'t>, depth: usize) -> Val {
        if depth > MAX_DEPTH {
            return None;
        }
        match self.shape(n) {
            Shape::Comment => None,
            Shape::Env(var) => {
                if is_ci_env_read_name(&var) {
                    one(Truth::InCi, &var)
                } else {
                    None
                }
            }
            Shape::Not(inner) | Shape::Flip(inner) => flip(self.eval(inner, locals, depth + 1)),
            Shape::Inner(inner) => self.eval(inner, locals, depth + 1),
            Shape::And(left, right) | Shape::Or(left, right) => and(
                self.eval(left, locals, depth + 1),
                self.eval(right, locals, depth + 1),
            ),
            Shape::Cmp(left, right, equal) => {
                let (value, falsy) = if let Some(f) = self.literal_is_falsy(right) {
                    (self.eval(left, locals, depth + 1), f)
                } else if let Some(f) = self.literal_is_falsy(left) {
                    (self.eval(right, locals, depth + 1), f)
                } else {
                    return self.undecided(n, locals, depth);
                };
                // Nothing decided on the other side: a CI variable named anywhere in
                // the comparison (`props.contains("CI")`) involves it undecidedly.
                if value.is_none() {
                    return self.undecided(n, locals, depth);
                }
                if equal == falsy {
                    flip(value)
                } else {
                    value
                }
            }
            Shape::Lit(Lit::Str(s)) => mention(&s).and_then(|var| one(Truth::InCi, var)),
            Shape::Lit(_) => None,
            Shape::Name(name) => self.eval_name(&name, locals, depth),
            Shape::Call(name) => {
                let functions = self.functions_named(&name, locals);
                if functions.is_empty() {
                    self.undecided(n, locals, depth)
                } else {
                    combine(
                        functions
                            .iter()
                            .map(|f| self.eval_function(*f, depth + 1))
                            .collect(),
                    )
                }
            }
            Shape::Other => self.undecided(n, locals, depth),
        }
    }

    /// The value of a condition that is a constant whatever the environment.
    fn constant(&self, n: Node<'t>, locals: &Binds<'t>, depth: usize) -> Option<bool> {
        if depth > MAX_DEPTH {
            return None;
        }
        match self.shape(n) {
            Shape::Lit(Lit::Truthy) => Some(true),
            Shape::Lit(Lit::Falsy) => Some(false),
            Shape::Lit(Lit::Num(s)) => {
                let value: f64 = s.parse().ok()?;
                Some(self.g == Grammar::Ruby || value != 0.0)
            }
            Shape::Inner(inner) => self.constant(inner, locals, depth + 1),
            Shape::Not(inner) => self.constant(inner, locals, depth + 1).map(|v| !v),
            Shape::And(left, right) => match (
                self.constant(left, locals, depth + 1),
                self.constant(right, locals, depth + 1),
            ) {
                (Some(false), _) | (_, Some(false)) => Some(false),
                (Some(true), Some(true)) => Some(true),
                _ => None,
            },
            Shape::Or(left, right) => match (
                self.constant(left, locals, depth + 1),
                self.constant(right, locals, depth + 1),
            ) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (Some(false), Some(false)) => Some(false),
                _ => None,
            },
            Shape::Cmp(left, right, equal) => {
                let scalar = |n: Node<'t>| match self.shape(n) {
                    Shape::Lit(Lit::Str(s)) => Some(("string", s)),
                    Shape::Lit(Lit::Num(s)) => Some(("number", s)),
                    _ => None,
                };
                let (left, right) = (scalar(left)?, scalar(right)?);
                if left.0 != right.0 {
                    return None;
                }
                Some((left.1 == right.1) == equal)
            }
            Shape::Name(name) => match self.bound(&name, locals)?.as_slice() {
                [Bind::Value(e)] => self.constant(*e, locals, depth + 1),
                _ => None,
            },
            _ => None,
        }
    }

    fn is_function(&self, n: Node<'t>) -> bool {
        let kind = n.kind();
        match self.g {
            Grammar::CSharp => matches!(
                kind,
                "method_declaration"
                    | "local_function_statement"
                    | "lambda_expression"
                    | "anonymous_method_expression"
                    | "constructor_declaration"
            ),
            Grammar::Ruby => matches!(
                kind,
                "method" | "singleton_method" | "do_block" | "block" | "lambda"
            ),
            Grammar::Php => matches!(
                kind,
                "method_declaration"
                    | "function_definition"
                    | "anonymous_function"
                    | "anonymous_function_creation_expression"
                    | "arrow_function"
            ),
            Grammar::Swift => matches!(
                kind,
                "function_declaration" | "lambda_literal" | "init_declaration"
            ),
            Grammar::Scala => matches!(kind, "function_definition" | "lambda_expression"),
            Grammar::C | Grammar::ObjC => matches!(
                kind,
                "function_definition" | "lambda_expression" | "method_definition" | "block_literal"
            ),
        }
    }

    fn function_name(&self, func: Node<'t>) -> Option<String> {
        if let Some(name) = func.child_by_field_name("name") {
            return Some(self.text(name).to_string());
        }
        match func.kind() {
            "function_definition" => {
                let mut cur = func.child_by_field_name("declarator")?;
                while !matches!(cur.kind(), "identifier" | "field_identifier") {
                    cur = cur
                        .child_by_field_name("declarator")
                        .or_else(|| cur.child_by_field_name("name"))?;
                }
                Some(self.text(cur).to_string())
            }
            "method_definition" => named(func)
                .into_iter()
                .find(|c| c.kind() == "identifier")
                .map(|c| self.text(c).to_string()),
            _ => None,
        }
    }

    /// The expressions a function can return.
    fn returns(&self, func: Node<'t>) -> Vec<Node<'t>> {
        let mut out = Vec::new();
        let mut stack = named(func);
        while let Some(n) = stack.pop() {
            if self.is_function(n) {
                continue;
            }
            match n.kind() {
                "return_statement" | "return_expression" | "return" => {
                    if let Some(mut value) = named(n).first().copied() {
                        if value.kind() == "argument_list" {
                            value = named(value).first().copied().unwrap_or(value);
                        }
                        out.push(value);
                    }
                    continue;
                }
                "control_transfer_statement"
                    if n.child(0).is_some_and(|c| self.text(c) == "return") =>
                {
                    out.extend(named(n).last());
                    continue;
                }
                "arrow_expression_clause" => {
                    out.extend(named(n).first());
                    continue;
                }
                _ => {}
            }
            stack.extend(named(n));
        }
        // The value of a body that ends in an expression.
        let last_of = |block: Node<'t>| {
            named(block).last().copied().filter(|l| {
                !matches!(
                    l.kind(),
                    "return" | "return_expression" | "control_transfer_statement"
                )
            })
        };
        match self.g {
            Grammar::Ruby => {
                let body = func.child_by_field_name("body").unwrap_or(func);
                out.extend(last_of(body));
            }
            Grammar::Scala => {
                if let Some(body) = func.child_by_field_name("body") {
                    if body.kind() == "block" {
                        out.extend(last_of(body));
                    } else {
                        out.push(body);
                    }
                }
            }
            Grammar::Swift => {
                let mut stack = named(func);
                while let Some(n) = stack.pop() {
                    if n.kind() == "statements" {
                        if let [only] = named(n).as_slice() {
                            out.extend(last_of(n).filter(|l| l.id() == only.id()));
                        }
                        continue;
                    }
                    if matches!(n.kind(), "function_body" | "computed_property") {
                        stack.extend(named(n));
                    }
                }
            }
            Grammar::Php if func.kind() == "arrow_function" => {
                out.extend(func.child_by_field_name("body"));
            }
            _ => {}
        }
        out
    }

    /// A name bound to a value or a function by `n`.
    fn binding(&self, n: Node<'t>) -> Option<(String, Bind<'t>)> {
        let value_of = |name: Option<Node<'t>>, value: Option<Node<'t>>| {
            Some((self.text(name?).to_string(), Bind::Value(value?)))
        };
        match (self.g, n.kind()) {
            (Grammar::CSharp, "variable_declarator") => {
                let name = n.child_by_field_name("name")?;
                let value = named(n)
                    .into_iter()
                    .rfind(|c| c.id() != name.id() && c.kind() != "bracketed_argument_list");
                value_of(Some(name), value)
            }
            (Grammar::CSharp, "property_declaration") => {
                let name = self.text(n.child_by_field_name("name")?).to_string();
                match n.child_by_field_name("value") {
                    Some(value) => Some((name, Bind::Value(value))),
                    None => Some((name, Bind::Func(n))),
                }
            }
            (Grammar::CSharp | Grammar::C | Grammar::ObjC, "assignment_expression")
            | (Grammar::Ruby, "assignment")
            | (Grammar::Php, "assignment_expression") => {
                let left = n.child_by_field_name("left")?;
                if !matches!(
                    left.kind(),
                    "identifier"
                        | "constant"
                        | "instance_variable"
                        | "global_variable"
                        | "class_variable"
                        | "variable_name"
                ) {
                    return None;
                }
                value_of(Some(left), n.child_by_field_name("right"))
            }
            (Grammar::Php, "const_element") => {
                let parts = named(n);
                match parts.as_slice() {
                    [name, .., value] => value_of(Some(*name), Some(*value)),
                    _ => None,
                }
            }
            (Grammar::Swift, "property_declaration") => {
                let name = self.text(n.child_by_field_name("name")?).trim().to_string();
                match n.child_by_field_name("value") {
                    Some(value) => Some((name, Bind::Value(value))),
                    None => n
                        .child_by_field_name("computed_value")
                        .map(|_| (name, Bind::Func(n))),
                }
            }
            (Grammar::Scala, "val_definition" | "var_definition") => {
                let pattern = n.child_by_field_name("pattern")?;
                if pattern.kind() != "identifier" {
                    return None;
                }
                value_of(Some(pattern), n.child_by_field_name("value"))
            }
            (Grammar::C | Grammar::ObjC, "init_declarator") => {
                let mut name = n.child_by_field_name("declarator")?;
                while name.kind() != "identifier" {
                    name = name.child_by_field_name("declarator")?;
                }
                value_of(Some(name), n.child_by_field_name("value"))
            }
            _ => None,
        }
    }

    /// Whether the declarations of a file are looked for under a node of this kind.
    fn module_descends(&self, kind: &str) -> bool {
        match self.g {
            Grammar::CSharp => matches!(
                kind,
                "namespace_declaration"
                    | "file_scoped_namespace_declaration"
                    | "declaration_list"
                    | "class_declaration"
                    | "struct_declaration"
                    | "record_declaration"
                    | "field_declaration"
                    | "variable_declaration"
                    | "global_statement"
                    | "local_declaration_statement"
            ),
            Grammar::Ruby => matches!(kind, "class" | "module" | "body_statement"),
            Grammar::Php => matches!(
                kind,
                "class_declaration"
                    | "trait_declaration"
                    | "declaration_list"
                    | "namespace_definition"
                    | "compound_statement"
                    | "expression_statement"
                    | "const_declaration"
            ),
            Grammar::Swift => matches!(kind, "class_declaration" | "class_body"),
            Grammar::Scala => matches!(
                kind,
                "class_definition"
                    | "object_definition"
                    | "trait_definition"
                    | "template_body"
                    | "package_clause"
            ),
            Grammar::C | Grammar::ObjC => matches!(
                kind,
                "declaration"
                    | "namespace_definition"
                    | "declaration_list"
                    | "linkage_specification"
                    | "expression_statement"
                    | "class_implementation"
                    | "implementation_definition"
            ),
        }
    }

    fn collect(
        &self,
        scope: Node<'t>,
        out: &mut Binds<'t>,
        module: bool,
        seen: &mut HashSet<usize>,
    ) {
        for child in named(scope) {
            if self.is_function(child) {
                if let Some(name) = self.function_name(child) {
                    if seen.insert(child.id()) {
                        out.entry(name).or_default().push(Bind::Func(child));
                    }
                }
                continue;
            }
            if let Some((name, bind)) = self.binding(child) {
                if seen.insert(child.id()) {
                    out.entry(name).or_default().push(bind);
                }
            }
            if !module || self.module_descends(child.kind()) {
                self.collect(child, out, module, seen);
            }
        }
    }

    fn swift_directive_guards(&self, parent: Node<'t>, cur: Node<'t>) -> Vec<Guard<'t>> {
        if self.g != Grammar::Swift || parent.kind() != "statements" {
            return Vec::new();
        }
        let cur_start = cur.start_byte();
        let mut cursor = parent.walk();
        let has_closing = parent.children(&mut cursor).any(|c| {
            c.kind() == "directive"
                && c.start_byte() >= cur.end_byte()
                && self.text(c).trim().starts_with("#endif")
        });
        if !has_closing {
            return Vec::new();
        }

        struct Layer<'a> {
            earlier_conds: Vec<Node<'a>>,
            active: Option<(Node<'a>, bool)>,
        }
        let mut stack: Vec<Layer<'t>> = Vec::new();

        let mut cursor = parent.walk();
        for child in parent.children(&mut cursor) {
            if child.start_byte() >= cur_start {
                break;
            }
            if child.kind() != "directive" {
                continue;
            }
            let text = self.text(child).trim();
            let is_negated = child
                .children(&mut child.walk())
                .any(|c| c.kind() == "!" || self.text(c).trim() == "!");
            if text.starts_with("#if") {
                let cond = named(child).first().copied();
                stack.push(Layer {
                    earlier_conds: Vec::new(),
                    active: cond.map(|c| (c, is_negated)),
                });
            } else if text.starts_with("#elseif") {
                if let Some(top) = stack.last_mut() {
                    if let Some((prev, _)) = top.active.take() {
                        top.earlier_conds.push(prev);
                    }
                    let cond = named(child).first().copied();
                    top.active = cond.map(|c| (c, is_negated));
                }
            } else if text.starts_with("#else") {
                if let Some(top) = stack.last_mut() {
                    if let Some((prev, _)) = top.active.take() {
                        top.earlier_conds.push(prev);
                    }
                    top.active = None;
                }
            } else if text.starts_with("#endif") {
                stack.pop();
            }
        }

        let mut guards = Vec::new();
        for layer in stack {
            for earlier in layer.earlier_conds {
                guards.push(Guard {
                    conds: vec![earlier],
                    negated: true,
                });
            }
            if let Some((active, negated)) = layer.active {
                guards.push(Guard {
                    conds: vec![active],
                    negated,
                });
            }
        }
        guards
    }

    fn preproc_guards(&self, parent: Node<'t>, cur: Node<'t>) -> Vec<Guard<'t>> {
        let kind = parent.kind();
        let one = |cond: Node<'t>, negated: bool| {
            vec![Guard {
                conds: vec![cond],
                negated,
            }]
        };
        match kind {
            "preproc_if" | "preproc_elif" => {
                let Some(cond) = parent.child_by_field_name("condition") else {
                    return Vec::new();
                };
                if cond.id() == cur.id() {
                    return Vec::new();
                }
                let in_alt = named(parent).iter().any(|c| {
                    matches!(c.kind(), "preproc_elif" | "preproc_else")
                        && (c.id() == cur.id() || cur.start_byte() >= c.start_byte())
                });
                one(cond, in_alt)
            }
            "preproc_ifdef" | "preproc_ifndef" => {
                let Some(name) = parent.child_by_field_name("name") else {
                    return Vec::new();
                };
                if name.id() == cur.id() {
                    return Vec::new();
                }
                let is_ifndef = kind == "preproc_ifndef"
                    || parent
                        .child(0)
                        .is_some_and(|c| self.text(c).trim() == "#ifndef");
                let in_alt = named(parent).iter().any(|c| {
                    matches!(c.kind(), "preproc_elif" | "preproc_else")
                        && (c.id() == cur.id() || cur.start_byte() >= c.start_byte())
                });
                one(name, if is_ifndef { !in_alt } else { in_alt })
            }
            _ => Vec::new(),
        }
    }

    /// The conditions `parent` puts on its child `cur`.
    fn guards(&self, parent: Node<'t>, cur: Node<'t>) -> Vec<Guard<'t>> {
        let one = |cond: Node<'t>, negated: bool| {
            vec![Guard {
                conds: vec![cond],
                negated,
            }]
        };
        let kind = parent.kind();
        // Swift directives in `statements`
        if self.g == Grammar::Swift && kind == "statements" {
            let swift_guards = self.swift_directive_guards(parent, cur);
            if !swift_guards.is_empty() {
                return swift_guards;
            }
        }
        // Preprocessor conditionals in C, ObjC, C#
        if matches!(self.g, Grammar::C | Grammar::ObjC | Grammar::CSharp) {
            let preproc = self.preproc_guards(parent, cur);
            if !preproc.is_empty() {
                return preproc;
            }
        }
        // Swift: the conditions carry no branch field; the branches stand before and
        // after the `else` keyword, and the body of a `guard` after its `else`.
        if self.g == Grammar::Swift && matches!(kind, "if_statement" | "guard_statement") {
            let mut cursor = parent.walk();
            let conds: Vec<Node<'t>> = parent
                .children_by_field_name("condition", &mut cursor)
                .filter(|c| c.is_named() && c.kind() != "value_binding_pattern")
                .collect();
            if conds.is_empty() || conds.iter().any(|c| c.id() == cur.id()) {
                return Vec::new();
            }
            let mut cursor = parent.walk();
            let else_at = parent
                .children(&mut cursor)
                .find(|c| c.kind() == "else")
                .map(|c| c.start_byte());
            let after_else = else_at.is_some_and(|at| cur.start_byte() >= at);
            let in_branch = matches!(cur.kind(), "statements" | "if_statement");
            if !in_branch {
                return Vec::new();
            }
            return vec![Guard {
                conds,
                negated: after_else,
            }];
        }
        if matches!(self.g, Grammar::Ruby | Grammar::Php)
            && matches!(kind, "binary" | "binary_expression")
        {
            if let Shape::And(left, right) | Shape::Or(left, right) = self.shape(parent) {
                if right.id() == cur.id() {
                    return one(left, matches!(self.shape(parent), Shape::Or(..)));
                }
            }
            return Vec::new();
        }

        // Loops: while, until, do-while, for, foreach
        let loop_guard = match self.g {
            Grammar::C | Grammar::ObjC => match kind {
                "while_statement" | "do_statement" => {
                    parent.child_by_field_name("condition").map(|c| (c, false))
                }
                "for_statement" => parent.child_by_field_name("condition").map(|c| (c, false)),
                "for_range_loop" => parent.child_by_field_name("right").map(|c| (c, false)),
                _ => None,
            },
            Grammar::CSharp => match kind {
                "while_statement" | "do_statement" => {
                    parent.child_by_field_name("condition").map(|c| (c, false))
                }
                "for_statement" => parent.child_by_field_name("condition").map(|c| (c, false)),
                "foreach_statement" => parent.child_by_field_name("right").map(|c| (c, false)),
                _ => None,
            },
            Grammar::Ruby => match kind {
                "while" | "while_modifier" => {
                    parent.child_by_field_name("condition").map(|c| (c, false))
                }
                "until" | "until_modifier" => {
                    parent.child_by_field_name("condition").map(|c| (c, true))
                }
                "for" => parent
                    .child_by_field_name("value")
                    .map(|v| (named(v).last().copied().unwrap_or(v), false)),
                _ => None,
            },
            Grammar::Php => match kind {
                "while_statement" | "do_statement" => {
                    parent.child_by_field_name("condition").map(|c| (c, false))
                }
                "for_statement" => parent.child_by_field_name("condition").map(|c| (c, false)),
                "foreach_statement" => {
                    let mut cursor = parent.walk();
                    let coll = parent
                        .children(&mut cursor)
                        .take_while(|c| c.kind() != "as")
                        .filter(|c| c.is_named())
                        .last();
                    coll.map(|c| (c, false))
                }
                _ => None,
            },
            Grammar::Swift => match kind {
                "while_statement" | "repeat_while_statement" => {
                    parent.child_by_field_name("condition").map(|c| (c, false))
                }
                "for_statement" => parent.child_by_field_name("collection").map(|c| (c, false)),
                _ => None,
            },
            Grammar::Scala => match kind {
                "while_expression" | "do_while_expression" => {
                    parent.child_by_field_name("condition").map(|c| (c, false))
                }
                "for_expression" => parent
                    .children(&mut parent.walk())
                    .find(|c| c.kind() == "enumerators")
                    .map(|c| (c, false)),
                _ => None,
            },
        };
        if let Some((cond, negated)) = loop_guard {
            if cond.id() != cur.id() {
                return one(cond, negated);
            }
        }

        // Switch / Match / Case
        let switch_guard = match self.g {
            Grammar::C | Grammar::ObjC | Grammar::Php => match kind {
                "switch_statement" | "match_expression" => parent.child_by_field_name("condition"),
                _ => None,
            },
            Grammar::CSharp => match kind {
                "switch_statement" | "switch_expression" => parent.child_by_field_name("value"),
                _ => None,
            },
            Grammar::Swift => match kind {
                "switch_statement" => parent.child_by_field_name("expr"),
                _ => None,
            },
            Grammar::Ruby => match kind {
                "case" => parent.child_by_field_name("value"),
                _ => None,
            },
            Grammar::Scala => match kind {
                "match_expression" => parent.child_by_field_name("value"),
                _ => None,
            },
        };
        if let Some(cond) = switch_guard {
            if cond.id() != cur.id() {
                return one(cond, false);
            }
        }

        let conditional = match self.g {
            Grammar::CSharp => matches!(kind, "if_statement" | "conditional_expression"),
            Grammar::Ruby => matches!(
                kind,
                "if" | "unless" | "elsif" | "if_modifier" | "unless_modifier" | "conditional"
            ),
            Grammar::Php => matches!(
                kind,
                "if_statement" | "else_if_clause" | "conditional_expression"
            ),
            Grammar::Swift => kind == "ternary_expression",
            Grammar::Scala => kind == "if_expression",
            Grammar::C | Grammar::ObjC => matches!(kind, "if_statement" | "conditional_expression"),
        };
        if !conditional {
            return Vec::new();
        }
        let Some(cond) = parent.child_by_field_name("condition") else {
            return Vec::new();
        };
        if cond.id() == cur.id() {
            return Vec::new();
        }
        let unless = matches!(kind, "unless" | "unless_modifier");
        let in_body = ["consequence", "body", "if_true"]
            .iter()
            .any(|f| same(parent.child_by_field_name(f), cur));
        if in_body {
            return one(cond, unless);
        }
        // An `else` or `elseif` branch: the condition failed, and so did that of every
        // `elseif` written before this branch.
        let mut out = one(cond, !unless);
        if self.g == Grammar::Php {
            for sibling in named(parent) {
                if sibling.kind() != "else_if_clause" || sibling.start_byte() >= cur.start_byte() {
                    continue;
                }
                if let Some(earlier) = sibling.child_by_field_name("condition") {
                    out.extend(one(earlier, true));
                }
            }
        }
        out
    }

    fn cond_text(&self, cond: Node<'t>) -> &'s str {
        let inner = match cond.kind() {
            "parenthesized_expression" | "parenthesized_statements" | "condition_clause" => {
                match named(cond).as_slice() {
                    [inner] => *inner,
                    _ => cond,
                }
            }
            _ => cond,
        };
        self.text(inner).trim()
    }

    fn guard_text(&self, guard: &Guard<'t>) -> String {
        let text = guard
            .conds
            .iter()
            .map(|c| self.cond_text(*c))
            .collect::<Vec<_>>()
            .join(", ");
        if guard.negated {
            format!("!({text})")
        } else {
            text
        }
    }
}

fn reader<'t, 's>(
    g: Grammar,
    site: Node<'t>,
    anc: &Ancestry<'t>,
    src: &'s [u8],
) -> (Reader<'t, 's>, Binds<'t>) {
    let mut reader = Reader {
        g,
        src,
        module: Binds::new(),
    };
    let root = anc.root();
    let mut scopes = Vec::new();
    let mut below = site;
    while let Some((scope, _)) = anc.nearest(below, Above::SkipScope, |above, _| {
        reader.is_function(above) || (g == Grammar::Scala && above.kind() == "block")
    }) {
        scopes.push(scope);
        below = scope;
    }
    let mut seen = HashSet::new();
    let mut module = Binds::new();
    reader.collect(root, &mut module, true, &mut seen);
    reader.module = module;
    let mut locals = Binds::new();
    for scope in scopes {
        reader.collect(scope, &mut locals, false, &mut seen);
    }
    (reader, locals)
}

/// What the skip at `site` does: under every condition between it and its function, and
/// under `own`, the condition the skip takes itself. `own` is the condition with whether
/// the test runs when it holds (`XCTSkipUnless(c)`, `assume(c)`) rather than is skipped
/// (`XCTSkipIf(c)`).
///
/// A condition that is a constant is not a condition: a skip all of whose conditions
/// always hold is unconditional, and one under a condition that never holds is no skip.
pub fn read_skip<'t>(
    g: Grammar,
    site: Node<'t>,
    anc: &Ancestry<'t>,
    own: Option<(Node<'t>, bool)>,
    src: &[u8],
) -> SkipRead {
    let (reader, locals) = reader(g, site, anc, src);
    // Innermost first: the condition as reported, its value, and its value as a constant.
    let mut chain: Vec<(String, Val, Option<bool>)> = Vec::new();
    if let Some((cond, runs_when)) = own {
        let cond = reader.unwrap(cond);
        let guard = Guard {
            conds: vec![cond],
            negated: runs_when,
        };
        let value = reader.eval(cond, &locals, 0);
        let constant = reader.constant(cond, &locals, 0);
        chain.push((
            reader.guard_text(&guard),
            if runs_when { flip(value) } else { value },
            constant.map(|c| c != runs_when),
        ));
    }
    let mut cur = site;
    while let Some(parent) = anc.parent(cur) {
        if reader.is_function(parent) {
            break;
        }
        for guard in reader.guards(parent, cur) {
            let value = guard
                .conds
                .iter()
                .fold(None, |acc, c| and(acc, reader.eval(*c, &locals, 0)));
            let constants: Vec<Option<bool>> = guard
                .conds
                .iter()
                .map(|c| reader.constant(*c, &locals, 0))
                .collect();
            let constant = if constants.contains(&Some(false)) {
                Some(false)
            } else if constants.iter().all(|c| *c == Some(true)) {
                Some(true)
            } else {
                None
            };
            chain.push((
                reader.guard_text(&guard),
                if guard.negated { flip(value) } else { value },
                constant.map(|c| c != guard.negated),
            ));
        }
        cur = parent;
    }
    if chain
        .iter()
        .any(|(_, _, constant)| *constant == Some(false))
    {
        return SkipRead {
            text: String::new(),
            outcome: SkipCondition::Never,
        };
    }
    chain.retain(|(_, _, constant)| constant.is_none());
    let Some((nearest_text, nearest_value, _)) = chain.first().cloned() else {
        return SkipRead {
            text: String::new(),
            outcome: SkipCondition::Always,
        };
    };
    let whole = chain
        .iter()
        .fold(None, |acc, (_, value, _)| and(acc, value.clone()));
    let verdict = verdict_of(&whole);
    let nearest_decides = matches!(verdict_of(&nearest_value), CiVerdict::Skips(_));
    let text = if matches!(verdict, CiVerdict::Skips(_)) && !nearest_decides {
        chain
            .iter()
            .rev()
            .map(|(text, _, _)| text.as_str())
            .collect::<Vec<_>>()
            .join(" && ")
    } else {
        nearest_text
    };
    SkipRead {
        text,
        outcome: SkipCondition::When(verdict),
    }
}

/// Each case reads one test through its language pack: whether it carries an
/// unconditional skip, and the condition and verdict of a conditional one.
#[cfg(all(
    test,
    feature = "lang-csharp",
    feature = "lang-ruby",
    feature = "lang-php",
    feature = "lang-swift",
    feature = "lang-scala",
    feature = "lang-c",
    feature = "lang-cpp",
    feature = "lang-objc"
))]
mod tests {
    use crate::ast::ci_condition::CiVerdict;
    use crate::ast::{default_registry, AssertVocabulary};

    /// `(ignored, condition, whether a CI variable makes the test skip)`.
    type Read = (bool, Option<String>, bool);

    fn read(path: &str, src: &str) -> Read {
        let registry = default_registry();
        let pack = registry.find_pack(path).expect("a pack for the path");
        let facts = pack
            .extract(path, src, &AssertVocabulary::default())
            .expect("the file parses");
        assert_eq!(facts.tests.len(), 1, "{src}");
        let test = &facts.tests[0];
        (
            test.ignored,
            test.conditional_ignore.clone(),
            matches!(test.ci_verdict, Some(CiVerdict::Skips(_))),
        )
    }

    fn conditional(text: &str, ci: bool) -> Read {
        (false, Some(text.to_string()), ci)
    }

    const UNCONDITIONAL: Read = (true, None, false);
    const RUNS: Read = (false, None, false);

    fn cs(body: &str) -> Read {
        read(
            "tests/QTests.cs",
            &format!("public class QTests {{\n    [Test]\n    public void Adds() {{\n        {body}\n        Assert.AreEqual(2, 1 + 1);\n    }}\n}}\n"),
        )
    }

    #[test]
    fn csharp_reads_the_condition_around_a_skip_and_the_one_it_takes() {
        let env = "Environment.GetEnvironmentVariable(\"CI\")";
        assert_eq!(
            cs(&format!("if ({env} != null) {{ Assert.Ignore(); }}")),
            conditional(&format!("{env} != null"), true)
        );
        assert_eq!(
            cs(&format!("if ({env} == null) {{ Assert.Ignore(); }}")),
            conditional(&format!("{env} == null"), false)
        );
        assert_eq!(
            cs(&format!(
                "if ({env} == null) {{ Run(); }} else {{ Assert.Ignore(); }}"
            )),
            conditional(&format!("!({env} == null)"), true)
        );
        assert_eq!(
            cs(&format!("Assume.That({env} == null);")),
            conditional(&format!("!({env} == null)"), true)
        );
        assert_eq!(
            cs("if (OperatingSystem.IsWindows()) { if (Slow()) { Assert.Ignore(); } }"),
            conditional("Slow()", false)
        );
        // An outer condition on CI is what makes the skip CI-conditional: both are given.
        assert_eq!(
            cs(&format!(
                "if ({env} != null) {{ if (Slow()) {{ Assert.Ignore(); }} }}"
            )),
            conditional(&format!("{env} != null && Slow()"), true)
        );
        assert_eq!(cs("Assert.Ignore();"), UNCONDITIONAL);
        assert_eq!(cs("Skip.If(false);"), RUNS);
    }

    fn rb(body: &str) -> Read {
        read(
            "test/q_test.rb",
            &format!("class QTest < Minitest::Test\n  def test_adds\n    {body}\n    assert_equal 2, 1 + 1\n  end\nend\n"),
        )
    }

    #[test]
    fn ruby_reads_modifiers_unless_and_short_circuits() {
        assert_eq!(rb("skip if ENV[\"CI\"]"), conditional("ENV[\"CI\"]", true));
        assert_eq!(
            rb("skip unless ENV[\"CI\"]"),
            conditional("!(ENV[\"CI\"])", false)
        );
        assert_eq!(
            rb("ENV[\"CI\"] || skip"),
            conditional("!(ENV[\"CI\"])", false)
        );
        assert_eq!(
            rb("skip if ENV[\"CI\"].to_s.empty?"),
            conditional("ENV[\"CI\"].to_s.empty?", false)
        );
        assert_eq!(
            rb("skip if RUBY_ENGINE == \"jruby\""),
            conditional("RUBY_ENGINE == \"jruby\"", false)
        );
        assert_eq!(rb("skip"), UNCONDITIONAL);
    }

    fn php(body: &str) -> Read {
        read(
            "tests/QTest.php",
            &format!("<?php\nclass QTest extends TestCase {{\n    public function testAdds(): void {{\n        {body}\n        $this->assertSame(2, 1 + 1);\n    }}\n}}\n"),
        )
    }

    #[test]
    fn php_reads_getenv_and_the_superglobals() {
        assert_eq!(
            php("if (getenv('CI')) { $this->markTestSkipped(); }"),
            conditional("getenv('CI')", true)
        );
        assert_eq!(
            php("if (empty($_ENV['CI'])) { $this->markTestSkipped(); }"),
            conditional("empty($_ENV['CI'])", false)
        );
        assert_eq!(
            php("if ($this->config['CI_MODE']) { $this->markTestSkipped(); }"),
            conditional("$this->config['CI_MODE']", false)
        );
        assert_eq!(php("$this->markTestSkipped();"), UNCONDITIONAL);
    }

    fn swift(body: &str) -> Read {
        read(
            "Tests/QTests/QTests.swift",
            &format!("final class QTests: XCTestCase {{\n    func testAdds() throws {{\n        {body}\n        XCTAssertEqual(1 + 1, 2)\n    }}\n}}\n"),
        )
    }

    #[test]
    fn swift_reads_guard_and_the_condition_taking_calls() {
        let env = "ProcessInfo.processInfo.environment[\"CI\"]";
        assert_eq!(
            swift(&format!("guard {env} == nil else {{ throw XCTSkip() }}")),
            conditional(&format!("!({env} == nil)"), true)
        );
        assert_eq!(
            swift(&format!("try XCTSkipUnless({env} != nil)")),
            conditional(&format!("!({env} != nil)"), false)
        );
        assert_eq!(
            swift("try XCTSkipIf(ProcessInfo.processInfo.environment[\"HOME\"] != nil)"),
            conditional(
                "ProcessInfo.processInfo.environment[\"HOME\"] != nil",
                false
            )
        );
        assert_eq!(swift("throw XCTSkip()"), UNCONDITIONAL);
    }

    fn scala(body: &str) -> Read {
        read(
            "src/test/scala/QSuite.scala",
            &format!("class QSuite extends AnyFunSuite {{\n  test(\"adds\") {{\n    {body}\n    assert(1 + 1 == 2)\n  }}\n}}\n"),
        )
    }

    #[test]
    fn scala_reads_assume_and_cancel_under_an_if() {
        assert_eq!(
            scala("assume(!sys.env.contains(\"CI\"))"),
            conditional("!(!sys.env.contains(\"CI\"))", true)
        );
        assert_eq!(
            scala("if (sys.env.contains(\"CI\")) println(1) else cancel()"),
            conditional("!(sys.env.contains(\"CI\"))", false)
        );
        assert_eq!(
            scala("if (props.contains(\"CI\")) cancel()"),
            conditional("props.contains(\"CI\")", true)
        );
        assert_eq!(
            scala("if (props.contains(\"fast\")) cancel()"),
            conditional("props.contains(\"fast\")", false)
        );
        assert_eq!(scala("cancel()"), UNCONDITIONAL);
        assert_eq!(scala("assume(true)"), RUNS);
    }

    #[test]
    fn c_cpp_and_objc_read_getenv_and_the_process_environment() {
        let cpp = |body: &str| {
            read(
                "tests/q_test.cpp",
                &format!("TEST(Q, Adds) {{\n  {body}\n  EXPECT_EQ(1 + 1, 2);\n}}\n"),
            )
        };
        assert_eq!(
            cpp("if (std::getenv(\"CI\")) { GTEST_SKIP(); }"),
            conditional("std::getenv(\"CI\")", true)
        );
        assert_eq!(
            cpp("if (!std::getenv(\"CI\")) { GTEST_SKIP(); }"),
            conditional("!std::getenv(\"CI\")", false)
        );
        assert_eq!(
            cpp("if (my::getenv(\"CI\")) { GTEST_SKIP(); }"),
            conditional("my::getenv(\"CI\")", true)
        );
        assert_eq!(cpp("GTEST_SKIP();"), UNCONDITIONAL);
        let objc = |body: &str| {
            read(
                "Tests/QTests.m",
                &format!("@implementation QTests\n- (void)testAdds {{\n  {body}\n  XCTAssertEqual(1 + 1, 2);\n}}\n@end\n"),
            )
        };
        assert_eq!(
            objc("XCTSkipIf([[NSProcessInfo processInfo] environment][@\"CI\"] != nil, @\"x\");"),
            conditional(
                "[[NSProcessInfo processInfo] environment][@\"CI\"] != nil",
                true
            )
        );
        assert_eq!(
            objc(
                "XCTSkipUnless([[NSProcessInfo processInfo] environment][@\"CI\"] != nil, @\"x\");"
            ),
            conditional(
                "!([[NSProcessInfo processInfo] environment][@\"CI\"] != nil)",
                false
            )
        );
        assert_eq!(
            objc("XCTSkipIf([settings environment][@\"HOME\"] != nil, @\"x\");"),
            conditional("[settings environment][@\"HOME\"] != nil", false)
        );
        assert_eq!(objc("XCTSkip(@\"x\");"), UNCONDITIONAL);
    }

    #[test]
    fn skips_under_loops_switch_match_and_preprocessor_conditionals_are_read() {
        // C#
        let env_cs = "Environment.GetEnvironmentVariable(\"CI\")";
        assert_eq!(
            cs(&format!("while ({env_cs} != null) {{ Assert.Ignore(); }}")),
            conditional(&format!("{env_cs} != null"), true)
        );
        assert_eq!(
            cs("while (NotCi()) { Assert.Ignore(); }"),
            conditional("NotCi()", false)
        );
        assert_eq!(
            cs("for (int i = 0; i < 10; i++) { Assert.Ignore(); }"),
            conditional("i < 10", false)
        );
        assert_eq!(
            cs("foreach (var x in list) { Assert.Ignore(); }"),
            conditional("list", false)
        );
        assert_eq!(
            cs("do { Assert.Ignore(); } while (c);"),
            conditional("c", false)
        );
        assert_eq!(
            cs(&format!(
                "switch ({env_cs}) {{ case \"1\": Assert.Ignore(); break; }}"
            )),
            conditional(env_cs, true)
        );
        assert_eq!(
            cs("#if CI\nAssert.Ignore();\n#endif"),
            conditional("CI", true)
        );
        assert_eq!(
            cs("#if !CI\nAssert.Ignore();\n#endif"),
            conditional("!CI", false)
        );

        // Ruby
        assert_eq!(
            rb("while ENV[\"CI\"]; skip; end"),
            conditional("ENV[\"CI\"]", true)
        );
        assert_eq!(
            rb("until ENV[\"CI\"]; skip; end"),
            conditional("!(ENV[\"CI\"])", false)
        );
        assert_eq!(
            rb("skip while ENV[\"CI\"]"),
            conditional("ENV[\"CI\"]", true)
        );
        assert_eq!(
            rb("skip until ENV[\"CI\"]"),
            conditional("!(ENV[\"CI\"])", false)
        );
        assert_eq!(rb("for x in xs; skip; end"), conditional("xs", false));
        assert_eq!(
            rb("case ENV[\"CI\"]; when 1; skip; end"),
            conditional("ENV[\"CI\"]", true)
        );

        // PHP
        assert_eq!(
            php("while (getenv('CI')) { $this->markTestSkipped(); }"),
            conditional("getenv('CI')", true)
        );
        assert_eq!(
            php("for ($i = 0; $i < 10; $i++) { $this->markTestSkipped(); }"),
            conditional("$i < 10", false)
        );
        assert_eq!(
            php("do { $this->markTestSkipped(); } while ($c);"),
            conditional("$c", false)
        );
        assert_eq!(
            php("foreach ($xs as $x) { $this->markTestSkipped(); }"),
            conditional("$xs", false)
        );
        assert_eq!(
            php("switch (getenv('CI')) { case 1: $this->markTestSkipped(); }"),
            conditional("getenv('CI')", true)
        );
        assert_eq!(
            php("$res = match (getenv('CI')) { 1 => $this->markTestSkipped(), default => null };"),
            conditional("getenv('CI')", true)
        );

        // Swift
        let env_swift = "ProcessInfo.processInfo.environment[\"CI\"]";
        assert_eq!(
            swift(&format!("while {env_swift} != nil {{ throw XCTSkip() }}")),
            conditional(&format!("{env_swift} != nil"), true)
        );
        assert_eq!(
            swift("repeat { throw XCTSkip() } while c"),
            conditional("c", false)
        );
        assert_eq!(
            swift("for x in xs { throw XCTSkip() }"),
            conditional("xs", false)
        );
        assert_eq!(
            swift(&format!(
                "switch {env_swift} {{ case nil: throw XCTSkip(); default: break; }}"
            )),
            conditional(env_swift, true)
        );
        assert_eq!(
            swift("#if CI\nthrow XCTSkip()\n#endif"),
            conditional("CI", true)
        );
        assert_eq!(
            swift("#if !CI\nthrow XCTSkip()\n#endif"),
            conditional("!(CI)", false)
        );

        // Scala
        assert_eq!(
            scala("while (sys.env.contains(\"CI\")) { cancel() }"),
            conditional("sys.env.contains(\"CI\")", true)
        );
        assert_eq!(
            scala("for (x <- xs) { cancel() }"),
            conditional("x <- xs", false)
        );
        assert_eq!(
            scala("sys.env.get(\"CI\") match { case Some(_) => cancel(); case _ => () }"),
            conditional("sys.env.get(\"CI\")", true)
        );

        // C / C++ / ObjC
        let cpp = |body: &str| {
            read(
                "tests/q_test.cpp",
                &format!("TEST(Q, Adds) {{\n  {body}\n  EXPECT_EQ(1 + 1, 2);\n}}\n"),
            )
        };
        assert_eq!(
            cpp("while (std::getenv(\"CI\")) { GTEST_SKIP(); }"),
            conditional("std::getenv(\"CI\")", true)
        );
        assert_eq!(
            cpp("for (int i = 0; i < 10; i++) { GTEST_SKIP(); }"),
            conditional("i < 10", false)
        );
        assert_eq!(
            cpp("do { GTEST_SKIP(); } while (c);"),
            conditional("c", false)
        );
        assert_eq!(
            cpp("switch (my::getenv(\"CI\")) { case 1: GTEST_SKIP(); }"),
            conditional("my::getenv(\"CI\")", true)
        );
        assert_eq!(
            cpp("#ifdef CI\nGTEST_SKIP();\n#endif"),
            conditional("CI", true)
        );
        assert_eq!(
            cpp("#ifndef CI\nGTEST_SKIP();\n#endif"),
            conditional("!(CI)", false)
        );
        assert_eq!(
            cpp("#if defined(CI)\nGTEST_SKIP();\n#endif"),
            conditional("defined(CI)", true)
        );
        assert_eq!(
            cpp("#if !defined(CI)\nGTEST_SKIP();\n#endif"),
            conditional("!defined(CI)", false)
        );
        assert_eq!(
            cpp("#if OTHER\n//\n#elif defined(CI)\nGTEST_SKIP();\n#endif"),
            conditional("defined(CI)", true)
        );
    }
}
