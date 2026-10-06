//! Whether a CI environment variable decides a test's conditional skip, and which way.
//!
//! A skip under `if os.environ.get("CI")` stops the test running in CI; a skip under
//! `if not os.environ.get("CI")` leaves it running there. The condition is evaluated from
//! its syntax tree into one of three values: true in CI, true outside CI, or involving a
//! CI variable in a way this module does not decide. The last is treated as a skip in CI.
//!
//! A name is followed within its file: a local variable, a module-level variable or
//! constant, and a function defined in the file whose result is the read. A name bound in
//! another file is not followed. One that is not bound in the file and is spelled like a
//! CI variable (`CI`, `settings.CI`) counts as a read of it.

use std::collections::{BTreeSet, HashMap};
use tree_sitter::Node;

/// What a language pack decided about a conditional skip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CiVerdict {
    /// The test is skipped when these CI variables are set.
    Skips(Vec<String>),
    /// No CI variable makes the test skip: the condition names none, or holds only
    /// outside CI.
    NotCi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Python,
    Go,
    JavaScript,
    Rust,
}

/// The enclosing conditions of a skip or early exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    pub verdict: CiVerdict,
    /// Whether any enclosing condition involves a CI variable.
    pub related: bool,
    /// The condition as reported: the nearest `if`, negated in its `else` branch, or the
    /// whole chain when an outer `if` is what makes the skip CI-conditional.
    pub text: String,
    /// Whether the nearest `if` holds the site in its `else` branch.
    pub in_else: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Truth {
    InCi,
    NotInCi,
    Mixed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Ci {
    truth: Truth,
    vars: BTreeSet<String>,
}

type Val = Option<Ci>;

fn one(truth: Truth, var: &str) -> Val {
    Some(Ci {
        truth,
        vars: BTreeSet::from([var.to_string()]),
    })
}

fn flip(v: Val) -> Val {
    v.map(|c| Ci {
        truth: match c.truth {
            Truth::InCi => Truth::NotInCi,
            Truth::NotInCi => Truth::InCi,
            Truth::Mixed => Truth::Mixed,
        },
        vars: c.vars,
    })
}

fn union(a: &Ci, b: &Ci) -> BTreeSet<String> {
    a.vars.union(&b.vars).cloned().collect()
}

/// `a && b`: a skip under both. A part that names no CI variable only narrows the skip.
fn and(a: Val, b: Val) -> Val {
    match (a, b) {
        (None, x) | (x, None) => x,
        (Some(a), Some(b)) => Some(Ci {
            truth: if a.truth == b.truth {
                a.truth
            } else {
                Truth::Mixed
            },
            vars: union(&a, &b),
        }),
    }
}

/// `a || b`: a skip under either. With a part that names no CI variable, a part true in
/// CI still skips every CI run, and a part true outside CI leaves the CI run to the other
/// part: the same table as [`and`].
fn or(a: Val, b: Val) -> Val {
    and(a, b)
}

/// Values a name or function can take, from each place the file binds it.
fn combine(vals: Vec<Val>) -> Val {
    let mut it = vals.into_iter();
    let first = it.next()?;
    it.fold(first, |acc, v| match (acc, v) {
        (None, None) => None,
        (Some(a), Some(b)) if a.truth == b.truth => Some(Ci {
            truth: a.truth,
            vars: union(&a, &b),
        }),
        (Some(a), Some(b)) => Some(Ci {
            truth: Truth::Mixed,
            vars: union(&a, &b),
        }),
        (Some(a), None) | (None, Some(a)) => Some(Ci {
            truth: Truth::Mixed,
            vars: a.vars,
        }),
    })
}

/// A variable a CI system sets, as the name of an environment read.
pub fn is_ci_env_read_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    super::CI_VARS.contains(&upper.as_str())
        || matches!(
            upper.as_str(),
            "GITHUB_RUN_ID" | "BUILD_ID" | "DRONE" | "WOODPECKER"
        )
        || upper.starts_with("CI_")
}

/// A name that is not an environment read but is spelled like a CI variable.
fn mention(name: &str) -> Option<&'static str> {
    super::CI_VARS
        .iter()
        .copied()
        .find(|v| v.eq_ignore_ascii_case(name))
}

const FUNCTION_KINDS: &[&str] = &[
    "function_definition",
    "lambda",
    "function_declaration",
    "method_declaration",
    "func_literal",
    "arrow_function",
    "function_expression",
    "function",
    "generator_function_declaration",
    "method_definition",
    "function_item",
    "closure_expression",
];

const IF_KINDS: &[&str] = &["if_statement", "if_expression", "elif_clause"];

enum Bound<'t> {
    Expr(Node<'t>),
    /// A name destructured from the environment object (`const { CI } = process.env`).
    EnvVar(String),
}

type Bindings<'t> = HashMap<String, Vec<Bound<'t>>>;

struct Eval<'t, 's> {
    lang: Lang,
    src: &'s [u8],
    module: Bindings<'t>,
}

const MAX_DEPTH: usize = 8;

impl<'t, 's> Eval<'t, 's> {
    fn text(&self, n: Node) -> &'s str {
        n.utf8_text(self.src).unwrap_or("")
    }

    /// The content of a string literal node, or `None` for any other node.
    fn string_value(&self, n: Node) -> Option<String> {
        match n.kind() {
            "string" | "interpreted_string_literal" | "raw_string_literal" | "string_literal" => {}
            _ => return None,
        }
        // An interpolated string is not a constant.
        let mut cursor = n.walk();
        if n.named_children(&mut cursor).any(|c| {
            matches!(
                c.kind(),
                "interpolation" | "template_substitution" | "escape_sequence"
            )
        }) {
            return None;
        }
        let raw = self
            .text(n)
            .trim_start_matches(|c: char| c.is_ascii_alphabetic());
        let quote = raw.chars().next()?;
        if !matches!(quote, '"' | '\'' | '`') {
            return None;
        }
        let inner = raw.strip_prefix(quote)?.strip_suffix(quote)?;
        Some(inner.to_string())
    }

    fn first_argument(&self, call: Node<'t>) -> Option<Node<'t>> {
        let args = call.child_by_field_name("arguments")?;
        let mut cursor = args.walk();
        let first = args.named_children(&mut cursor).next();
        first
    }

    /// Whether `n` is the process environment object (`os.environ`, `process.env`).
    fn is_env_object(&self, n: Node) -> bool {
        match self.lang {
            Lang::Python => matches!(self.text(n), "os.environ" | "environ"),
            Lang::JavaScript => {
                if n.kind() != "member_expression" {
                    return false;
                }
                let object = n.child_by_field_name("object").map(|o| self.text(o));
                let property = n.child_by_field_name("property").map(|p| self.text(p));
                property == Some("env") && matches!(object, Some("process" | "import.meta"))
            }
            Lang::Go | Lang::Rust => false,
        }
    }

    /// The variable an environment read names, when `n` is one.
    fn env_read(&self, n: Node<'t>) -> Option<String> {
        match (self.lang, n.kind()) {
            (Lang::Python, "call") => {
                let func = n.child_by_field_name("function")?;
                let reads = match func.kind() {
                    "identifier" => self.text(func) == "getenv",
                    "attribute" => {
                        let object = func.child_by_field_name("object")?;
                        let attr = self.text(func.child_by_field_name("attribute")?);
                        (attr == "getenv" && self.text(object) == "os")
                            || (attr == "get" && self.is_env_object(object))
                    }
                    _ => false,
                };
                if !reads {
                    return None;
                }
                self.string_value(self.first_argument(n)?)
            }
            (Lang::Python, "subscript") => {
                let value = n.child_by_field_name("value")?;
                if !self.is_env_object(value) {
                    return None;
                }
                self.string_value(n.child_by_field_name("subscript")?)
            }
            (Lang::Go, "call_expression") => {
                let func = n.child_by_field_name("function")?;
                if func.kind() != "selector_expression" {
                    return None;
                }
                let operand = self.text(func.child_by_field_name("operand")?);
                let field = self.text(func.child_by_field_name("field")?);
                if !(matches!(operand, "os" | "syscall") && matches!(field, "Getenv" | "LookupEnv"))
                {
                    return None;
                }
                self.string_value(self.first_argument(n)?)
            }
            (Lang::JavaScript, "member_expression") => {
                let object = n.child_by_field_name("object")?;
                if !self.is_env_object(object) {
                    return None;
                }
                Some(self.text(n.child_by_field_name("property")?).to_string())
            }
            (Lang::JavaScript, "subscript_expression") => {
                let object = n.child_by_field_name("object")?;
                if !self.is_env_object(object) {
                    return None;
                }
                self.string_value(n.child_by_field_name("index")?)
            }
            (Lang::Rust, "call_expression") => {
                let func = n.child_by_field_name("function")?;
                if func.kind() != "scoped_identifier" {
                    return None;
                }
                let name = self.text(func.child_by_field_name("name")?);
                let path = self.text(func.child_by_field_name("path")?);
                if !(matches!(name, "var" | "var_os") && matches!(path, "env" | "std::env")) {
                    return None;
                }
                self.string_value(self.first_argument(n)?)
            }
            (Lang::Rust, "macro_invocation") => {
                let name = self.text(n.child_by_field_name("macro")?);
                if !matches!(name, "option_env" | "env" | "std::option_env" | "std::env") {
                    return None;
                }
                let mut cursor = n.walk();
                let tree = n.children(&mut cursor).find(|c| c.kind() == "token_tree")?;
                let mut inner = tree.walk();
                let literal = tree.named_children(&mut inner).next()?;
                self.string_value(literal)
            }
            _ => None,
        }
    }

    /// A constant on one side of a comparison: `Some(true)` when comparing equal to it
    /// means the variable is unset or off (`""`, `None`, `"false"`, `"0"`).
    fn literal_is_falsy(&self, n: Node<'t>) -> Option<bool> {
        if let Some(s) = self.string_value(n) {
            let lower = s.to_ascii_lowercase();
            return Some(matches!(lower.as_str(), "" | "0" | "false" | "no" | "off"));
        }
        match n.kind() {
            "none" | "nil" | "null" | "undefined" | "false" => Some(true),
            "true" => Some(false),
            "integer" | "int_literal" | "number" | "integer_literal" => {
                Some(self.text(n).trim() == "0")
            }
            "identifier" => match self.text(n) {
                "None" | "nil" | "undefined" | "False" | "false" => Some(true),
                "True" | "true" => Some(false),
                _ => None,
            },
            "boolean_literal" => Some(self.text(n) == "false"),
            // `Ok("true")`, `Some("1")`.
            "call_expression" if self.lang == Lang::Rust => {
                let func = n.child_by_field_name("function")?;
                if !matches!(self.text(func), "Ok" | "Some") {
                    return None;
                }
                self.literal_is_falsy(self.first_argument(n)?)
            }
            "parenthesized_expression" => {
                let mut cursor = n.walk();
                let inner = n.named_children(&mut cursor).next()?;
                self.literal_is_falsy(inner)
            }
            _ => None,
        }
    }

    fn compare(
        &self,
        left: Node<'t>,
        op: &str,
        right: Node<'t>,
        locals: &Bindings<'t>,
        depth: usize,
    ) -> Option<Val> {
        let equal = match op {
            "==" | "===" | "is" => true,
            "!=" | "!==" | "is not" => false,
            _ => return None,
        };
        let (value, falsy) = if let Some(f) = self.literal_is_falsy(right) {
            (self.eval(left, locals, depth), f)
        } else {
            let f = self.literal_is_falsy(left)?;
            (self.eval(right, locals, depth), f)
        };
        value.as_ref()?;
        Some(if equal == falsy { flip(value) } else { value })
    }

    /// Fallback for a node this module has no rule for: a CI variable anywhere under it
    /// is involved in a way that is not decided.
    fn undecided(&self, n: Node<'t>, locals: &Bindings<'t>, depth: usize) -> Val {
        let mut cursor = n.walk();
        let mut vars = BTreeSet::new();
        for child in n.named_children(&mut cursor) {
            if FUNCTION_KINDS.contains(&child.kind()) {
                continue;
            }
            if let Some(c) = self.eval(child, locals, depth) {
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

    fn eval_name(&self, name: &str, locals: &Bindings<'t>, depth: usize) -> Val {
        let bound = locals.get(name).or_else(|| self.module.get(name));
        match bound {
            Some(bounds) => combine(
                bounds
                    .iter()
                    .map(|b| match b {
                        Bound::EnvVar(var) if is_ci_env_read_name(var) => one(Truth::InCi, var),
                        Bound::EnvVar(_) => None,
                        Bound::Expr(e) if FUNCTION_KINDS.contains(&e.kind()) => None,
                        Bound::Expr(e) => self.eval(*e, locals, depth + 1),
                    })
                    .collect(),
            ),
            None => mention(name).and_then(|var| one(Truth::InCi, var)),
        }
    }

    /// The value a call of a function defined in this file returns.
    fn eval_function(&self, func: Node<'t>, depth: usize) -> Val {
        let body = func.child_by_field_name("body")?;
        let mut locals = Bindings::new();
        self.collect_bindings(body, &mut locals, false);
        let mut returns = Vec::new();
        if matches!(body.kind(), "block" | "statement_block") {
            self.collect_returns(body, &mut returns);
            // A Rust block's value is its last expression.
            if self.lang == Lang::Rust {
                let mut cursor = body.walk();
                if let Some(last) = body.named_children(&mut cursor).last() {
                    if !last.kind().ends_with("_statement")
                        && !last.kind().ends_with("_declaration")
                        && !last.kind().ends_with("_item")
                        && last.kind() != "return_expression"
                    {
                        returns.push(last);
                    }
                }
            }
        } else {
            // `() => expr`, `lambda: expr`.
            returns.push(body);
        }
        let decided = combine(
            returns
                .iter()
                .map(|r| self.eval(*r, &locals, depth + 1))
                .collect(),
        );
        decided.or_else(|| self.undecided(body, &locals, depth + 1))
    }

    fn collect_returns(&self, n: Node<'t>, out: &mut Vec<Node<'t>>) {
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            if FUNCTION_KINDS.contains(&child.kind()) {
                continue;
            }
            if matches!(child.kind(), "return_statement" | "return_expression") {
                let mut inner = child.walk();
                let value = child.named_children(&mut inner).next();
                if let Some(mut value) = value {
                    if value.kind() == "expression_list" {
                        value = value.named_child(0).unwrap_or(value);
                    }
                    out.push(value);
                }
                continue;
            }
            self.collect_returns(child, out);
        }
    }

    fn eval_call(&self, n: Node<'t>, locals: &Bindings<'t>, depth: usize) -> Val {
        let Some(func) = n.child_by_field_name("function") else {
            return self.undecided(n, locals, depth);
        };
        match func.kind() {
            "identifier" => {
                let name = self.text(func);
                // A function defined in this file.
                let defined = locals.get(name).or_else(|| self.module.get(name));
                if let Some(bounds) = defined {
                    let functions: Vec<Node<'t>> = bounds
                        .iter()
                        .filter_map(|b| match b {
                            Bound::Expr(e) if FUNCTION_KINDS.contains(&e.kind()) => Some(*e),
                            _ => None,
                        })
                        .collect();
                    if !functions.is_empty() {
                        return combine(
                            functions
                                .iter()
                                .map(|f| self.eval_function(*f, depth + 1))
                                .collect(),
                        );
                    }
                }
                // A conversion that keeps truthiness.
                if matches!(
                    name,
                    "bool" | "str" | "int" | "Boolean" | "String" | "Number"
                ) {
                    if let Some(arg) = self.first_argument(n) {
                        return self.eval(arg, locals, depth);
                    }
                }
                self.undecided(n, locals, depth)
            }
            // A method on a value: `.is_ok()`, `.lower()`, `strings.ToLower(x)`.
            "attribute" | "member_expression" | "field_expression" | "selector_expression" => {
                let receiver = ["object", "value", "operand"]
                    .iter()
                    .find_map(|f| func.child_by_field_name(f));
                let method = ["attribute", "property", "field"]
                    .iter()
                    .find_map(|f| func.child_by_field_name(f))
                    .map(|m| self.text(m))
                    .unwrap_or("");
                if let Some(receiver) = receiver {
                    if self.text(receiver) == "strings"
                        && matches!(method, "ToLower" | "ToUpper" | "TrimSpace")
                    {
                        if let Some(arg) = self.first_argument(n) {
                            return self.eval(arg, locals, depth);
                        }
                    }
                    let value = self.eval(receiver, locals, depth);
                    if value.is_some() {
                        return match method {
                            "is_ok" | "is_some" | "ok" | "as_deref" | "as_ref" | "clone"
                            | "unwrap_or_default" | "unwrap_or" | "lower" | "upper" | "strip"
                            | "toLowerCase" | "toUpperCase" | "trim" | "to_lowercase"
                            | "to_uppercase" | "as_str" | "to_string" | "to_string_lossy" => value,
                            "is_err" | "is_none" | "is_empty" => flip(value),
                            _ => self.undecided(n, locals, depth),
                        };
                    }
                }
                self.undecided(n, locals, depth)
            }
            _ => self.undecided(n, locals, depth),
        }
    }

    fn eval(&self, n: Node<'t>, locals: &Bindings<'t>, depth: usize) -> Val {
        if depth > MAX_DEPTH {
            return None;
        }
        if let Some(var) = self.env_read(n) {
            return if is_ci_env_read_name(&var) {
                one(Truth::InCi, &var)
            } else {
                None
            };
        }
        match n.kind() {
            "comment" | "line_comment" | "block_comment" => None,
            "parenthesized_expression" => {
                let mut cursor = n.walk();
                let inner = n.named_children(&mut cursor).next();
                inner.and_then(|i| self.eval(i, locals, depth))
            }
            "not_operator" => {
                let arg = n.child_by_field_name("argument")?;
                flip(self.eval(arg, locals, depth))
            }
            "unary_expression" => {
                let negates = n.child(0).is_some_and(|c| c.kind() == "!");
                let mut cursor = n.walk();
                let operand = n.named_children(&mut cursor).last();
                match operand {
                    Some(o) if negates => flip(self.eval(o, locals, depth)),
                    _ => self.undecided(n, locals, depth),
                }
            }
            "boolean_operator" | "binary_expression" => {
                let (Some(left), Some(right), Some(op)) = (
                    n.child_by_field_name("left"),
                    n.child_by_field_name("right"),
                    n.child_by_field_name("operator"),
                ) else {
                    return self.undecided(n, locals, depth);
                };
                match op.kind() {
                    "and" | "&&" => and(
                        self.eval(left, locals, depth),
                        self.eval(right, locals, depth),
                    ),
                    "or" | "||" | "??" => or(
                        self.eval(left, locals, depth),
                        self.eval(right, locals, depth),
                    ),
                    // `"CI" in process.env`
                    "in" if self.is_env_object(right) => match self.string_value(left) {
                        Some(var) if is_ci_env_read_name(&var) => one(Truth::InCi, &var),
                        _ => None,
                    },
                    kind => self
                        .compare(left, kind, right, locals, depth)
                        .unwrap_or_else(|| self.undecided(n, locals, depth)),
                }
            }
            "comparison_operator" => {
                let mut cursor = n.walk();
                let operands: Vec<Node<'t>> = n.named_children(&mut cursor).collect();
                if operands.len() != 2 {
                    return self.undecided(n, locals, depth);
                }
                let (left, right) = (operands[0], operands[1]);
                let op = self.src[left.end_byte()..right.start_byte()]
                    .split(|b| b.is_ascii_whitespace())
                    .filter(|w| !w.is_empty())
                    .map(|w| std::str::from_utf8(w).unwrap_or(""))
                    .collect::<Vec<_>>()
                    .join(" ");
                // `"CI" in os.environ`, `"CI" not in os.environ`
                if matches!(op.as_str(), "in" | "not in") && self.is_env_object(right) {
                    let read = match self.string_value(left) {
                        Some(var) if is_ci_env_read_name(&var) => one(Truth::InCi, &var),
                        _ => None,
                    };
                    return if op == "in" { read } else { flip(read) };
                }
                self.compare(left, &op, right, locals, depth)
                    .unwrap_or_else(|| self.undecided(n, locals, depth))
            }
            "call" | "call_expression" => self.eval_call(n, locals, depth),
            "identifier" => self.eval_name(self.text(n), locals, depth),
            "property_identifier" | "field_identifier" | "type_identifier" => {
                mention(self.text(n)).and_then(|var| one(Truth::InCi, var))
            }
            "string" | "interpreted_string_literal" | "raw_string_literal" | "string_literal" => {
                self.string_value(n)
                    .and_then(|s| mention(&s))
                    .and_then(|var| one(Truth::InCi, var))
            }
            _ => self.undecided(n, locals, depth),
        }
    }

    /// Names bound to a value under `scope`: every assignment and declaration of a
    /// function body, or with `top_level` the declarations of a file.
    fn collect_bindings(&self, scope: Node<'t>, out: &mut Bindings<'t>, top_level: bool) {
        let mut cursor = scope.walk();
        for child in scope.named_children(&mut cursor) {
            let kind = child.kind();
            if FUNCTION_KINDS.contains(&kind) {
                if let Some(name) = child.child_by_field_name("name") {
                    out.entry(self.text(name).to_string())
                        .or_default()
                        .push(Bound::Expr(child));
                }
                continue;
            }
            match kind {
                // Python `x = value`
                "assignment" => {
                    self.bind_pair(
                        child.child_by_field_name("left"),
                        child.child_by_field_name("right"),
                        out,
                    );
                }
                // Go `x := value`, `x = value`, `var x = value`, `const x = value`
                "short_var_declaration" | "assignment_statement" => {
                    self.bind_lists(
                        child.child_by_field_name("left"),
                        child.child_by_field_name("right"),
                        out,
                    );
                }
                "var_spec" | "const_spec" => {
                    let mut names = child.walk();
                    let lefts: Vec<Node<'t>> =
                        child.children_by_field_name("name", &mut names).collect();
                    let rights = child
                        .child_by_field_name("value")
                        .map(|v| self.list_items(v))
                        .unwrap_or_default();
                    self.bind_each(&lefts, &rights, out);
                }
                // JavaScript `const x = value`, `const { CI } = process.env`
                "variable_declarator" => {
                    let name = child.child_by_field_name("name");
                    let value = child.child_by_field_name("value");
                    match (name, value) {
                        (Some(name), Some(value))
                            if name.kind() == "object_pattern" && self.is_env_object(value) =>
                        {
                            let mut props = name.walk();
                            for prop in name.named_children(&mut props) {
                                if prop.kind() == "shorthand_property_identifier_pattern" {
                                    let var = self.text(prop).to_string();
                                    out.entry(var.clone()).or_default().push(Bound::EnvVar(var));
                                }
                            }
                        }
                        _ => self.bind_pair(name, value, out),
                    }
                }
                // Rust `let x = value;`, `const X: T = value;`, `static X: T = value;`
                "let_declaration" => {
                    self.bind_pair(
                        child.child_by_field_name("pattern"),
                        child.child_by_field_name("value"),
                        out,
                    );
                }
                "const_item" | "static_item" => {
                    self.bind_pair(
                        child.child_by_field_name("name"),
                        child.child_by_field_name("value"),
                        out,
                    );
                }
                _ => {}
            }
            // A file's bindings are its own statements, wherever a declaration keyword,
            // an `export` or a Rust `mod` wraps them; a function's are anywhere in it.
            let descend = if top_level {
                matches!(
                    kind,
                    "expression_statement"
                        | "decorated_definition"
                        | "var_declaration"
                        | "var_spec_list"
                        | "const_declaration"
                        | "lexical_declaration"
                        | "variable_declaration"
                        | "export_statement"
                        | "mod_item"
                        | "declaration_list"
                )
            } else {
                true
            };
            if descend {
                self.collect_bindings(child, out, top_level);
            }
        }
    }

    fn list_items(&self, n: Node<'t>) -> Vec<Node<'t>> {
        if n.kind() == "expression_list" {
            let mut cursor = n.walk();
            n.named_children(&mut cursor).collect()
        } else {
            vec![n]
        }
    }

    fn bind_pair(&self, name: Option<Node<'t>>, value: Option<Node<'t>>, out: &mut Bindings<'t>) {
        if let (Some(name), Some(value)) = (name, value) {
            if name.kind() == "identifier" {
                out.entry(self.text(name).to_string())
                    .or_default()
                    .push(Bound::Expr(value));
            }
        }
    }

    fn bind_lists(&self, left: Option<Node<'t>>, right: Option<Node<'t>>, out: &mut Bindings<'t>) {
        if let (Some(left), Some(right)) = (left, right) {
            self.bind_each(&self.list_items(left), &self.list_items(right), out);
        }
    }

    /// Pairs names with values. `v, ok := os.LookupEnv("CI")` binds both names to the one
    /// call: either is set when the variable is.
    fn bind_each(&self, names: &[Node<'t>], values: &[Node<'t>], out: &mut Bindings<'t>) {
        for (i, name) in names.iter().enumerate() {
            let value = if values.len() == names.len() {
                Some(values[i])
            } else if values.len() == 1 {
                Some(values[0])
            } else {
                None
            };
            self.bind_pair(Some(*name), value, out);
        }
    }

    fn condition_text(&self, if_node: Node<'t>, cond: Node<'t>) -> String {
        let cond_text =
            if self.lang == Lang::JavaScript && cond.kind() == "parenthesized_expression" {
                let mut cursor = cond.walk();
                let inner = cond.named_children(&mut cursor).next().unwrap_or(cond);
                self.text(inner).trim()
            } else {
                self.text(cond).trim()
            };
        match if_node.child_by_field_name("initializer") {
            Some(init) => format!("{}; {cond_text}", self.text(init).trim()),
            None => cond_text.to_string(),
        }
    }

    fn negated_text(&self, text: &str) -> String {
        match self.lang {
            Lang::Python => format!("not ({text})"),
            Lang::Go | Lang::JavaScript | Lang::Rust => format!("!({text})"),
        }
    }
}

/// Every function around `site`, innermost first: a test callback, and the `describe`
/// callback or outer test whose variables it closes over.
fn enclosing_functions(site: Node) -> Vec<Node> {
    let mut out = Vec::new();
    let mut cur = site;
    while let Some(p) = cur.parent() {
        if FUNCTION_KINDS.contains(&p.kind()) {
            out.push(p);
        }
        cur = p;
    }
    out
}

fn root_of(site: Node) -> Node {
    let mut cur = site;
    while let Some(p) = cur.parent() {
        cur = p;
    }
    cur
}

fn evaluator<'t, 's>(lang: Lang, site: Node<'t>, src: &'s [u8]) -> (Eval<'t, 's>, Bindings<'t>) {
    let mut eval = Eval {
        lang,
        src,
        module: Bindings::new(),
    };
    let mut module = Bindings::new();
    eval.collect_bindings(root_of(site), &mut module, true);
    eval.module = module;
    let mut locals = Bindings::new();
    for function in enclosing_functions(site) {
        if let Some(body) = function.child_by_field_name("body") {
            eval.collect_bindings(body, &mut locals, false);
        }
    }
    (eval, locals)
}

fn verdict_of(value: &Val) -> CiVerdict {
    match value {
        Some(c) if c.truth != Truth::NotInCi => CiVerdict::Skips(c.vars.iter().cloned().collect()),
        _ => CiVerdict::NotCi,
    }
}

/// The conditions under which `site` (a skip call or an early `return`) runs: every `if`
/// between it and its function. `None` when no `if` encloses it.
pub fn site(lang: Lang, site: Node, src: &[u8]) -> Option<Site> {
    let (eval, locals) = evaluator(lang, site, src);
    // Innermost first: (condition text, value, in the else branch).
    let mut chain: Vec<(String, Val, bool)> = Vec::new();
    let mut cur = site;
    while let Some(p) = cur.parent() {
        if FUNCTION_KINDS.contains(&p.kind()) {
            break;
        }
        if IF_KINDS.contains(&p.kind()) {
            if let Some(cond) = p.child_by_field_name("condition") {
                let in_condition = cond.id() == cur.id();
                let in_initializer = p
                    .child_by_field_name("initializer")
                    .is_some_and(|i| i.id() == cur.id());
                if !in_condition && !in_initializer {
                    let in_else = p
                        .child_by_field_name("consequence")
                        .is_none_or(|c| c.id() != cur.id());
                    let value = eval.eval(cond, &locals, 0);
                    let text = eval.condition_text(p, cond);
                    if in_else {
                        chain.push((eval.negated_text(&text), flip(value), true));
                    } else {
                        chain.push((text, value, false));
                    }
                }
            }
        }
        cur = p;
    }
    let (nearest_text, nearest_value, in_else) = chain.first()?.clone();
    let whole = chain
        .iter()
        .fold(None, |acc, (_, value, _)| and(acc, value.clone()));
    let verdict = verdict_of(&whole);
    let nearest_decides = matches!(verdict_of(&nearest_value), CiVerdict::Skips(_));
    let text = if matches!(verdict, CiVerdict::Skips(_)) && !nearest_decides {
        let joiner = if lang == Lang::Python {
            " and "
        } else {
            " && "
        };
        chain
            .iter()
            .rev()
            .map(|(text, _, _)| text.as_str())
            .collect::<Vec<_>>()
            .join(joiner)
    } else {
        nearest_text
    };
    Some(Site {
        verdict,
        related: whole.is_some(),
        text,
        in_else,
    })
}

/// Joins what a pack already reports for a skip with what the syntax tree says about it.
/// `legacy` is the condition the pack reports (the nearest `if` holding the skip in its
/// body), `None` where the pack reads the skip as unconditional. Returns the condition
/// and verdict to record, or `None` to leave the skip unconditional: a skip in the
/// `else` branch of an `if` is conditional here only when a CI variable is involved.
pub fn conditional(legacy: Option<String>, site: Option<Site>) -> Option<(String, CiVerdict)> {
    match (legacy, site) {
        (Some(text), Some(site)) => Some(match site.verdict {
            CiVerdict::Skips(_) => (site.text, site.verdict),
            CiVerdict::NotCi => (text, site.verdict),
        }),
        (Some(text), None) => Some((text, CiVerdict::NotCi)),
        (None, Some(site)) if site.related => Some((site.text, site.verdict)),
        (None, _) => None,
    }
}

/// The verdict for one expression used as a skip condition (`skipIf(<expr>)`), with
/// `negated` for a run condition (`runIf(<expr>)`). `None` when it involves no CI variable.
pub fn expression(lang: Lang, expr: Node, src: &[u8], negated: bool) -> Option<CiVerdict> {
    let (eval, locals) = evaluator(lang, expr, src);
    let value = eval.eval(expr, &locals, 0);
    let value = if negated { flip(value) } else { value };
    value.as_ref()?;
    Some(verdict_of(&value))
}

/// Statements `is_exit` accepts that sit directly in a branch of an `if` of `body`,
/// nested `if`s included, in source order.
pub fn exits_under_if<'t>(body: Node<'t>, is_exit: &dyn Fn(Node<'t>) -> bool) -> Vec<Node<'t>> {
    fn statements<'t>(block: Node<'t>, out: &mut Vec<Node<'t>>) {
        let mut cursor = block.walk();
        for child in block.named_children(&mut cursor) {
            match child.kind() {
                // Go wraps a block's statements; Rust and Python wrap an expression.
                "statement_list" | "block" | "statement_block" | "else_clause" => {
                    statements(child, out)
                }
                "expression_statement" => match child.named_child(0) {
                    Some(inner) if IF_KINDS.contains(&inner.kind()) => out.push(inner),
                    _ => out.push(child),
                },
                _ => out.push(child),
            }
        }
    }
    fn walk<'t>(
        block: Node<'t>,
        under_if: bool,
        is_exit: &dyn Fn(Node<'t>) -> bool,
        out: &mut Vec<Node<'t>>,
    ) {
        let mut stmts = Vec::new();
        if matches!(
            block.kind(),
            "block" | "statement_block" | "statement_list" | "else_clause"
        ) {
            statements(block, &mut stmts);
        } else {
            // A branch that is one statement: `if (x) return;`
            stmts.push(block);
        }
        for stmt in stmts {
            if IF_KINDS.contains(&stmt.kind()) {
                let mut cursor = stmt.walk();
                for (i, branch) in stmt.children(&mut cursor).enumerate() {
                    let field = stmt.field_name_for_child(i as u32);
                    if matches!(field, Some("consequence" | "alternative" | "body")) {
                        walk(branch, true, is_exit, out);
                    }
                }
            } else if under_if && is_exit(stmt) {
                out.push(stmt);
            }
        }
    }
    let mut out = Vec::new();
    walk(body, false, is_exit, &mut out);
    out
}

/// The verdict for a Rust `cfg` predicate under which a test is ignored
/// (`#[cfg_attr(<predicate>, ignore)]`), from the predicate's token nodes. `None` when it
/// names no CI cfg.
pub fn rust_cfg_predicate(nodes: &[Node], src: &[u8]) -> Option<CiVerdict> {
    fn split<'t>(nodes: &[Node<'t>]) -> Vec<Vec<Node<'t>>> {
        let mut out = vec![Vec::new()];
        for n in nodes {
            if n.kind() == "," {
                out.push(Vec::new());
            } else if let Some(last) = out.last_mut() {
                last.push(*n);
            }
        }
        out.retain(|part| !part.is_empty());
        out
    }
    fn inner<'t>(tree: Node<'t>) -> Vec<Node<'t>> {
        let mut cursor = tree.walk();
        tree.children(&mut cursor)
            .filter(|c| !matches!(c.kind(), "(" | ")" | "[" | "]" | "{" | "}"))
            .collect()
    }
    fn eval(nodes: &[Node], src: &[u8]) -> Val {
        let items: Vec<Node> = nodes
            .iter()
            .copied()
            .filter(|n| !matches!(n.kind(), "line_comment" | "block_comment"))
            .collect();
        let first = items.first()?;
        let name = first.utf8_text(src).ok()?;
        if first.kind() != "identifier" {
            return None;
        }
        match items.get(1) {
            Some(tree) if tree.kind() == "token_tree" => {
                let parts = split(&inner(*tree));
                match name {
                    "not" => flip(parts.first().and_then(|p| eval(p, src))),
                    "all" => parts.iter().fold(None, |acc, p| and(acc, eval(p, src))),
                    "any" => parts.iter().fold(None, |acc, p| or(acc, eval(p, src))),
                    _ => None,
                }
            }
            // A bare cfg name: `ci`, `github_actions`.
            None => mention(name).and_then(|var| one(Truth::InCi, var)),
            Some(_) => None,
        }
    }
    let value = eval(nodes, src);
    value.as_ref()?;
    Some(verdict_of(&value))
}

/// Each case reads a test through its language pack and asks the two questions the
/// `ignored-tests` gate asks: does the test carry a conditional skip, and does a CI
/// variable decide it.
#[cfg(all(
    test,
    feature = "lang-python",
    feature = "lang-go",
    feature = "lang-javascript",
    feature = "lang-rust"
))]
mod tests {
    use crate::ast::{AssertVocabulary, LanguagePack, TestFn};

    fn only_test(pack: &dyn LanguagePack, path: &str, src: &str) -> TestFn {
        let facts = pack
            .extract(path, src, &AssertVocabulary::default())
            .unwrap();
        assert_eq!(facts.tests.len(), 1, "{src}");
        facts.tests[0].clone()
    }

    fn py(module: &str, body: &str) -> TestFn {
        let src = format!(
            "import os\nimport pytest\n\n{module}\ndef test_q():\n{body}    assert 1 + 1 == 2\n"
        );
        only_test(&crate::ast::python::PythonPack, "tests/test_q.py", &src)
    }

    fn go(package: &str, body: &str) -> TestFn {
        let src = format!(
            "package p\n\nimport (\n\t\"os\"\n\t\"testing\"\n)\n{package}\nfunc TestA(t *testing.T) {{\n{body}\tif 1+1 != 2 {{\n\t\tt.Fatal(\"math\")\n\t}}\n}}\n"
        );
        only_test(&crate::ast::r#go::GoPack, "p_test.go", &src)
    }

    fn js(module: &str, call: &str, body: &str) -> TestFn {
        let src = format!(
            "{module}\n{call}('adds', function () {{\n{body}  expect(1 + 1).toBe(2);\n}});\n"
        );
        only_test(&crate::ast::javascript::JavaScriptPack, "a.test.js", &src)
    }

    fn rs(items: &str, attrs: &str, body: &str) -> TestFn {
        let src =
            format!("{items}\n{attrs}#[test]\nfn adds() {{\n{body}    assert_eq!(1 + 1, 2);\n}}\n");
        only_test(&crate::ast::rust::RustPack, "tests/q.rs", &src)
    }

    /// `(conditional skip recorded, a CI variable decides it)`
    fn read(test: &TestFn) -> (bool, bool) {
        (
            test.conditional_ignore.is_some() && !test.ignored,
            test.is_ci_skip(),
        )
    }

    const CI_SKIP: (bool, bool) = (true, true);
    const OTHER_SKIP: (bool, bool) = (true, false);

    #[test]
    fn a_ci_read_through_a_name_of_the_file_decides_the_skip() {
        let skip = "    if IN_CI:\n        pytest.skip()\n";
        // Module constant, local variable, helper.
        assert_eq!(read(&py("IN_CI = os.environ.get(\"CI\")\n", skip)), CI_SKIP);
        assert_eq!(
            read(&py(
                "",
                "    IN_CI = os.getenv(\"CI\")\n    if IN_CI:\n        pytest.skip()\n"
            )),
            CI_SKIP
        );
        assert_eq!(
            read(&py(
                "def is_ci():\n    return bool(os.environ.get(\"CI\"))\n",
                "    if is_ci():\n        pytest.skip()\n"
            )),
            CI_SKIP
        );
        assert_eq!(
            read(&go(
                "var inCI = os.Getenv(\"CI\") != \"\"\n",
                "\tif inCI {\n\t\tt.Skip()\n\t}\n"
            )),
            CI_SKIP
        );
        assert_eq!(
            read(&go(
                "func isCI() bool {\n\treturn os.Getenv(\"CI\") != \"\"\n}\n",
                "\tif isCI() {\n\t\treturn\n\t}\n"
            )),
            CI_SKIP
        );
        assert_eq!(
            read(&js(
                "const isCI = !!process.env.CI;\n",
                "test",
                "  if (isCI) return;\n"
            )),
            CI_SKIP
        );
        assert_eq!(
            read(&js(
                "const inCI = () => process.env.CI === 'true';\n",
                "test",
                "  if (inCI()) return;\n"
            )),
            CI_SKIP
        );
        assert_eq!(
            read(&rs(
                "fn in_ci() -> bool {\n    std::env::var(\"CI\").is_ok()\n}\n",
                "",
                "    if in_ci() {\n        return;\n    }\n"
            )),
            CI_SKIP
        );
        // Controls: the same shapes bound to something that is not a CI read.
        assert_eq!(
            read(&py("IN_CI = os.environ.get(\"SLOW\")\n", skip)),
            OTHER_SKIP
        );
        assert_eq!(
            read(&go(
                "var inCI = os.Getenv(\"SLOW\") != \"\"\n",
                "\tif inCI {\n\t\tt.Skip()\n\t}\n"
            )),
            OTHER_SKIP
        );
        // No CI read and no environment read: an early return is not a skip at all.
        assert_eq!(
            read(&js(
                "const isCI = false;\n",
                "test",
                "  if (isCI) return;\n"
            )),
            (false, false)
        );
    }

    #[test]
    fn a_skip_that_holds_only_outside_ci_is_not_decided_by_ci() {
        let py_skip = |cond: &str| py("", &format!("    if {cond}:\n        pytest.skip()\n"));
        let go_skip = |cond: &str| go("", &format!("\tif {cond} {{\n\t\tt.Skip()\n\t}}\n"));
        let js_ret = |cond: &str| js("", "test", &format!("  if ({cond}) return;\n"));
        let rs_ret = |cond: &str| {
            rs(
                "",
                "",
                &format!("    if {cond} {{\n        return;\n    }}\n"),
            )
        };
        for (name, outside, inside) in [
            (
                "python not",
                py_skip("not os.environ.get(\"CI\")"),
                py_skip("os.environ.get(\"CI\")"),
            ),
            (
                "python is None",
                py_skip("os.environ.get(\"CI\") is None"),
                py_skip("os.environ.get(\"CI\") is not None"),
            ),
            (
                "python == ''",
                py_skip("os.environ.get(\"CI\", \"\") == \"\""),
                py_skip("os.environ.get(\"CI\", \"\") != \"\""),
            ),
            (
                "python != 'true'",
                py_skip("os.environ.get(\"CI\") != \"true\""),
                py_skip("os.environ.get(\"CI\") == \"true\""),
            ),
            (
                "python not in",
                py_skip("\"CI\" not in os.environ"),
                py_skip("\"CI\" in os.environ"),
            ),
            (
                "go == \"\"",
                go_skip("os.Getenv(\"CI\") == \"\""),
                go_skip("os.Getenv(\"CI\") != \"\""),
            ),
            (
                "go !ok",
                go_skip("_, ok := os.LookupEnv(\"CI\"); !ok"),
                go_skip("_, ok := os.LookupEnv(\"CI\"); ok"),
            ),
            (
                "javascript !",
                js_ret("!process.env.CI"),
                js_ret("process.env.CI"),
            ),
            (
                "javascript === undefined",
                js_ret("process.env.CI === undefined"),
                js_ret("process.env.CI !== undefined"),
            ),
            (
                "rust is_err",
                rs_ret("std::env::var(\"CI\").is_err()"),
                rs_ret("std::env::var(\"CI\").is_ok()"),
            ),
            (
                "rust is_none",
                rs_ret("std::env::var_os(\"CI\").is_none()"),
                rs_ret("std::env::var_os(\"CI\").is_some()"),
            ),
            (
                "rust cfg_attr",
                rs("", "#[cfg_attr(not(ci), ignore)]\n", ""),
                rs("", "#[cfg_attr(ci, ignore)]\n", ""),
            ),
        ] {
            assert_eq!(read(&outside), OTHER_SKIP, "{name}: outside CI");
            // Control: the opposite polarity is a CI skip.
            assert_eq!(read(&inside), CI_SKIP, "{name}: inside CI");
        }
    }

    #[test]
    fn the_else_branch_of_a_ci_condition_is_the_opposite_condition() {
        let outside = py(
            "",
            "    if os.environ.get(\"CI\"):\n        pass\n    else:\n        pytest.skip()\n",
        );
        assert_eq!(read(&outside), OTHER_SKIP);
        assert_eq!(
            outside.conditional_ignore.as_deref(),
            Some("not (os.environ.get(\"CI\"))")
        );
        let inside = go(
            "",
            "\tif os.Getenv(\"CI\") == \"\" {\n\t\tt.Log(\"local\")\n\t} else {\n\t\tt.Skip()\n\t}\n",
        );
        assert_eq!(read(&inside), CI_SKIP);
        // Control: the `else` branch of a condition on no CI variable is unconditional.
        let other = py(
            "HAVE_DB = False\n",
            "    if HAVE_DB:\n        pass\n    else:\n        pytest.skip()\n",
        );
        assert!(other.ignored);
        assert_eq!(other.conditional_ignore, None);
    }

    #[test]
    fn an_outer_or_later_ci_condition_is_the_one_recorded() {
        let nested = py(
            "FLAKY = True\n",
            "    if os.environ.get(\"CI\"):\n        if FLAKY:\n            pytest.skip()\n",
        );
        assert_eq!(read(&nested), CI_SKIP);
        assert_eq!(
            nested.conditional_ignore.as_deref(),
            Some("os.environ.get(\"CI\") and FLAKY")
        );
        let second = rs(
            "",
            "",
            "    if std::env::var(\"SLOW\").is_ok() {\n        return;\n    }\n    if std::env::var(\"CI\").is_ok() {\n        return;\n    }\n",
        );
        assert_eq!(read(&second), CI_SKIP);
        assert_eq!(
            second.conditional_ignore.as_deref(),
            Some("std::env::var(\"CI\").is_ok()")
        );
        // Control: with no CI condition the first one stays.
        let first = rs(
            "",
            "",
            "    if std::env::var(\"SLOW\").is_ok() {\n        return;\n    }\n    if std::env::var(\"FAST\").is_ok() {\n        return;\n    }\n",
        );
        assert_eq!(read(&first), OTHER_SKIP);
        assert_eq!(
            first.conditional_ignore.as_deref(),
            Some("std::env::var(\"SLOW\").is_ok()")
        );
    }

    #[test]
    fn a_name_spelled_like_a_ci_variable_counts_only_when_the_file_does_not_bind_it() {
        let bound = py(
            "",
            "    ci = os.path.exists(\"ci.sock\")\n    if ci:\n        pytest.skip()\n",
        );
        assert_eq!(read(&bound), OTHER_SKIP);
        // Control: not bound in the file (a fixture, an import).
        let unbound = py("", "    if ci:\n        pytest.skip()\n");
        assert_eq!(read(&unbound), CI_SKIP);
    }

    #[test]
    fn run_if_and_this_skip_are_read_only_when_they_skip_in_ci() {
        assert_eq!(read(&js("", "test.runIf(!process.env.CI)", "")), CI_SKIP);
        assert_eq!(
            read(&js(
                "",
                "it",
                "  if (process.env.CI) {\n    this.skip();\n  }\n"
            )),
            CI_SKIP
        );
        // Controls: a run condition that holds in CI, and a skip outside CI.
        assert_eq!(
            read(&js("", "test.runIf(process.env.CI)", "")),
            (false, false)
        );
        assert_eq!(
            read(&js(
                "",
                "it",
                "  if (!process.env.CI) {\n    this.skip();\n  }\n"
            )),
            (false, false)
        );
    }

    #[test]
    fn ci_variable_names_of_an_environment_read() {
        for name in [
            "CI",
            "GITHUB_ACTIONS",
            "GITHUB_RUN_ID",
            "CI_JOB_ID",
            "BUILD_ID",
            "DRONE",
            "WOODPECKER",
        ] {
            assert!(super::is_ci_env_read_name(name), "{name}");
        }
        for name in [
            "CIRCUS",
            "SKIP_SLOW",
            "BUILD_IDENTITY",
            "DRONES",
            "MY_CI_FLAG",
        ] {
            assert!(!super::is_ci_env_read_name(name), "{name}");
        }
        assert_eq!(
            read(&py(
                "",
                "    if os.environ.get(\"DRONE\"):\n        pytest.skip()\n"
            )),
            CI_SKIP
        );
        assert_eq!(
            read(&py(
                "",
                "    if os.environ.get(\"DRONES\"):\n        pytest.skip()\n"
            )),
            OTHER_SKIP
        );
    }
}
