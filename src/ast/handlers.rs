//! Error handlers that swallow: `except: pass`, `catch (e) {}`, `rescue; end`, a
//! discarded `Result` (`let _ = fallible();`, `fallible().ok();`).
//!
//! A failure an agent cannot fix is easy to hide behind an empty handler; the tests then
//! pass because the error never surfaces. One walker with per-language node kinds finds
//! each handler and judges its body the way `functions.rs` judges a function body.

use tree_sitter::Node;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwallowSite {
    pub line: usize,
    /// `empty-handler`, `logging-handler`, `skipped-input`, `discarded-result`,
    /// `discarded-value`, `silenced-error`, or `constant-fallback` (kept apart from the
    /// others, in `ParsedFileFacts::constant_fallbacks`).
    pub kind: &'static str,
    /// The handler's first line, trimmed.
    pub snippet: String,
}

/// Reads a silencing expression from the syntax tree: the site's kind, or `None`.
pub type SilenceNode = fn(Node, &str, &HandlerSpec) -> Option<&'static str>;

pub struct HandlerSpec {
    /// Node kinds that are an error handler with a body (`catch_clause`, `except_clause`).
    pub handler_kinds: &'static [&'static str],
    /// When not empty, `handler_kinds` are the arms of a handler (Scala's `case` in a
    /// `catch`): an arm counts only when its parent or grandparent is one of these
    /// kinds, and an arm with no body at all is empty.
    pub arm_of: &'static [&'static str],
    /// Field or child kind holding the handler body.
    pub body_fields: &'static [&'static str],
    /// Node kinds ignored when counting statements.
    pub ignored_kinds: &'static [&'static str],
    /// Statement texts that swallow when they are the whole body (after trimming `;`).
    pub trivial: &'static [&'static str],
    /// Node kinds of a statement that may discard a result (`let_declaration`,
    /// `expression_statement`); judged by `discards`.
    pub discard_kinds: &'static [&'static str],
    /// Whether a statement's text discards a result.
    pub discards: fn(&str) -> bool,
    /// Sorts a discarding statement by what it discards: `Some(kind)` is the site's kind,
    /// `None` is not a discarded result. Unset: every discard is `discarded-result`.
    pub classify_discard: Option<fn(Node, &str) -> Option<&'static str>>,
    /// For a binding statement, the node kinds of a right-hand side that is a call; a
    /// binding of anything else (a tuple, an identifier) is not a discarded result.
    pub call_value_kinds: &'static [&'static str],
    /// Node kinds of an expression that silences the errors of what it wraps (PHP's `@`,
    /// Ruby's `rescue` modifier); judged by `silences`.
    pub silence_kinds: &'static [&'static str],
    /// Whether a silencing expression's text drops the error rather than handling it.
    pub silences: fn(&str) -> bool,
    /// A syntax-tree check a silencing expression must also pass (JS/TS `.catch(...)`:
    /// the callback's body is read from the tree, not from text). It returns the site's
    /// kind (`silenced-error`, or `logging-handler` for a callback that only logs), or
    /// `None` when the expression handles the error. Unset: `silences` alone, as
    /// `silenced-error`.
    pub silence_node: Option<SilenceNode>,
}

/// Statement heads that only record: a logging or printing call. A handler made of these
/// alone logs and swallows; a stub padded with these is still a stub.
pub const LOGGING_VOCAB: &[&str] = &[
    "log::",
    "log.",
    "logger.",
    "logging.",
    "tracing::",
    "warn!(",
    "info!(",
    "debug!(",
    "error!(",
    "trace!(",
    "println!(",
    "eprintln!(",
    "print!(",
    "eprint!(",
    "console.",
    "print(",
    "println(",
    "pprint(",
    "fmt.Print",
    "log.Print",
    "slog.",
    "zap.",
    "System.out.print",
    "System.err.print",
    "Console.Write",
    "Debug.Write",
    "Trace.Write",
    "_logger.",
    "Log.",
    "logger::",
    "error_log(",
    "printf(",
    "fprintf(",
    "puts ",
    "puts(",
    "warn ",
    "p ",
    "pp ",
    "echo ",
    "var_dump(",
    "print_r(",
    "std::cerr",
    "std::cout",
    "spdlog::",
    "LOG(",
    "LOG_",
    "NSLog(",
];

/// Whether a statement's text is a logging or printing call and nothing else.
pub fn is_logging_statement(t: &str) -> bool {
    let t = t.trim();
    LOGGING_VOCAB.iter().any(|v| t.starts_with(v))
}

fn text<'a>(node: Node, src: &'a str) -> &'a str {
    node.utf8_text(src.as_bytes()).unwrap_or("")
}

/// Whether `t` is one of the pack's trivial statements. Runs of whitespace are folded to
/// one space, so `return  []` and `return\n[]` match the `return []` entry.
fn is_trivial(spec: &HandlerSpec, t: &str) -> bool {
    let folded = t.split_whitespace().collect::<Vec<_>>().join(" ");
    spec.trivial.contains(&folded.as_str())
}

fn first_line(t: &str) -> String {
    t.lines().next().unwrap_or("").trim().to_string()
}

/// How a handler body fails the error: `empty-handler` when it does nothing with it,
/// `logging-handler` when every statement only logs it.
fn body_swallows(body: Node, src: &str, spec: &HandlerSpec) -> Option<&'static str> {
    let mut cursor = body.walk();
    let stmts: Vec<Node> = body
        .named_children(&mut cursor)
        .filter(|c| !spec.ignored_kinds.contains(&c.kind()))
        .collect();
    let mut all = body.walk();
    let has_other_named = body.named_children(&mut all).next().is_some();
    match stmts.len() {
        // No statement at all (a comment inside the block does not count), or a body
        // that is a bare expression rather than a statement list.
        0 if has_other_named => Some("empty-handler"),
        0 => {
            let inner = text(body, src)
                .trim()
                .trim_start_matches('{')
                .trim_end_matches('}')
                .trim();
            let inner = inner.trim_end_matches(';').trim();
            if inner.is_empty() || is_trivial(spec, inner) {
                Some("empty-handler")
            } else if is_logging_statement(inner) {
                Some("logging-handler")
            } else {
                None
            }
        }
        1 => {
            let t = text(stmts[0], src).trim().trim_end_matches(';').trim();
            if is_trivial(spec, t) {
                Some("empty-handler")
            } else if is_logging_statement(t) {
                Some("logging-handler")
            } else {
                None
            }
        }
        // Several statements that all only log: the error is recorded and dropped. Any
        // other statement (a re-raise, a return of the error, a state change) handles it.
        _ if stmts.iter().all(|st| is_logging_statement(text(*st, src))) => Some("logging-handler"),
        _ => None,
    }
}

/// How a pack's syntax tree spells a handler statement that puts a number in place of the
/// result: `except FileNotFoundError: ops_per_sec = 150000.0`. Every field is a list of
/// node kinds; nothing here is matched against source text.
pub struct ConstantSpec {
    /// A handler body whose named children are its statements. A body of any other kind
    /// is one statement itself (a Scala arm: `case e: E => 150000.0`).
    pub blocks: &'static [&'static str],
    /// Nodes that stand for their only named child: an expression statement, parentheses,
    /// Ruby's `return` argument list, Swift's assignable expression.
    pub wrappers: &'static [&'static str],
    /// Numeric literals.
    pub numbers: &'static [&'static str],
    /// A unary expression. It is a numeric literal when its operator token is `-` or `+`
    /// and its operand is one of `numbers`.
    pub signs: &'static [&'static str],
    /// A plain assignment. A grammar that uses the same kind for `+=` is told apart by
    /// the operator token, which must be `=`.
    pub assignments: &'static [&'static str],
    /// What an assignment may store into: a name, an attribute, a subscript.
    pub targets: &'static [&'static str],
    /// Calls. A target, or a collection key, with one of these below it runs other code
    /// and is not read as a constant fallback.
    pub calls: &'static [&'static str],
    /// A `return` with a value. Its first token must be `return` (Swift uses one kind
    /// for `return`, `throw` and `break`).
    pub returns: &'static [&'static str],
    /// The handler's value is its last expression (Ruby, Kotlin, Scala): a number there
    /// is what `return <number>` is elsewhere.
    pub value_is_last_expression: bool,
    /// Collection literals, read one level deep.
    pub collections: &'static [&'static str],
    /// Nodes holding one of `collections` beside a type (`new double[] {1.0, 2.0}`).
    pub collection_holders: &'static [&'static str],
    /// A `key: value` entry of a collection; its value is its last named child.
    pub pairs: &'static [&'static str],
    /// Literal keys of an entry.
    pub keys: &'static [&'static str],
}

/// The pack's own named children of `node`: its ignored kinds (comments) left out.
fn own_children<'t>(node: Node<'t>, spec: &HandlerSpec) -> Vec<Node<'t>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|c| !spec.ignored_kinds.contains(&c.kind()))
        .collect()
}

/// `node`, or what it wraps when it is one of the pack's wrappers around one child.
fn unwrapped<'t>(mut node: Node<'t>, spec: &HandlerSpec, c: &ConstantSpec) -> Node<'t> {
    while c.wrappers.contains(&node.kind()) {
        let [only] = own_children(node, spec)[..] else {
            break;
        };
        node = only;
    }
    node
}

/// Whether a call sits anywhere below `node`.
fn has_call_below(node: Node, c: &ConstantSpec) -> bool {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();
    children
        .into_iter()
        .any(|child| c.calls.contains(&child.kind()) || has_call_below(child, c))
}

/// Whether the text of a numeric literal node is a zero, whatever its spelling: `0`,
/// `0.0`, `0.`, `.0`, `0e0`, `0x0`, `0b0`, `0o0`, `00`, with digit separators (`0_0`,
/// C++ `0'0`), with a type suffix (`0L`, `0f`, `0.0f`, `0u`, `0m`, `0d`, `0n`, `0r`,
/// `0j`), under a sign the grammar keeps inside the literal (`-0`, `-0.0`). The value is
/// read from the digits: after a `0x` / `0b` / `0o` prefix the digits of that radix, else
/// the decimal digits before any exponent or suffix; it is zero when there is a digit and
/// none is other than `0`. `0x0f` is fifteen (`f` is a hex digit there), `1e-9`, `0.1`
/// and `007` are not zero.
///
/// The caller has already established from the node's kind that this is a numeric
/// literal; the text is read only for its value.
fn is_zero_literal(literal: &str) -> bool {
    let t: String = literal
        .trim()
        .trim_start_matches(['-', '+'])
        .chars()
        .filter(|c| !matches!(c, '_' | '\'') && !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    let (digits, radix) = match t.get(..2) {
        Some("0x") => (&t[2..], 16),
        Some("0b") => (&t[2..], 2),
        Some("0o") => (&t[2..], 8),
        _ => (t.as_str(), 10),
    };
    let mantissa: Vec<char> = digits
        .chars()
        .take_while(|c| *c == '.' || c.is_digit(radix))
        .collect();
    mantissa.contains(&'0') && mantissa.iter().all(|c| matches!(c, '0' | '.'))
}

/// `Some(true)` for a numeric literal, signed or not, that is not a zero, `Some(false)`
/// for one that is, `None` for anything else.
fn number(node: Node, src: &str, spec: &HandlerSpec, c: &ConstantSpec) -> Option<bool> {
    let node = unwrapped(node, spec, c);
    let literal = if c.numbers.contains(&node.kind()) {
        node
    } else if c.signs.contains(&node.kind())
        && node
            .child(0)
            .is_some_and(|op| !op.is_named() && matches!(op.kind(), "-" | "+"))
    {
        let [operand] = own_children(node, spec)[..] else {
            return None;
        };
        if !c.numbers.contains(&operand.kind()) {
            return None;
        }
        operand
    } else {
        return None;
    };
    Some(!is_zero_literal(text(literal, src)))
}

/// A numeric literal that is not a zero: a number invented in place of a result. A zero
/// is a default, in every pack, and is not one.
fn is_fallback_number(node: Node, src: &str, spec: &HandlerSpec, c: &ConstantSpec) -> bool {
    number(node, src, spec, c) == Some(true)
}

/// A number as `is_fallback_number` reads it, or a collection literal whose every value
/// is a numeric literal, at least one of them not a zero, its keys being literals. A
/// collection of zeros only is a default, like a zero.
fn is_fallback_value(node: Node, src: &str, spec: &HandlerSpec, c: &ConstantSpec) -> bool {
    let node = unwrapped(node, spec, c);
    if is_fallback_number(node, src, spec, c) {
        return true;
    }
    let collection = if c.collections.contains(&node.kind()) {
        node
    } else if c.collection_holders.contains(&node.kind()) {
        let held: Vec<Node> = own_children(node, spec)
            .into_iter()
            .filter(|n| c.collections.contains(&n.kind()))
            .collect();
        let [held] = held[..] else {
            return false;
        };
        held
    } else {
        return false;
    };
    let literal_key = |k: Node| {
        let k = unwrapped(k, spec, c);
        c.keys.contains(&k.kind()) && !has_call_below(k, c)
    };
    let mut non_zero = 0usize;
    let mut value = |v: Node| {
        let read = number(v, src, spec, c);
        non_zero += usize::from(read == Some(true));
        read.is_some()
    };
    let mut cursor = collection.walk();
    if !cursor.goto_first_child() {
        return false;
    }
    loop {
        let child = cursor.node();
        if !child.is_named() && cursor.field_name().is_some() {
            // A value the grammar spells as a bare token (Swift's `nil`), not punctuation.
            return false;
        }
        if child.is_named() && !spec.ignored_kinds.contains(&child.kind()) {
            if c.pairs.contains(&child.kind()) {
                let ok = match own_children(child, spec)[..] {
                    [v] => value(v),
                    [key, v] => literal_key(key) && value(v),
                    _ => false,
                };
                if !ok {
                    return false;
                }
            } else if cursor.field_name() == Some("key") {
                if !literal_key(child) {
                    return false;
                }
            } else if !value(child) {
                return false;
            }
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }
    non_zero > 0
}

/// Whether `stmt` stores or returns a number in place of the result: an assignment of one
/// to a name, attribute or subscript, a `return` of one or of a collection of them, or,
/// where the handler's value is its last expression, that expression (`last`).
fn is_fallback_statement(
    stmt: Node,
    src: &str,
    spec: &HandlerSpec,
    c: &ConstantSpec,
    last: bool,
) -> bool {
    let node = unwrapped(stmt, spec, c);
    let field = |names: &[&str]| names.iter().find_map(|f| node.child_by_field_name(f));
    if c.assignments.contains(&node.kind()) {
        let mut cursor = node.walk();
        let plain = node
            .children(&mut cursor)
            .any(|t| !t.is_named() && t.kind() == "=");
        let (Some(left), Some(right)) = (field(&["left", "target"]), field(&["right", "result"]))
        else {
            return false;
        };
        let target = unwrapped(left, spec, c);
        plain
            && c.targets.contains(&target.kind())
            && !has_call_below(target, c)
            && is_fallback_number(right, src, spec, c)
    } else if c.returns.contains(&node.kind()) {
        node.child(0).is_some_and(|t| t.kind() == "return")
            && matches!(own_children(node, spec)[..], [value] if is_fallback_value(value, src, spec, c))
    } else {
        last && c.value_is_last_expression && is_fallback_value(node, src, spec, c)
    }
}

/// Every node the pack names as the handler's body: one block, or, for a Scala arm
/// written without braces, each of its statements.
fn handler_bodies<'t>(handler: Node<'t>, spec: &HandlerSpec) -> Vec<Node<'t>> {
    for f in spec.body_fields {
        let mut cursor = handler.walk();
        let by_field: Vec<Node> = handler.children_by_field_name(f, &mut cursor).collect();
        if !by_field.is_empty() {
            return by_field;
        }
        let mut cursor = handler.walk();
        let by_kind: Vec<Node> = handler
            .children(&mut cursor)
            .filter(|c| c.kind() == *f)
            .collect();
        if !by_kind.is_empty() {
            return by_kind;
        }
    }
    Vec::new()
}

/// Whether a handler replaces the failure with a number: every statement of its body,
/// logging calls aside, is one `is_fallback_statement` accepts, and at least one is. A
/// re-raise, a call, a store or return of the error, a value that marks absence (`None`,
/// `null`, a NaN however it is spelled: none of them is a numeric literal node) or a
/// zero is some other statement, and the handler is then not one.
fn is_constant_fallback(handler: Node, src: &str, spec: &HandlerSpec, c: &ConstantSpec) -> bool {
    let bodies = handler_bodies(handler, spec);
    let stmts: Vec<Node> = match bodies[..] {
        [block] if c.blocks.contains(&block.kind()) => own_children(block, spec),
        _ => bodies,
    };
    let mut fallbacks = 0usize;
    for (i, stmt) in stmts.iter().enumerate() {
        if is_fallback_statement(*stmt, src, spec, c, i + 1 == stmts.len()) {
            fallbacks += 1;
        } else if !is_logging_statement(text(*stmt, src)) {
            return false;
        }
    }
    fallbacks > 0
}

/// Sites in `root`, in source order. `is_test_line` excludes handlers inside tests.
pub fn extract(
    root: Node,
    src: &str,
    spec: &HandlerSpec,
    is_test_line: &dyn Fn(usize) -> bool,
) -> Vec<SwallowSite> {
    extract_with_constants(root, src, spec, None, is_test_line).0
}

/// The sites of [`extract`], and beside them the handlers that replace the failure with a
/// numeric literal (`constant-fallback`), read when the pack passes its [`ConstantSpec`].
/// The two lists share no handler: a body the first list holds (empty, default literal,
/// logging only) is never in the second. `error-swallowing` reads the second only in the
/// files `constant_fallback_paths` names.
pub fn extract_with_constants(
    root: Node,
    src: &str,
    spec: &HandlerSpec,
    constants: Option<&ConstantSpec>,
    is_test_line: &dyn Fn(usize) -> bool,
) -> (Vec<SwallowSite>, Vec<SwallowSite>) {
    let mut out = Vec::new();
    let mut fallbacks = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let line = node.start_position().row + 1;
        // Ruby's `rescue` keyword token has the same kind as the `rescue` clause; only
        // the named node is a handler.
        let in_arm_parent = || {
            spec.arm_of.is_empty() || {
                let p = node.parent();
                let in_arm = |n: Option<Node>| n.is_some_and(|n| spec.arm_of.contains(&n.kind()));
                in_arm(p) || in_arm(p.and_then(|p| p.parent()))
            }
        };
        if node.is_named()
            && spec.handler_kinds.contains(&node.kind())
            && in_arm_parent()
            && !is_test_line(line)
        {
            let body = spec.body_fields.iter().find_map(|f| {
                node.child_by_field_name(f).or_else(|| {
                    let mut cursor = node.walk();
                    let found = node.children(&mut cursor).find(|c| c.kind() == *f);
                    found
                })
            });
            let swallows = match body {
                Some(b) => body_swallows(b, src, spec),
                // An arm with nothing after `=>` handles nothing.
                None if !spec.arm_of.is_empty() => Some("empty-handler"),
                // A handler with no body node at all (`except: pass` on one line in some
                // grammars) is judged by its own text.
                None => {
                    // `Foo::Bar` in an exception path is not the `:` that ends a Python head.
                    let t = text(node, src).replace("::", "");
                    let after = t.split_once([':', '{']).map(|(_, r)| r).unwrap_or("");
                    let after = after.trim().trim_end_matches('}').trim();
                    let after = after.trim_end_matches(';').trim();
                    if after.is_empty() || is_trivial(spec, after) {
                        Some("empty-handler")
                    } else if is_logging_statement(after) {
                        Some("logging-handler")
                    } else {
                        None
                    }
                }
            };
            if swallows.is_none()
                && constants.is_some_and(|c| is_constant_fallback(node, src, spec, c))
                && !expects_the_error(node, src)
                && !catches_only_signals(node, src)
            {
                fallbacks.push(SwallowSite {
                    line,
                    kind: "constant-fallback",
                    snippet: first_line(text(node, src)),
                });
            }
            if let Some(kind) = swallows
                .filter(|_| !expects_the_error(node, src) && !catches_only_signals(node, src))
                .map(|k| {
                    if k == "empty-handler" && skips_unparseable_input(node, src) {
                        "skipped-input"
                    } else {
                        k
                    }
                })
            {
                out.push(SwallowSite {
                    line,
                    kind,
                    snippet: first_line(text(node, src)),
                });
            }
        } else if spec.discard_kinds.contains(&node.kind()) && !is_test_line(line) {
            let t = text(node, src);
            let binding_of_call = node.child_by_field_name("value").is_none_or(|v| {
                spec.call_value_kinds.is_empty() || spec.call_value_kinds.contains(&v.kind())
            });
            let kind = if binding_of_call && (spec.discards)(t) {
                spec.classify_discard
                    .map_or(Some("discarded-result"), |classify| classify(node, src))
            } else {
                None
            };
            if let Some(kind) = kind {
                out.push(SwallowSite {
                    line,
                    kind,
                    snippet: first_line(t),
                });
            }
        } else if spec.silence_kinds.contains(&node.kind()) && !is_test_line(line) {
            let t = text(node, src);
            let kind = if (spec.silences)(t) {
                spec.silence_node
                    .map_or(Some("silenced-error"), |f| f(node, src, spec))
            } else {
                None
            };
            if let Some(kind) = kind.filter(|_| !result_is_tested(node, src)) {
                out.push(SwallowSite {
                    line,
                    kind,
                    snippet: first_line(t),
                });
                continue;
            }
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
    (out, fallbacks)
}

/// The expect-this-to-raise idiom: the handler is the passing path and the code around it
/// fails when nothing was raised. Either the `try` body ends in a statement that always
/// fails (`assert False`, `raise`, `pytest.fail(...)`), the `try` has an `else` that
/// raises or fails, or the handler is `continue` / `pass` / a bare `return` and the
/// statement after the `try` records a failure (`try: fn() except E: return` then
/// `raise AssertionError(...)`).
fn expects_the_error(handler: Node, src: &str) -> bool {
    let Some(try_stmt) = handler.parent() else {
        return false;
    };
    let always_fails = |t: &str| {
        let t = t.trim();
        let assert_false = t
            .strip_prefix("assert")
            .map(|r| r.trim_start().trim_start_matches('('))
            .is_some_and(|r| r.starts_with("False") || r.starts_with("0,") || r == "0");
        assert_false
            || t.starts_with("raise")
            || t.starts_with("throw")
            || t.starts_with("fail(")
            || t.contains(".fail(")
    };
    let body_ends_failing = try_stmt
        .child_by_field_name("body")
        .and_then(|b| b.named_child(b.named_child_count().checked_sub(1)?))
        .is_some_and(|last| always_fails(text(last, src)));
    if body_ends_failing {
        return true;
    }
    let fails = |t: &str| {
        let t = t.trim();
        t.starts_with("raise")
            || t.starts_with("assert")
            || t.starts_with("throw")
            || t.contains(".fail(")
            || t.contains("failures.append(")
            || t.contains("errors.append(")
            || t.contains("fail(")
    };
    let mut cursor = try_stmt.walk();
    let has_failing_else = try_stmt.children(&mut cursor).any(|c| {
        matches!(c.kind(), "else_clause" | "else")
            && fails(
                text(c, src)
                    .trim_start_matches("else:")
                    .trim_start_matches("else"),
            )
    });
    if has_failing_else {
        return true;
    }
    let body = text(handler, src);
    let handler_only_skips = body
        .lines()
        .skip(1)
        .map(str::trim)
        .all(|l| l.is_empty() || matches!(l, "continue" | "pass" | "..." | "return"));
    handler_only_skips
        && try_stmt
            .next_named_sibling()
            .is_some_and(|next| fails(text(next, src)))
}

/// Whether a silencing expression's value decides what runs next: the condition of an
/// `if` / `while` / ternary, a comparison (`@f() === false`), or the left operand of
/// `&&` / `||`, possibly under `!` and parentheses (not `?:`, which substitutes). The operator mutes the diagnostic,
/// but the caller reads the failure from the return value, so nothing is swallowed.
fn result_is_tested(node: Node, src: &str) -> bool {
    let mut cur = node;
    while let Some(p) = cur.parent() {
        let is = |field: &str| {
            p.child_by_field_name(field)
                .is_some_and(|c| c.id() == cur.id())
        };
        match p.kind() {
            "parenthesized_expression" => {}
            "unary_op_expression" if text(p, src).trim_start().starts_with('!') => {}
            "binary_expression" => {
                let op = p
                    .child_by_field_name("operator")
                    .map_or("", |o| text(o, src));
                match op {
                    "&&" | "||" | "and" | "or" | "xor" if is("left") => return true,
                    "&&" | "||" | "and" | "or" | "xor" => {}
                    "===" | "!==" | "==" | "!=" | "<>" | "<" | ">" | "<=" | ">=" => return true,
                    _ => return false,
                }
            }
            "if_statement" | "else_if_clause" | "while_statement" | "do_statement" => {
                return is("condition")
            }
            // `@f() ?: 0` replaces the failure with a constant; `@f() ? a : b` branches.
            "conditional_expression" => {
                return is("condition") && p.child_by_field_name("body").is_some()
            }
            _ => return false,
        }
        cur = p;
    }
    false
}

/// Python: a handler for `KeyboardInterrupt`, `SystemExit` or `GeneratorExit` alone. These
/// derive from `BaseException`, not `Exception`: a stop request or an exit a sub-run already
/// reported (`except KeyboardInterrupt: print("stopped")`), not a failure to swallow.
fn catches_only_signals(handler: Node, src: &str) -> bool {
    const SIGNALS: &[&str] = &["KeyboardInterrupt", "SystemExit", "GeneratorExit"];
    let head = first_line(text(handler, src));
    let Some(rest) = head.strip_prefix("except") else {
        return false;
    };
    let types = rest.split(':').next().unwrap_or("");
    let types = types.split(" as ").next().unwrap_or("");
    let names: Vec<&str> = types
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split(',')
        .map(|t| t.trim().trim_start_matches("builtins."))
        .filter(|t| !t.is_empty())
        .collect();
    !names.is_empty() && names.iter().all(|n| SIGNALS.contains(n))
}

/// Python: `for line in out: try: rows.append(json.loads(line)) except JSONDecodeError:
/// continue`. The handler is `continue` alone (Python allows `continue` only in a loop,
/// however deep in `if` blocks) and it catches only parse errors: an input item that does
/// not parse is skipped. The item is still dropped without a count, so the site stays
/// reported, as `skipped-input`.
fn skips_unparseable_input(handler: Node, src: &str) -> bool {
    const PARSE_ERRORS: &[&str] = &[
        "ValueError",
        "JSONDecodeError",
        "UnicodeDecodeError",
        "InvalidOperation",
        "csv.Error",
    ];
    let head = first_line(text(handler, src));
    let Some(rest) = head.strip_prefix("except") else {
        return false;
    };
    let types = rest.split(':').next().unwrap_or("");
    let types = types.split(" as ").next().unwrap_or("");
    let parse_only = {
        let names: Vec<&str> = types
            .trim()
            .trim_start_matches('(')
            .trim_end_matches(')')
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .collect();
        !names.is_empty()
            && names.iter().all(|n| {
                PARSE_ERRORS.contains(n)
                    || PARSE_ERRORS
                        .iter()
                        .any(|p| !p.contains('.') && n.ends_with(&format!(".{p}")))
            })
    };
    let only_continue = {
        let mut cursor = handler.walk();
        let stmts: Vec<Node> = handler
            .children(&mut cursor)
            .filter(|c| c.kind() == "block")
            .flat_map(|b| {
                let mut c = b.walk();
                b.named_children(&mut c)
                    .filter(|s| s.kind() != "comment")
                    .collect::<Vec<_>>()
            })
            .collect();
        stmts.len() == 1 && stmts[0].kind() == "continue_statement"
    };
    parse_only && only_continue
}

pub fn no_discard(_: &str) -> bool {
    false
}

/// PHP: `@call()` silences every error the call raises. The node kind is exact
/// (`error_suppression_expression`), so any text qualifies.
pub fn php_silences(_: &str) -> bool {
    true
}

/// JS/TS: a `.catch(...)` call is a candidate; `js_catch_site_kind` reads the callback.
pub fn js_catch_text(t: &str) -> bool {
    t.contains(".catch(") || t.contains(".catch (")
}

/// JS/TS: how a `.catch(...)` call drops the rejection, or `None` when it does not.
/// The call has one argument, an arrow or function expression:
/// - `silenced-error` when its body is empty, a trivial statement (`return null`,
///   `return []`), or an expression body equal to one of the pack's default values
///   (`p.catch(() => {})`, `p.catch(() => null)`, `p.catch(function () {})`);
/// - `logging-handler` when its body only logs: a block whose every statement is a logging
///   call, or an expression body that is a logging call (`e => console.error(e)`), judged
///   by the same `body_swallows` / `is_logging_statement` as a `catch` clause.
///
/// A callback that computes something (`e => handle(e)`), logs then rethrows or returns a
/// computed value, or any other call is handling the rejection; a named handler
/// (`.catch(noop)`) is not read.
pub fn js_catch_site_kind(node: Node, src: &str, spec: &HandlerSpec) -> Option<&'static str> {
    if node.kind() != "call_expression" {
        return None;
    }
    let is_catch = node
        .child_by_field_name("function")
        .filter(|f| f.kind() == "member_expression")
        .and_then(|f| f.child_by_field_name("property"))
        .is_some_and(|p| text(p, src) == "catch");
    if !is_catch {
        return None;
    }
    let args = node.child_by_field_name("arguments")?;
    let mut cursor = args.walk();
    let named: Vec<Node> = args
        .named_children(&mut cursor)
        .filter(|a| a.kind() != "comment")
        .collect();
    let [callback] = named[..] else {
        return None;
    };
    if !matches!(
        callback.kind(),
        "arrow_function" | "function_expression" | "function"
    ) {
        return None;
    }
    let body = callback.child_by_field_name("body")?;
    if body.kind() == "statement_block" {
        return match body_swallows(body, src, spec) {
            Some("empty-handler") => Some("silenced-error"),
            Some("logging-handler") => Some("logging-handler"),
            _ => None,
        };
    }
    // An expression body that is a single logging call: `e => console.error(e)`. The
    // node kind rules out `console.error(e) || fallback(e)` and a comma sequence.
    if body.kind() == "call_expression" && is_logging_statement(text(body, src)) {
        return Some("logging-handler");
    }
    // An expression body: `() => null`, `() => ({})`, `() => []`. The value is silent when
    // `return <value>` is one of the pack's trivial statements.
    let mut value = text(body, src).trim();
    while let Some(inner) = value.strip_prefix('(').and_then(|v| v.strip_suffix(')')) {
        value = inner.trim();
    }
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    spec.trivial
        .iter()
        .filter_map(|t| t.strip_prefix("return "))
        .any(|v| v == value)
        .then_some("silenced-error")
}

/// Ruby: `call rescue nil` (and `rescue false` / `[]` / `{}` / `0` / `""`) replaces an
/// error with a constant. A handler that computes a fallback is not silenced.
pub fn ruby_silences(t: &str) -> bool {
    let handler = t.rsplit(" rescue ").next().unwrap_or("").trim();
    matches!(handler, "nil" | "false" | "[]" | "{}" | "0" | "''" | "\"\"")
}

/// Kotlin: `runCatching { ... }.getOrNull()` and `.getOrDefault(x)` turn a failure into a
/// value with nothing done about it; `.getOrElse { ... }` and `.onFailure { ... }` handle it.
pub fn kotlin_silences(t: &str) -> bool {
    let t = t.trim();
    (t.starts_with("runCatching") || t.starts_with("kotlin.runCatching"))
        && (t.ends_with(".getOrNull()") || t.contains(".getOrDefault("))
}

/// C / C++: `(void)call()` throws a result away by casting it, the same statement as
/// Rust's `let _ = call()`; `(void)x` of a variable silences an unused warning and is not
/// a call (the pack's `call_value_kinds` keep it out).
pub fn c_discards(t: &str) -> bool {
    t.trim().starts_with("(void)")
}

/// Rust: `let _ = f(...)` and `f(...).ok();` throw a `Result` away. A `let _ = ` binding of
/// a plain identifier or literal is not a discarded result.
pub fn rust_discards(t: &str) -> bool {
    let t = t.trim().trim_end_matches(';').trim();
    if let Some(rest) = t.strip_prefix("let _ =") {
        let rest = rest.trim();
        return rest.contains('(') && !rest.starts_with("std::mem::") && !rest.starts_with("mem::");
    }
    t.ends_with(".ok()") && !t.starts_with("let ") && !t.contains("=")
}

/// Rust callees whose result is a `Result` (or an `Option` standing for a failure) by
/// convention: throwing it away hides an error. Matched on the method or function name.
const RUST_FALLIBLE_CALLEES: &[&str] = &[
    "send",
    "recv",
    "send_to",
    "recv_from",
    "join",
    "write",
    "write_all",
    "write_fmt",
    "flush",
    "sync_all",
    "sync_data",
    "lock",
    "read",
    "read_exact",
    "read_to_string",
    "read_to_end",
    "read_line",
    "read_dir",
    "seek",
    "set_len",
    "set_permissions",
    "remove_file",
    "remove_dir",
    "remove_dir_all",
    "create_dir",
    "create_dir_all",
    "rename",
    "copy",
    "hard_link",
    "shutdown",
    "connect",
    "bind",
    "accept",
    "parse",
    "kill",
    "wait",
    "spawn",
    "commit",
    "rollback",
    "execute",
    "persist",
    "close",
    "set_nonblocking",
    "set_read_timeout",
    "set_write_timeout",
    "set_var",
    "set_current_dir",
    "from_str",
    "from_utf8",
    "blocking_send",
    "blocking_recv",
    "send_timeout",
    "recv_timeout",
];

/// Rust callees that return a plain value (a reference, an entry, the value itself):
/// binding it to `_` discards nothing fallible.
const RUST_INFALLIBLE_CALLEES: &[&str] = &[
    "get_or_init",
    "get_or_insert",
    "get_or_insert_with",
    "get_mut_or_init",
    "entry",
    "or_insert",
    "or_insert_with",
    "or_insert_with_key",
    "or_default",
    "unwrap_or",
    "unwrap_or_default",
    "unwrap_or_else",
    "clone",
    "to_owned",
    "to_string",
    "into",
    "as_ref",
    "as_mut",
    "borrow",
    "borrow_mut",
    "len",
    "is_empty",
    "hash",
    "black_box",
    "type_name",
    "size_of",
    "drop",
    "forget",
    "Box::leak",
    "leak",
];

/// The callee name of a call, method call or macro: `a.b.c(..)` is `c`, `x::y::<T>(..)` is
/// `y`, `writeln!(..)` is `writeln!`.
fn rust_callee(value: Node, src: &str) -> Option<String> {
    match value.kind() {
        "call_expression" => {
            let f = value.child_by_field_name("function")?;
            let name = match f.kind() {
                "field_expression" => text(f.child_by_field_name("field")?, src),
                "generic_function" => text(f.child_by_field_name("function")?, src),
                _ => text(f, src),
            };
            let name = name.split("::<").next().unwrap_or(name);
            Some(name.rsplit("::").next().unwrap_or(name).to_string())
        }
        "macro_invocation" => {
            let m = text(value.child_by_field_name("macro")?, src);
            Some(format!("{}!", m.rsplit("::").next().unwrap_or(m)))
        }
        "await_expression" => rust_callee(value.named_child(0)?, src),
        _ => None,
    }
}

/// Rust: sorts `let _ = <call>;` by its callee name, since the grammar carries no types.
/// A known-fallible callee (`send`, `sync_all`, `try_*`, `*_checked`, `write!`) is a
/// `discarded-result`; a known accessor (`get_or_init`, `entry`) is nothing; any other
/// callee is a `discarded-value`, which the gate reports at `warning`. `f().ok();` and
/// `let _ = f()?;` keep their reading: `.ok()` exists only to drop an error, and `?` has
/// already propagated it.
pub fn rust_discard_class(node: Node, src: &str) -> Option<&'static str> {
    let Some(value) = node.child_by_field_name("value") else {
        return Some("discarded-result");
    };
    if value.kind() == "try_expression" {
        return Some("discarded-value");
    }
    let Some(name) = rust_callee(value, src) else {
        return Some("discarded-value");
    };
    let name = name.as_str();
    if matches!(name, "write!" | "writeln!") {
        return Some("discarded-result");
    }
    if RUST_INFALLIBLE_CALLEES.contains(&name) {
        return None;
    }
    if RUST_FALLIBLE_CALLEES.contains(&name)
        || name.starts_with("try_")
        || name.starts_with("checked_")
        || name.ends_with("_checked")
    {
        return Some("discarded-result");
    }
    Some("discarded-value")
}

/// Go callees whose dropped last value is an `error` by convention.
const GO_FALLIBLE_CALLEES: &[&str] = &[
    "Write",
    "WriteString",
    "WriteByte",
    "WriteRune",
    "WriteTo",
    "ReadFrom",
    "Read",
    "ReadAll",
    "ReadFile",
    "WriteFile",
    "ReadString",
    "ReadBytes",
    "ReadLine",
    "Close",
    "Sync",
    "Flush",
    "Seek",
    "Truncate",
    "Copy",
    "CopyN",
    "Encode",
    "Decode",
    "Marshal",
    "MarshalIndent",
    "Unmarshal",
    "Atoi",
    "ParseInt",
    "ParseUint",
    "ParseFloat",
    "ParseBool",
    "Parse",
    "ParseDuration",
    "Open",
    "OpenFile",
    "Create",
    "Remove",
    "RemoveAll",
    "Mkdir",
    "MkdirAll",
    "MkdirTemp",
    "CreateTemp",
    "Rename",
    "Stat",
    "Lstat",
    "Chmod",
    "Chown",
    "Chdir",
    "Setenv",
    "Unsetenv",
    "Exec",
    "ExecContext",
    "Query",
    "QueryContext",
    "Scan",
    "Dial",
    "DialContext",
    "Listen",
    "Accept",
    "Do",
    "NewRequest",
    "NewRequestWithContext",
    "Fprintf",
    "Fprintln",
    "Fprint",
    "Shutdown",
    "Serve",
    "ListenAndServe",
    "Wait",
    "Run",
    "Start",
    "Output",
    "CombinedOutput",
    "Commit",
    "Rollback",
    "Begin",
    "BeginTx",
    "Ping",
    "Prepare",
    "Getwd",
    "Hostname",
    "Executable",
    "Glob",
    "Abs",
    "Rel",
    "EvalSymlinks",
    "Walk",
    "WalkDir",
];

/// Go callees whose second value is an ok flag or a count, not an error.
const GO_OK_CALLEES: &[&str] = &[
    "Load",
    "LoadOrStore",
    "LoadAndDelete",
    "LookupEnv",
    "Lookup",
    "Cut",
    "CutPrefix",
    "CutSuffix",
    "Swap",
    "CompareAndSwap",
    "Caller",
    "FromContext",
];

/// C / C++ callees whose result reports a failure (`-1`, non-zero, `EOF`).
const C_FALLIBLE_CALLEES: &[&str] = &[
    "write",
    "read",
    "pread",
    "pwrite",
    "close",
    "fclose",
    "fflush",
    "fsync",
    "fdatasync",
    "fwrite",
    "fread",
    "fputs",
    "fputc",
    "fprintf",
    "fscanf",
    "remove",
    "unlink",
    "rename",
    "mkdir",
    "rmdir",
    "chdir",
    "chmod",
    "chown",
    "setvbuf",
    "pthread_mutex_lock",
    "pthread_mutex_unlock",
    "pthread_join",
    "pthread_create",
    "pthread_cond_wait",
    "pthread_cond_signal",
    "pthread_cond_broadcast",
    "sem_wait",
    "sem_post",
    "dup2",
    "pipe",
    "setsid",
    "setuid",
    "setgid",
    "seteuid",
    "setegid",
    "truncate",
    "ftruncate",
    "lseek",
    "fseek",
    "system",
    "posix_memalign",
    "munmap",
    "mprotect",
    "madvise",
    "sigaction",
    "kill",
    "waitpid",
    "nanosleep",
    "clock_gettime",
    "send",
    "recv",
    "sendto",
    "recvfrom",
    "connect",
    "bind",
    "listen",
    "accept",
    "shutdown",
    "setsockopt",
    "getsockopt",
];

/// `Some("discarded-result")` for a known-fallible name, `None` for a known value-only
/// name, else `Some("discarded-value")` (reported at `warning`).
fn sort_by_callee(name: &str, fallible: &[&str], value_only: &[&str]) -> Option<&'static str> {
    if value_only.contains(&name) {
        None
    } else if fallible.contains(&name) {
        Some("discarded-result")
    } else {
        Some("discarded-value")
    }
}

/// Go: sorts a discard by what it drops. `_ = err` drops an error. `x, _ := f()` drops
/// `f`'s last value, sorted by `f`'s name. `_ = f()` drops `f`'s only value: a discarded
/// result when `f` is a known-fallible name, nothing otherwise. A type assertion, map index or channel
/// receive (`v, _ := x.(T)`, `m[k]`, `<-ch`) drops an ok flag and is not reported.
pub fn go_discard_class(node: Node, src: &str) -> Option<&'static str> {
    let right = node.child_by_field_name("right")?;
    let value = if right.kind() == "expression_list" {
        right.named_child(0)?
    } else {
        right
    };
    match value.kind() {
        "call_expression" => {
            let f = value.child_by_field_name("function")?;
            let name = match f.kind() {
                "selector_expression" => text(f.child_by_field_name("field")?, src),
                _ => text(f, src),
            };
            let class = sort_by_callee(name, GO_FALLIBLE_CALLEES, GO_OK_CALLEES);
            // `_ = f()` drops the one value `f` returns. Only a known-fallible name says
            // that value is an error; for any other callee nothing does.
            let single_blank = node
                .child_by_field_name("left")
                .is_some_and(|left| text(left, src).trim() == "_");
            if single_blank && class != Some("discarded-result") {
                return None;
            }
            class
        }
        "identifier" => Some("discarded-result"),
        _ => None,
    }
}

/// C / C++: sorts `(void)call()` by the callee's name; a known-fallible system call is
/// `discarded-result`, any other callee `discarded-value`.
pub fn c_discard_class(node: Node, src: &str) -> Option<&'static str> {
    let value = node.child_by_field_name("value")?;
    let f = value.child_by_field_name("function")?;
    let name = match f.kind() {
        "field_expression" => text(f.child_by_field_name("field")?, src),
        "template_function" => text(f.child_by_field_name("name")?, src),
        _ => text(f, src),
    };
    let name = name.rsplit("::").next().unwrap_or(name);
    sort_by_callee(name, C_FALLIBLE_CALLEES, &[])
}

/// Objective-C: `(void)call()` as in C, or a message whose `error:` argument is `nil` /
/// `NULL`, which throws the `NSError` away before it exists.
pub fn objc_discards(t: &str) -> bool {
    let t = t.trim();
    t.starts_with("(void)") || objc_drops_error(t)
}

fn objc_drops_error(t: &str) -> bool {
    let compact: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    compact.contains("error:nil]") || compact.contains("error:NULL]")
}

/// Objective-C: an `error:nil` message is a discarded result; `(void)` of a call is sorted
/// by callee as in C, and `(void)` of a message is a discarded value.
pub fn objc_discard_class(node: Node, src: &str) -> Option<&'static str> {
    match node.kind() {
        "message_expression" => {
            // Only the message that carries the argument, not an enclosing one.
            let own: String = text(node, src)
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            (own.ends_with("error:nil]") || own.ends_with("error:NULL]"))
                .then_some("discarded-result")
        }
        "cast_expression" => match node.child_by_field_name("value").map(|v| v.kind()) {
            Some("message_expression") => Some("discarded-value"),
            _ => c_discard_class(node, src),
        },
        _ => None,
    }
}

/// Scala: `Try(f).getOrElse(x)` and `Try(f).toOption` replace every failure with a value;
/// `.recover { ... }` and a `match` on the `Try` handle it.
pub fn scala_silences(t: &str) -> bool {
    let t = t.trim();
    (t.starts_with("Try(") || t.starts_with("Try {") || t.starts_with("scala.util.Try("))
        && (t.ends_with(".toOption") || t.contains(".getOrElse("))
}

/// Swift: `try?` turns a thrown error into `nil`.
pub fn swift_discards(t: &str) -> bool {
    t.trim_start().starts_with("try?")
}

/// Swift: a `try?` whose value is thrown away, as a statement of its own or bound to
/// `_`, drops the error; `let v = try? f()` keeps a value the code goes on to handle.
pub fn swift_discard_class(node: Node, src: &str) -> Option<&'static str> {
    let parent = node.parent()?;
    match parent.kind() {
        "statements" => Some("discarded-result"),
        "assignment" => {
            let target = parent
                .child_by_field_name("target")
                .map(|t| text(t, src).trim());
            (target == Some("_")).then_some("discarded-result")
        }
        _ => None,
    }
}

/// Go: `_ = err`, `_, _ = f()`, `x, _ := f()` where the dropped value is the error, and
/// `_ = f()`, which `go_discard_class` keeps only for a known-fallible callee.
pub fn go_discards(t: &str) -> bool {
    let t = t.trim();
    if t == "_ = err" || t.starts_with("_ = err") {
        return true;
    }
    // A trailing `_` on the left of `=` / `:=` of a multi-value call drops the last value.
    if let Some((lhs, rhs)) = t.split_once([':', '=']) {
        let lhs = lhs.trim().trim_end_matches(':').trim();
        let rhs = rhs.trim_start_matches('=').trim();
        // A single `_` on the left of a call drops its one value.
        return (lhs.contains(',') || lhs == "_") && lhs.ends_with('_') && rhs.contains('(');
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_and_go_discard_patterns() {
        assert!(rust_discards("let _ = file.sync_all();"));
        assert!(rust_discards("tx.commit().ok();"));
        assert!(!rust_discards("let _ = guard;"));
        assert!(!rust_discards("let _ = std::mem::replace(&mut a, b);"));
        assert!(!rust_discards("let ok = parse(s).ok();"));
        assert!(go_discards("_ = err"));
        assert!(go_discards("n, _ := w.Write(b)"));
        assert!(go_discards("_, _ = io.Copy(dst, src)"));
        assert!(!go_discards("n, err := w.Write(b)"));
        assert!(!go_discards("_ = x"));
        assert!(ruby_silences("File.read(p) rescue nil"));
        assert!(ruby_silences("x = load rescue {}"));
        assert!(!ruby_silences("x = load rescue fallback(p)"));
        assert!(kotlin_silences("runCatching { read(p) }.getOrNull()"));
        assert!(kotlin_silences(
            "runCatching { read(p) }.getOrDefault(\"\")"
        ));
        assert!(!kotlin_silences(
            "runCatching { read(p) }.getOrElse { log(it); throw it }"
        ));
        assert!(!kotlin_silences("runCatching { read(p) }"));
        assert!(c_discards("(void)write(fd, b, n)"));
        assert!(!c_discards("write(fd, b, n)"));
    }
}

#[cfg(all(
    test,
    feature = "lang-python",
    feature = "lang-javascript",
    feature = "lang-java",
    feature = "lang-rust",
    feature = "lang-go",
    feature = "lang-php",
    feature = "lang-ruby",
    feature = "lang-c",
    feature = "lang-cpp",
    feature = "lang-kotlin",
    feature = "lang-csharp"
))]
mod pack_tests {
    use crate::ast::{default_registry, AssertVocabulary, Fact};

    fn sites(path: &str, src: &str) -> Vec<(usize, &'static str)> {
        let reg = default_registry();
        let pack = reg.find_pack(path).unwrap();
        assert!(pack.supplies(Fact::Handlers));
        pack.extract(path, src, &AssertVocabulary::default())
            .unwrap()
            .swallowed
            .into_iter()
            .map(|s| (s.line, s.kind))
            .collect()
    }

    #[test]
    fn python_empty_except_is_a_site_and_a_handling_one_is_not() {
        let got = sites(
            "pkg/a.py",
            "def f():\n    try:\n        g()\n    except ValueError:\n        pass\n    try:\n        g()\n    except Exception as e:\n        log.warning(e)\n        raise\n    try:\n        g()\n    except OSError:\n        # nothing to do\n        return None\n\n\
             def test_f():\n    try:\n        f()\n    except Exception:\n        pass\n",
        );
        assert_eq!(got, vec![(4, "empty-handler"), (13, "empty-handler")]);
    }

    #[test]
    fn python_expect_raise_with_a_failing_try_body_is_not_a_site() {
        let got = sites(
            "pkg/check.py",
            "def _self_test():\n    try:\n        calculate(-1)\n        assert False, \"expected ValueError\"\n    except ValueError:\n        pass\n    try:\n        calculate(-1)\n        pytest.fail(\"no raise\")\n    except ValueError:\n        pass\n    try:\n        calculate(-1)\n        assert result\n    except ValueError:\n        pass\n",
        );
        // An ordinary assertion at the end of the body can pass: that handler swallows.
        assert_eq!(got, vec![(15, "empty-handler")]);
    }

    #[test]
    fn python_expect_error_helper_returning_on_the_error_is_not_a_site() {
        let got = sites(
            "scripts/drive.py",
            "def self_check():\n    def expect_error(fn, what):\n        try:\n            fn()\n        except InstrumentError:\n            return\n        raise AssertionError(f\"expected InstrumentError: {what}\")\n\n    def parse(v):\n        try:\n            return int(v)\n        except ValueError:\n            return\n\n",
        );
        // A bare `return` followed by a failure after the `try` is the passing path; the same
        // `return` with nothing failing after it swallows.
        assert_eq!(got, vec![(12, "empty-handler")]);
    }

    #[test]
    fn python_parse_filters_in_a_loop_are_skipped_input() {
        let got = sites(
            "scripts/parse.py",
            "import json\n\ndef rows(lines):\n    out = []\n    for line in lines:\n        try:\n            out.append(json.loads(line))\n        except json.JSONDecodeError:\n            continue  # banner lines\n    for tok in lines:\n        try:\n            out.append(int(tok))\n        except (ValueError, OSError):\n            continue\n    try:\n        out.append(json.loads(lines[0]))\n    except ValueError:\n        pass\n    for line in lines:\n        try:\n            out.append(float(line))\n        except ValueError:\n            pass\n    for line in lines:\n        if line:\n            try:\n                out.append(int(line))\n            except ValueError:\n                continue\n    return out\n",
        );
        // Parse filters: the first, and the fifth, whose `try` sits in an `if` in the loop.
        // The second also drops an OSError, the third does not `continue` (and is outside a
        // loop), the fourth does not `continue`.
        assert_eq!(
            got,
            vec![
                (8, "skipped-input"),
                (13, "empty-handler"),
                (17, "empty-handler"),
                (22, "empty-handler"),
                (28, "skipped-input")
            ]
        );
    }

    #[test]
    fn python_interrupt_and_exit_handlers_are_not_error_handlers() {
        let got = sites(
            "pkg/cli.py",
            "def watch():\n    try:\n        run()\n    except SystemExit:\n        pass\n    try:\n        loop()\n    except KeyboardInterrupt:\n        print('stopped')\n    try:\n        run()\n    except (SystemExit, KeyboardInterrupt) as e:\n        pass\n    try:\n        run()\n    except (SystemExit, OSError):\n        pass\n    try:\n        run()\n    except BaseException:\n        pass\n",
        );
        // A signal alone is a stop request; one mixed with an error type, or the
        // `BaseException` that covers errors too, still swallows.
        assert_eq!(got, vec![(16, "empty-handler"), (20, "empty-handler")]);
    }

    #[test]
    fn javascript_and_java_empty_catch() {
        let js = sites(
            "src/a.ts",
            "async function f() {\n  try { await g(); } catch (e) {}\n  try { await g(); } catch (e) { console.error(e); throw e; }\n  try { await g(); } catch { return null; }\n  try { await g(); } catch (e) {} // best effort\n}\n",
        );
        assert_eq!(
            js,
            vec![
                (2, "empty-handler"),
                (4, "empty-handler"),
                (5, "empty-handler")
            ]
        );
        let java = sites(
            "src/main/java/A.java",
            "class A {\n  void f() {\n    try { g(); } catch (IOException e) { }\n    try { g(); } catch (IOException e) { throw new RuntimeException(e); }\n  }\n}\n",
        );
        assert_eq!(java, vec![(3, "empty-handler")]);
    }

    #[test]
    fn rust_and_go_discarded_results() {
        let rs = sites(
            "src/a.rs",
            "fn f() {\n    let _ = file.sync_all();\n    tx.commit().ok();\n    let _guard = lock();\n    let v = parse(s).ok();\n}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() { let _ = f(); }\n}\n",
        );
        assert_eq!(rs, vec![(2, "discarded-result"), (3, "discarded-result")]);
        let go = sites(
            "pkg/a.go",
            "package a\nfunc F() {\n    n, _ := w.Write(b)\n    _ = err\n    n, err := w.Write(b)\n    _ = n\n}\n",
        );
        assert_eq!(go, vec![(3, "discarded-result"), (4, "discarded-result")]);
    }

    #[test]
    fn rust_discards_are_sorted_by_callee_name() {
        // expanse #997: `get_or_init` returns `&Shards`; nothing fallible is discarded.
        let rs = sites(
            "src/alloc.rs",
            "fn f(&self, w: &mut W) {\n    let _ = self.shards.get_or_init(|| {\n        build()\n    });\n    let _ = file.sync_all();\n    let _ = tx.send(());\n    let _ = writeln!(w, \"x\");\n    let _ = self.lookup(k);\n    let _ = map.entry(k).or_default();\n    let _ = n.checked_add(1);\n    let _ = h.try_reserve(8);\n    let _ = fs::remove_file(p);\n    let _ = std::fs::read_to_string::<&str>(p);\n    let _ = fetch().await;\n}\n",
        );
        assert_eq!(
            rs,
            vec![
                (5, "discarded-result"),
                (6, "discarded-result"),
                (7, "discarded-result"),
                (8, "discarded-value"),
                (10, "discarded-result"),
                (11, "discarded-result"),
                (12, "discarded-result"),
                (13, "discarded-result"),
                (14, "discarded-value"),
            ]
        );
    }

    #[test]
    fn go_and_c_discards_are_sorted_by_callee_name() {
        let go = sites(
            "pkg/a.go",
            "package a\nfunc F() {\n\tn, _ := w.Write(b)\n\tv, _ := cache.Load(k)\n\ts, _ := x.(string)\n\tm, _ := lookup(k)\n\ti, _ := strconv.Atoi(s)\n\t_ = err\n}\n",
        );
        assert_eq!(
            go,
            vec![
                (3, "discarded-result"),
                (6, "discarded-value"),
                (7, "discarded-result"),
                (8, "discarded-result"),
            ]
        );
        let c = sites(
            "src/a.c",
            "void f(int fd) {\n    (void)write(fd, b, n);\n    (void)snprintf(buf, 8, \"x\");\n    (void)fclose(fp);\n    (void)x;\n}\n",
        );
        assert_eq!(
            c,
            vec![
                (2, "discarded-result"),
                (3, "discarded-value"),
                (4, "discarded-result")
            ]
        );
    }

    #[test]
    fn php_ruby_and_c_cpp_handlers_silences_and_discards() {
        let php = sites(
            "src/Loader.php",
            "<?php\nfunction load($p) {\n    try { g(); } catch (\\Throwable $e) { }\n    try { g(); } catch (E $e) { return null; }\n    try { g(); } catch (E $e) { log($e); throw $e; }\n    $x = @file_get_contents($p);\n    @unlink($p);\n    return $x;\n}\n/** @test */\nfunction testLoad() { try { load('x'); } catch (E $e) { } $y = @g(); }\n",
        );
        assert_eq!(
            php,
            vec![
                (3, "empty-handler"),
                (4, "empty-handler"),
                (6, "silenced-error"),
                (7, "silenced-error")
            ]
        );
        let tested = sites(
            "scripts/clean.php",
            "<?php\nif (!@chdir($d)) { exit(2); }\n$ok = @mkdir($d) || is_dir($d);\n@is_file($f) && @unlink($f);\nif (@file_get_contents($f) === false) { exit(1); }\n$n = @filesize($f) ?: 0;\n$s = (string) @file_get_contents($f);\nwhile (@ob_end_clean()) {}\n$t = @stat($f) ? 1 : 0;\n",
        );
        assert_eq!(
            tested,
            vec![
                (4, "silenced-error"),
                (6, "silenced-error"),
                (7, "silenced-error")
            ]
        );
        let rb = sites(
            "lib/loader.rb",
            "def load(p)\n  begin\n    g\n  rescue Foo::Bar => e\n  end\n  begin\n    g\n  rescue => e\n    nil\n  rescue Other\n    log(e)\n    raise\n  end\n  x = File.read(p) rescue nil\n  y = File.read(p) rescue fallback(p)\n  z = g rescue []\nrescue\n  # nothing\nend\n\ndef check\n  begin\n    g\n  rescue Bad\n  else\n    raise 'did not raise'\n  end\nend\n\ndef test_load\n  begin\n    load('x')\n  rescue\n  end\nend\n",
        );
        assert_eq!(
            rb,
            vec![
                (4, "empty-handler"),
                (8, "empty-handler"),
                (14, "silenced-error"),
                (16, "silenced-error"),
                (17, "empty-handler")
            ]
        );
        let cpp = sites(
            "src/a.cpp",
            "int c() {\n  try { g(); } catch (const std::exception& e) { }\n  try { g(); } catch (...) { return false; }\n  try { g(); } catch (E& e) { log(e); throw; }\n  (void)write(1, \"x\", 1);\n  (void)unused;\n  return 1;\n}\nTEST(S, N) { try { g(); } catch (...) { } (void)g(); }\n",
        );
        assert_eq!(
            cpp,
            vec![
                (2, "empty-handler"),
                (3, "empty-handler"),
                (5, "discarded-result")
            ]
        );
        let c = sites(
            "src/a.c",
            "int c(void) {\n  (void)write(1, \"x\", 1);\n  (void)unused;\n  return 1;\n}\n",
        );
        assert_eq!(c, vec![(2, "discarded-result")]);
    }

    #[test]
    fn kotlin_catch_blocks_and_run_catching() {
        let kt = sites(
            "src/main/kotlin/Loader.kt",
            "fun peek(p: String): String? {\n    try { g() } catch (e: Exception) { }\n    try { g() } catch (e: E) { null }\n    try { g() } catch (e: E) { /* later */ }\n    try { g() } catch (e: IOException) { log(e); throw e }\n    val x = runCatching { read(p) }.getOrNull()\n    val y = runCatching { read(p) }.getOrDefault(\"\")\n    val z = runCatching { read(p) }.getOrElse { log(it); throw it }\n    return x\n}\n",
        );
        assert_eq!(
            kt,
            vec![
                (2, "empty-handler"),
                (3, "empty-handler"),
                (4, "empty-handler"),
                (6, "silenced-error"),
                (7, "silenced-error")
            ]
        );
        let test = sites(
            "src/test/kotlin/LoaderTest.kt",
            "class LoaderTest {\n    @Test\n    fun t() {\n        try { g() } catch (e: Exception) { }\n        runCatching { g() }.getOrNull()\n    }\n}\n",
        );
        assert!(test.is_empty(), "{test:?}");
    }

    #[test]
    fn a_handler_that_only_logs_swallows_and_one_that_logs_then_acts_does_not() {
        let py = sites(
            "pkg/a.py",
            "def f():\n    try:\n        g()\n    except ValueError as e:\n        log.warning(e)\n    try:\n        g()\n    except OSError as e:\n        logger.error(\"failed: %s\", e)\n        print(e)\n    try:\n        g()\n    except KeyError as e:\n        log.error(e)\n        raise\n    try:\n        g()\n    except IOError as e:\n        log.error(e)\n        return None\n",
        );
        assert_eq!(py, vec![(4, "logging-handler"), (8, "logging-handler")]);
        let js = sites(
            "src/a.ts",
            "async function f() {\n  try { await g(); } catch (e) { console.error(e); }\n  try { await g(); } catch (e) { console.error(e); throw e; }\n  try { await g(); } catch (e) { console.error(e); state.failed = true; }\n}\n",
        );
        assert_eq!(js, vec![(2, "logging-handler")]);
        let java = sites(
            "src/main/java/A.java",
            "class A {\n  void f() {\n    try { g(); } catch (IOException e) { log.warn(\"x\", e); }\n    try { g(); } catch (IOException e) { LOG.warn(e); metrics.inc(); }\n  }\n}\n",
        );
        assert_eq!(java, vec![(3, "logging-handler")]);
        let kt = sites(
            "src/main/kotlin/A.kt",
            "fun f() {\n    try { g() } catch (e: Exception) { println(e) }\n    try { g() } catch (e: Exception) { log.error(e); throw e }\n}\n",
        );
        assert_eq!(kt, vec![(2, "logging-handler")]);
    }

    #[test]
    fn python_default_literal_returns_swallow_and_a_computed_value_does_not() {
        let src = "def f():\n    try:\n        g()\n    except A:\n        return []\n    try:\n        g()\n    except B:\n        return {}\n    try:\n        g()\n    except C:\n        return 0\n    try:\n        g()\n    except D:\n        return \"\"\n    try:\n        g()\n    except E:\n        return False\n    try:\n        g()\n    except F:\n        return  ''\n    try:\n        g()\n    except G:\n        return compute(1)\n    try:\n        g()\n    except H:\n        return {\"error\": 1}\n    try:\n        g()\n    except I:\n        return 1\n    try:\n        g()\n    except J as e:\n        return str(e)\n";
        let got = sites("pkg/a.py", src);
        let lines: Vec<usize> = got.iter().map(|(l, _)| *l).collect();
        assert_eq!(lines, vec![4, 8, 12, 16, 20, 24], "{got:?}");
    }

    #[test]
    fn js_ts_default_literal_returns_swallow_and_a_computed_value_does_not() {
        let src = "function f() {\n  try { g(); } catch (e) { return []; }\n  try { g(); } catch (e) { return {}; }\n  try { g(); } catch (e) { return 0; }\n  try { g(); } catch (e) { return \"\"; }\n  try { g(); } catch (e) { return ''; }\n  try { g(); } catch (e) { return  []  }\n  try { g(); } catch (e) { return fallback(e); }\n  try { g(); } catch (e) { return { error: e }; }\n  try { g(); } catch (e) { return 1; }\n}\n";
        let lines: Vec<usize> = sites("src/a.ts", src).iter().map(|(l, _)| *l).collect();
        assert_eq!(lines, vec![2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn js_ts_promise_catch_with_an_empty_or_default_callback_is_silenced() {
        let src = "async function f(p) {\n  p.catch(() => {});\n  p.catch(() => null);\n  p.catch(() => []);\n  p.catch(function () {});\n  p.catch(() => undefined);\n  p.catch(() => ({}));\n  p.catch((e) => { return null; });\n  await p.then(go).catch(() => false);\n  p.catch(async () => {});\n  p?.catch(() => 0);\n  p.catch(function (e) { /* ignore */ });\n}\n";
        let got = sites("src/a.ts", src);
        assert!(got.iter().all(|(_, k)| *k == "silenced-error"), "{got:?}");
        let lines: Vec<usize> = got.iter().map(|(l, _)| *l).collect();
        assert_eq!(lines, vec![2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
        // The same in a plain `.js` file.
        let js = sites("src/a.js", "p.catch(() => {});\n");
        assert_eq!(js, vec![(1, "silenced-error")]);
    }

    #[test]
    fn js_ts_promise_catch_that_handles_the_rejection_is_not_silenced() {
        let src = "async function f(p, q) {\n  p.catch((err) => handle(err));\n  p.catch((err) => { handle(err); });\n  p.catch((e) => { throw e; });\n  p.catch(() => compute());\n  p.catch(() => 1);\n  p.catch(() => ({ ok: false }));\n  p.catch(noop);\n  p.catch();\n  p.catch(() => {}, extra);\n  p.then(() => {});\n  p.finally(() => {});\n  q.cache(() => {});\n  const catchIt = () => {};\n  p.then(ok, () => {});\n  p.catch(handle).then(() => {});\n  p.catch((e) => { console.error(e); throw e; });\n  p.catch((e) => { console.error(e); return compute(e); });\n  p.catch((e) => { console.error(e); handle(e); });\n  p.catch((e) => console.error(e) || compute(e));\n  p.catch((e) => (console.error(e), handle(e)));\n  p.catch((e) => { throw e; });\n  p.catch(console.error);\n}\n";
        let got = sites("src/a.ts", src);
        assert!(got.is_empty(), "{got:?}");
    }

    #[test]
    fn js_ts_promise_catch_that_only_logs_is_a_logged_and_dropped_error() {
        let src = "async function f(p, logger) {\n  p.catch((e) => console.error(e));\n  p.catch((e) => { console.error(e); });\n  p.catch(function (e) { logger.warn(e) });\n  p.catch((e) => { console.warn(\"a\"); console.error(e); });\n  p.catch(async (e) => logger.error(e));\n  await p.then(go).catch((e) => console.log(e));\n  p.catch((e) => {});\n}\n";
        let got = sites("src/a.ts", src);
        assert_eq!(
            got,
            vec![
                (2, "logging-handler"),
                (3, "logging-handler"),
                (4, "logging-handler"),
                (5, "logging-handler"),
                (6, "logging-handler"),
                (7, "logging-handler"),
                (8, "silenced-error"),
            ]
        );
        let js = sites("src/a.js", "p.catch((e) => console.error(e));\n");
        assert_eq!(js, vec![(1, "logging-handler")]);
        let test = sites(
            "src/a.test.ts",
            "it('x', async () => {\n  p.catch((e) => console.error(e));\n});\n",
        );
        assert!(test.is_empty(), "{test:?}");
    }

    #[test]
    fn js_ts_promise_catch_in_a_test_is_not_a_site() {
        let got = sites(
            "src/a.test.ts",
            "it('x', async () => {\n  p.catch(() => {});\n});\n",
        );
        assert!(got.is_empty(), "{got:?}");
    }

    #[test]
    fn java_csharp_php_and_cpp_default_literal_returns_swallow() {
        let java = sites(
            "src/main/java/A.java",
            "class A {\n  int f() {\n    try { g(); } catch (IOException e) { return 0; }\n    try { g(); } catch (IOException e) { return \"\"; }\n    try { g(); } catch (IOException e) { return Collections.emptyList(); }\n    try { g(); } catch (IOException e) { return List.of(); }\n    try { g(); } catch (IOException e) { return Optional.empty(); }\n    try { g(); } catch (IOException e) { return 1; }\n    try { g(); } catch (IOException e) { return List.of(e); }\n    return 2;\n  }\n}\n",
        );
        let lines: Vec<usize> = java.iter().map(|(l, _)| *l).collect();
        assert_eq!(lines, vec![3, 4, 5, 6, 7], "{java:?}");
        let cs = sites(
            "src/A.cs",
            "class A {\n  int F() {\n    try { G(); } catch (Exception e) { return 0; }\n    try { G(); } catch (Exception e) { return \"\"; }\n    try { G(); } catch (Exception e) { return string.Empty; }\n    try { G(); } catch (Exception e) { return default; }\n    try { G(); } catch (Exception e) { return Compute(e); }\n    return 2;\n  }\n}\n",
        );
        let lines: Vec<usize> = cs.iter().map(|(l, _)| *l).collect();
        assert_eq!(lines, vec![3, 4, 5, 6], "{cs:?}");
        let php = sites(
            "src/a.php",
            "<?php\nfunction f() {\n  try { g(); } catch (Exception $e) { return 0; }\n  try { g(); } catch (Exception $e) { return \"\"; }\n  try { g(); } catch (Exception $e) { return []; }\n  try { g(); } catch (Exception $e) { return array(); }\n  try { g(); } catch (Exception $e) { return $e->getCode(); }\n}\n",
        );
        let lines: Vec<usize> = php.iter().map(|(l, _)| *l).collect();
        assert_eq!(lines, vec![3, 4, 5, 6], "{php:?}");
        let cpp = sites(
            "src/a.cpp",
            "int f() {\n  try { g(); } catch (...) { return 0; }\n  try { g(); } catch (...) { return 1; }\n  try { g(); } catch (const E& e) { return code(e); }\n  return 2;\n}\n",
        );
        assert_eq!(cpp, vec![(2, "empty-handler")]);
    }
}

/// `constant-fallback` (#535): the handlers that put a numeric literal in place of the
/// result, per pack. Each case is one handler body between the pack's `OPEN` and `CLOSE`.
#[cfg(all(
    test,
    feature = "lang-python",
    feature = "lang-javascript",
    feature = "lang-java",
    feature = "lang-php",
    feature = "lang-ruby",
    feature = "lang-c",
    feature = "lang-cpp",
    feature = "lang-kotlin",
    feature = "lang-csharp",
    feature = "lang-scala",
    feature = "lang-swift",
    feature = "lang-objc"
))]
mod constant_tests {
    use super::ConstantSpec;
    use crate::ast::{default_registry, AssertVocabulary};

    /// One pack's cases: a production path, the text around a handler body, the bodies
    /// that are a constant fallback and the bodies that are not.
    struct Cases {
        path: &'static str,
        open: &'static str,
        close: &'static str,
        fallback: &'static [&'static str],
        other: &'static [&'static str],
        /// The text before and after a literal in a statement that assigns it to `x`.
        assign: (&'static str, &'static str),
        /// Spellings of zero in the language: assigned, none is a constant fallback.
        zeros: &'static [&'static str],
        /// Numbers close to those spellings that are not zero: each is one.
        near_zeros: &'static [&'static str],
    }

    const PYTHON: Cases = Cases {
        path: "pkg/a.py",
        open: "def f(rec, log):\n    try:\n        x = g()\n    except E as e:\n",
        close: "\n    return x\n",
        fallback: &[
            "        x = 150000.0",
            "        rec.ops = -1",
            "        rec.raw[\"ops\"] = 2.5e5",
            "        x: float = 150000.0",
            "        x = (3)",
            "        x = -1",
            "        x = 0x10",
            "        return 1",
            "        return -2.5",
            "        return {\"error\": 1}",
            "        return [1, 2.5]",
            "        return [1, 0]",
            "        return (1, 2)",
            "        return 1, 2",
            "        return {1, 2}",
            "        log.warning(e)\n        x = 150000.0",
            "        x = 150000.0\n        print(e)\n        rec.ops = 2",
            "        # the arm is missing\n        x = 150000.0",
        ],
        other: &[
            "        x = 0",
            "        return 0",
            "        x = None",
            "        x = float(\"nan\")",
            "        x = float('inf')",
            "        x = math.nan",
            "        x = True",
            "        x = \"150000\"",
            "        x = 150000.0\n        raise",
            "        raise RuntimeError(150000.0)",
            "        rec.error = e",
            "        return e",
            "        x = estimate()",
            "        mark_failed()\n        x = 150000.0",
            "        x = 150000.0 * 2",
            "        x = FALLBACK",
            "        x += 5",
            "        x, y = 1, 2",
            "        x = y = 5",
            "        rec.raw[slot()] = 5",
            "        slot().ops = 5",
            "        return [0, 0.0]",
            "        return {\"ops\": 0}",
            "        return [1, None]",
            "        return {\"ops\": e}",
            "        return {key(): 1}",
            "        return []",
            "        return [[1, 2]]",
            "        log.warning(e)",
            "        pass",
            "        x = 150000.0\n        return None",
        ],
        assign: ("        x = ", ""),
        zeros: &[
            "0", "0.0", "0.", ".0", "0e0", "0E0", "0x0", "0X0", "0b0", "0o0", "00", "0_0", "0.0_0",
            "0j",
        ],
        near_zeros: &[
            "0.1", "1e-9", "0x1", "0xf", "0b1", "0o7", "1_0", ".5", "1j", "-1",
        ],
    };

    const JS: Cases = Cases {
        path: "src/a.ts",
        open: "function f(rec) {\n  let x;\n  try { x = g(); } catch (e) {\n",
        close: "\n  }\n  return x;\n}\n",
        fallback: &[
            "    x = 150000.0;",
            "    rec.ops = -1;",
            "    rec.raw[\"ops\"] = 2.5e5;",
            "    x = (3);",
            "    return 1;",
            "    return 10n;",
            "    return [1, 0];",
            "    return { ops: 1, \"p99\": 2.5 };",
            "    return [1, -2];",
            "    console.error(e);\n    x = 150000.0;",
            "    // the arm is missing\n    x = 150000.0;",
        ],
        other: &[
            "    x = 0;",
            "    return 0;",
            "    x = null;",
            "    x = undefined;",
            "    x = NaN;",
            "    x = Number.NaN;",
            "    x = Infinity;",
            "    x = 150000.0;\n    throw e;",
            "    rec.error = e;",
            "    return e;",
            "    x = estimate();",
            "    markFailed();\n    x = 150000.0;",
            "    x = 150000.0 * 2;",
            "    x = FALLBACK;",
            "    x += 5;",
            "    let local = 5;",
            "    rec.raw[slot()] = 5;",
            "    return [0, 0];",
            "    return { error: e };",
            "    return { ...rec, ops: 1 };",
            "    return { [key()]: 1 };",
            "    return [];",
            "    console.error(e);",
        ],
        assign: ("    x = ", ";"),
        zeros: &[
            "0", "0.0", "0.", ".0", "0e0", "0x0", "0b0", "0o0", "0.0_0", "0n",
        ],
        near_zeros: &["0.1", "1e-9", "0x1", "0xf", "0b1", "0o7", ".5", "1n", "-1"],
    };

    const JAVA: Cases = Cases {
        path: "src/main/java/A.java",
        open: "class A {\n  double f() {\n    try { x = g(); } catch (IOException e) {\n",
        close: "\n    }\n    return x;\n  }\n}\n",
        fallback: &[
            "      x = 150000.0;",
            "      this.ops = -1;",
            "      raw[0] = 2.5e5;",
            "      return 1;",
            "      return 5L;",
            "      return 1.5f;",
            "      return 0x1F;",
            "      return new double[] {1.0, 2.5};",
            "      return new double[] {1.0, 0};",
            "      System.err.println(e);\n      x = 150000.0;",
        ],
        other: &[
            "      x = 0;",
            "      return 0;",
            "      boxed = null;",
            "      x = Double.NaN;",
            "      x = 150000.0;\n      throw new IllegalStateException(e);",
            "      lastError = e;",
            "      x = estimate();",
            "      markFailed();\n      x = 150000.0;",
            "      x = 150000.0 * 2;",
            "      x = FALLBACK;",
            "      x += 5;",
            "      double local = 5;",
            "      raw[slot()] = 5;",
            "      return new double[] {0, 0.0};",
            "      return new double[2];",
            "      return List.of(1.0, 2.5);",
        ],
        assign: ("      x = ", ";"),
        zeros: &[
            "0", "0.0", "0.", ".0", "0e0", "0x0", "0b0", "00", "0_0", "0L", "0l", "0f", "0.0f",
            "0d", "0D", "0x0L",
        ],
        near_zeros: &[
            "0.1", "1e-9", "0x1", "0xf", "0x0d", "0b1", "01", "1L", "0.1f", "1d", "-1",
        ],
    };

    const PHP: Cases = Cases {
        path: "src/a.php",
        open: "<?php\nfunction f($rec) {\n  try { $x = g(); } catch (E $e) {\n",
        close: "\n  }\n  return $x;\n}\n",
        fallback: &[
            "    $x = 150000.0;",
            "    $rec->ops = -1;",
            "    $rec->raw['ops'] = 2.5e5;",
            "    self::$ops = 5;",
            "    return 1;",
            "    return ['ops' => 1, 2.5];",
            "    return array(1, -2);",
            "    return [1, 0];",
            "    error_log($e);\n    $x = 150000.0;",
        ],
        other: &[
            "    $x = 0;",
            "    return 0;",
            "    $x = null;",
            "    $x = NAN;",
            "    $x = 150000.0;\n    throw $e;",
            "    $rec->error = $e;",
            "    return $e;",
            "    $x = estimate();",
            "    mark_failed();\n    $x = 150000.0;",
            "    $x = 150000.0 * 2;",
            "    $x = FALLBACK;",
            "    $x += 5;",
            "    $rec->raw[slot()] = 5;",
            "    return [0, 0.0];",
            "    return [key() => 1];",
            "    return [];",
        ],
        assign: ("    $x = ", ";"),
        zeros: &[
            "0", "0.0", "0.", ".0", "0e0", "0x0", "0b0", "0o0", "00", "0_0",
        ],
        near_zeros: &["0.1", "1e-9", "0x1", "0xf", "0b1", "01", "-1"],
    };

    const RUBY: Cases = Cases {
        path: "lib/a.rb",
        open: "def f(rec)\n  begin\n    x = g\n  rescue E => e\n",
        close: "\n  end\n  x\nend\n",
        fallback: &[
            "    x = 150000.0",
            "    rec.ops = -1",
            "    @raw[:ops] = 2.5e5",
            "    @last = 3",
            "    $last = 3",
            "    return 1",
            "    return { ops: 1, \"p99\" => 2.5 }",
            "    return [1, -2]",
            "    150000.0",
            "    [1, 2.5]",
            "    puts e\n    x = 150000.0",
        ],
        other: &[
            "    x = nil",
            "    x = Float::NAN",
            "    nil",
            "    x = 150000.0\n    raise",
            "    @error = e",
            "    return e",
            "    x = estimate(1)",
            "    mark_failed(1)\n    x = 150000.0",
            "    x = 150000.0 * 2",
            "    x = FALLBACK",
            "    x += 5",
            "    @raw[slot(1)] = 5",
            "    return [1, nil]",
            "    return 1, 2",
            "    return []",
            // A number that is not the handler's value does nothing.
            "    150000.0\n    x = nil",
        ],
        assign: ("    x = ", ""),
        zeros: &[
            "0", "0.0", "0e0", "0x0", "0b0", "0o0", "00", "0_0", "0r", "0i",
        ],
        near_zeros: &["0.1", "1e-9", "0x1", "0xf", "0b1", "01", "1r", "-1"],
    };

    const CPP: Cases = Cases {
        path: "src/a.cpp",
        open: "double f(Rec& rec, Rec* out) {\n  double x = 0;\n  try { x = g(); } catch (const E& e) {\n",
        close: "\n  }\n  return x;\n}\n",
        fallback: &[
            "    x = 150000.0;",
            "    rec.ops = -1;",
            "    out->raw[0] = 2.5e5;",
            "    ns::ops = 5;",
            "    x = - 1;",
            "    return 1;",
            "    return 1.5f;",
            "    return {1.0, 2.5};",
            "    return {1.0, 0};",
            "    std::cerr << \"failed\";\n    x = 150000.0;",
        ],
        other: &[
            "    x = 0;",
            "    return 0;",
            "    out = nullptr;",
            "    x = NAN;",
            "    x = std::nan(\"\");",
            "    x = 150000.0;\n    throw;",
            "    rec.error = e;",
            "    x = estimate();",
            "    mark_failed();\n    x = 150000.0;",
            "    x = 150000.0 * 2;",
            "    x = kFallback;",
            "    x += 5;",
            "    double local = 5;",
            "    out->raw[slot()] = 5;",
            "    return {0, 0.0};",
            "    return {};",
        ],
        assign: ("    x = ", ";"),
        zeros: &["0", "0.0", "0.", ".0", "0e0", "0x0", "0b0", "00", "0'0", "0L", "0u", "0ULL", "0.0f", "0.f", "0x0p0"],
        near_zeros: &["0.1", "1e-9", "0x1", "0xf", "0x0f", "0b1", "01", "1u", "0.1f", "0x1p0", "-1"],
    };

    const KOTLIN: Cases = Cases {
        path: "src/main/kotlin/A.kt",
        open: "fun f(rec: Rec): Double {\n    var x = 0.0\n    try { x = g() } catch (e: E) {\n",
        close: "\n    }\n    return x\n}\n",
        fallback: &[
            "        x = 150000.0",
            "        rec.ops = -1.0",
            "        rec.raw[0] = 2.5e5",
            "        return 1.0",
            "        return 5L",
            "        150000.0",
            "        println(e)\n        x = 150000.0",
        ],
        other: &[
            "        boxed = null",
            "        x = Double.NaN",
            "        null",
            "        x = 150000.0\n        throw e",
            "        rec.error = e",
            "        x = estimate()",
            "        markFailed()\n        x = 150000.0",
            "        x = 150000.0 * 2",
            "        x = FALLBACK",
            "        x += 5.0",
            "        val local = 5.0",
            "        rec.raw[slot()] = 5.0",
            "        return listOf(1.0, 2.5)",
        ],
        assign: ("        x = ", ""),
        zeros: &[
            "0", "0.0", "0e0", "0x0", "0b0", "0_0", "0L", "0f", "0.0f", "0u", "0UL",
        ],
        near_zeros: &["0.1", "1e-9", "0x1", "0xf", "0b1", "1L", "0.1f", "1u", "-1"],
    };

    const CSHARP: Cases = Cases {
        path: "src/A.cs",
        open: "class A {\n  double F() {\n    try { x = G(); } catch (Exception e) {\n",
        close: "\n    }\n    return x;\n  }\n}\n",
        fallback: &[
            "      x = 150000.0;",
            "      this.ops = -1;",
            "      raw[0] = 2.5e5;",
            "      return 1;",
            "      return 1.5m;",
            "      return new double[] {1.0, 2.5};",
            "      return new[] {1.0, 2.5};",
            "      return [1.0, 2.5];",
            "      return new double[] {1.0, 0};",
            "      Console.WriteLine(e);\n      x = 150000.0;",
        ],
        other: &[
            "      x = 0;",
            "      return 0;",
            "      boxed = null;",
            "      x = double.NaN;",
            "      x = 150000.0;\n      throw;",
            "      lastError = e;",
            "      x = Estimate();",
            "      MarkFailed();\n      x = 150000.0;",
            "      x = 150000.0 * 2;",
            "      x = Fallback;",
            "      x += 5;",
            "      double local = 5;",
            "      raw[Slot()] = 5;",
            "      return new double[] {0, 0.0};",
            "      return new Rec { Ops = 1.0 };",
            "      return default;",
        ],
        assign: ("      x = ", ";"),
        zeros: &[
            "0", "0.0", ".0", "0e0", "0x0", "0b0", "0_0", "0L", "0f", "0.0f", "0u", "0m", "0d",
            "0UL",
        ],
        near_zeros: &[
            "0.1", "1e-9", "0x1", "0xf", "0x0d", "0b1", "1L", "0.1f", "1m", "-1",
        ],
    };

    const SCALA: Cases = Cases {
        path: "src/main/scala/A.scala",
        open: "object A {\n  def f(rec: Rec): Double = {\n    var x = 0.0\n    try { x = g() } catch {\n      case e: E =>\n",
        close: "\n    }\n    x\n  }\n}\n",
        fallback: &[
            "        x = 150000.0",
            "        rec.ops = -1",
            "        150000.0",
            "        -1",
            "        return 1.5",
            "        (1.5, 2.5)",
            "        (1.5, 0)",
            "        { x = 150000.0 }",
            "        println(e)\n        x = 150000.0",
        ],
        other: &[
            "        x = 0",
            "        0",
            "        None",
            "        x = Double.NaN",
            "        throw e",
            "        x = 150000.0\n        throw e",
            "        rec.error = e",
            "        x = estimate()",
            "        markFailed()\n        x = 150000.0",
            "        x = 150000.0 * 2",
            "        x = Fallback",
            "        x += 5",
            "        d(0) = 3",
            "        List(1.5, 2.5)",
            "        (0, 0.0)",
        ],
        assign: ("        x = ", ""),
        zeros: &["0", "0.0", "0e0", "0x0", "0L", "0f", "0.0f", "0d", "0_0", "-0", "-0.0"],
        near_zeros: &["0.1", "1e-9", "0x1", "0xf", "1L", "0.1f", "-1"],
    };

    const SWIFT: Cases = Cases {
        path: "Sources/App/A.swift",
        open: "func f(rec: Rec) -> Double {\n    var x = 0.0\n    do { x = try g() } catch {\n",
        close: "\n    }\n    return x\n}\n",
        fallback: &[
            "        x = 150000.0",
            "        rec.ops = -1",
            "        self.ops = 0x10",
            "        return 1.5",
            "        return [1.5, 2.5]",
            "        return [1.5, 0]",
            "        return [\"ops\": 1.5, \"p99\": 2.5]",
            "        print(error)\n        x = 150000.0",
        ],
        other: &[
            "        boxed = nil",
            "        x = Double.nan",
            "        x = .nan",
            "        x = 150000.0\n        throw error",
            "        throw error",
            "        rec.error = error",
            "        x = estimate()",
            "        markFailed()\n        x = 150000.0",
            "        x = 150000.0 * 2",
            "        x = fallback",
            "        x += 5",
            "        let local = 5.0",
            "        raw[0] = 5.0",
            "        return [1.5, nil]",
            "        return [0, 0.0]",
            "        return [key(): 1.5]",
            "        return []",
        ],
        assign: ("        x = ", ""),
        zeros: &["0", "0.0", "0e0", "0x0", "0b0", "0o0", "0_0", "00"],
        near_zeros: &["0.1", "1e-9", "0x1", "0xf", "0b1", "0o7", "01", "-1"],
    };

    const OBJC: Cases = Cases {
        path: "Sources/a.m",
        open:
            "double f(Rec *rec) {\n  double x = 0;\n  @try { x = g(); } @catch (NSException *e) {\n",
        close: "\n  }\n  return x;\n}\n",
        fallback: &[
            "    x = 150000.0;",
            "    rec.ops = -1;",
            "    rec->raw[0] = 2.5e5;",
            "    return 1;",
            "    return @5;",
            "    return @[@1, @2.5];",
            "    return @[@1, @0];",
            "    NSLog(@\"failed\");\n    x = 150000.0;",
        ],
        other: &[
            "    x = 0;",
            "    return 0;",
            "    boxed = nil;",
            "    x = NAN;",
            "    x = 150000.0;\n    @throw e;",
            "    rec.error = e;",
            "    x = estimate();",
            "    [rec markFailed];\n    x = 150000.0;",
            "    x = 150000.0 * 2;",
            "    x = kFallback;",
            "    x += 5;",
            "    rec->raw[slot()] = 5;",
            "    return @[@0, @0.0];",
            "    return @(estimate());",
        ],
        assign: ("    x = ", ";"),
        zeros: &[
            "0", "0.0", "0.", ".0", "0e0", "0x0", "00", "0L", "0u", "0.0f", "@0",
        ],
        near_zeros: &["0.1", "1e-9", "0x1", "0xf", "01", "1u", "0.1f", "@1", "-1"],
    };

    const ALL: &[&Cases] = &[
        &PYTHON, &JS, &JAVA, &PHP, &RUBY, &CPP, &KOTLIN, &CSHARP, &SCALA, &SWIFT, &OBJC,
    ];

    /// The constant-fallback lines and the number of swallow sites of `src`.
    fn read(path: &str, src: &str) -> (Vec<usize>, usize) {
        let reg = default_registry();
        let pack = reg.find_pack(path).unwrap();
        let facts = pack
            .extract(path, src, &AssertVocabulary::default())
            .unwrap();
        assert!(
            facts
                .constant_fallbacks
                .iter()
                .all(|s| s.kind == "constant-fallback"),
            "{:?}",
            facts.constant_fallbacks
        );
        (
            facts.constant_fallbacks.iter().map(|s| s.line).collect(),
            facts.swallowed.len(),
        )
    }

    #[test]
    fn a_handler_that_stores_or_returns_a_number_is_a_constant_fallback_in_every_pack() {
        for cases in ALL {
            let handler_line = cases.open.lines().count();
            for body in cases.fallback {
                let src = format!("{}{body}{}", cases.open, cases.close);
                let (lines, swallowed) = read(cases.path, &src);
                assert_eq!(lines, vec![handler_line], "{}:\n{src}", cases.path);
                // Never also one of the gate's other sites.
                assert_eq!(swallowed, 0, "{}:\n{src}", cases.path);
            }
        }
    }

    #[test]
    fn a_handler_that_handles_or_marks_absence_is_not_a_constant_fallback_in_any_pack() {
        for cases in ALL {
            for body in cases.other {
                let src = format!("{}{body}{}", cases.open, cases.close);
                let (lines, _) = read(cases.path, &src);
                assert!(lines.is_empty(), "{}: {lines:?}\n{src}", cases.path);
            }
        }
    }

    /// A zero is a default in every pack, whatever its spelling, plain or under a minus;
    /// a number near one of those spellings is not, and `-1` is a number like any other.
    #[test]
    fn a_zero_in_any_spelling_is_not_a_constant_fallback_and_a_number_near_it_is() {
        for cases in ALL {
            let handler_line = cases.open.lines().count();
            let assigned = |literal: &str| {
                let src = format!(
                    "{}{}{literal}{}{}",
                    cases.open, cases.assign.0, cases.assign.1, cases.close
                );
                read(cases.path, &src).0
            };
            for zero in cases.zeros {
                for literal in [zero.to_string(), format!("-{zero}")] {
                    assert!(
                        assigned(&literal).is_empty(),
                        "{}: `{literal}` is a zero",
                        cases.path
                    );
                }
            }
            for near in cases.near_zeros {
                assert_eq!(
                    assigned(near),
                    vec![handler_line],
                    "{}: `{near}` is not a zero",
                    cases.path
                );
            }
        }
    }

    const ZERO_TEXTS: &[&str] = &[
        "0", "0.0", "0.", ".0", "0e0", "0E5", "0x0", "0X0", "0b0", "0o0", "00", "0_0", "0'0", "0L",
        "0f", "0.0f", "0u", "0m", "0d", "0n", "0r", "0j", "0UL", "0x0L", "0x0p0", "-0", "-0.0",
        "+0", "- 0",
    ];
    const NON_ZERO_TEXTS: &[&str] = &[
        "1", "-1", "0.1", ".5", "1e-9", "0x1", "0xf", "0x0f", "0x0d", "0b1", "0o7", "007", "1L",
        "0.1f", "10", "1_0", "150000.0", "0x1p0", "",
    ];

    #[test]
    fn a_zero_is_read_by_its_value_not_by_its_text() {
        for t in ZERO_TEXTS {
            assert!(super::is_zero_literal(t), "`{t}` is a zero");
        }
        for t in NON_ZERO_TEXTS {
            assert!(!super::is_zero_literal(t), "`{t}` is not a zero");
        }
    }

    const RUBY_ZERO: &str = "def f\n  begin\n    x = g\n  rescue E\n    x = 0\n  end\n  x\nend\n";
    const KOTLIN_ZERO: &str =
        "fun f(): Double {\n    try { return g() } catch (e: E) {\n        return 0\n    }\n}\n";
    const SWIFT_ZERO: &str =
        "func f() -> Double {\n    do { return try g() } catch {\n        return 0.0\n    }\n}\n";
    const PYTHON_ZERO: &str =
        "def f():\n    try:\n        return g()\n    except E:\n        return 0\n";

    /// A zero the gate's existing default list does not hold is reported by neither
    /// finding; one it holds stays the existing finding's and is not a constant fallback.
    #[test]
    fn a_zero_is_left_to_the_existing_finding_or_to_nothing() {
        assert_eq!(read("lib/a.rb", RUBY_ZERO), (vec![], 0));
        assert_eq!(read("src/main/kotlin/A.kt", KOTLIN_ZERO), (vec![], 0));
        assert_eq!(read("Sources/App/A.swift", SWIFT_ZERO), (vec![], 0));
        assert_eq!(read("pkg/a.py", PYTHON_ZERO), (vec![], 1));
    }

    /// The source of `python_default_literal_returns_swallow_and_a_computed_value_does_not`.
    const PYTHON_BOUNDARY: &str = "def f():\n    try:\n        g()\n    except A:\n        return []\n    try:\n        g()\n    except B:\n        return {}\n    try:\n        g()\n    except C:\n        return 0\n    try:\n        g()\n    except D:\n        return \"\"\n    try:\n        g()\n    except E:\n        return False\n    try:\n        g()\n    except F:\n        return  ''\n    try:\n        g()\n    except G:\n        return compute(1)\n    try:\n        g()\n    except H:\n        return {\"error\": 1}\n    try:\n        g()\n    except I:\n        return 1\n    try:\n        g()\n    except J as e:\n        return str(e)\n";

    /// `return 1` and `return {"error": 1}` stay out of `swallowed` (the existing test
    /// pins that) and are the two constant fallbacks; the default literals stay swallow
    /// sites and are not constant fallbacks.
    #[test]
    fn the_two_lists_split_the_python_default_literal_cases_between_them() {
        let (fallbacks, swallowed) = read("pkg/a.py", PYTHON_BOUNDARY);
        assert_eq!(fallbacks, vec![32, 36]);
        assert_eq!(swallowed, 6);
    }

    const PYTHON_IN_A_TEST: &str = "def test_f():\n    try:\n        x = g()\n    except E:\n        x = 150000.0\n\n\ndef f():\n    try:\n        x = g()\n    except E:\n        x = 150000.0\n";
    const PYTHON_EXPECTED_ERROR: &str = "def check():\n    try:\n        g()\n        raise AssertionError(\"no error\")\n    except E:\n        seen = 1\n    try:\n        loop()\n    except KeyboardInterrupt:\n        code = 130\n";

    #[test]
    fn test_scope_and_the_expected_error_idiom_hold_for_a_constant_fallback() {
        // A handler in a test function is not read; the same one outside it is.
        assert_eq!(read("pkg/a.py", PYTHON_IN_A_TEST).0, vec![11]);
        // A whole test file is not read.
        assert!(read("tests/test_a.py", PYTHON_IN_A_TEST).0.is_empty());
        // A `try` that fails when nothing is raised, and a handler for a stop request.
        assert!(read("pkg/check.py", PYTHON_EXPECTED_ERROR).0.is_empty());
    }

    /// Every node kind a pack's `ConstantSpec` names exists in that pack's grammar: a
    /// misspelled kind would silently never match.
    #[test]
    fn every_kind_a_constant_spec_names_is_a_kind_of_its_grammar() {
        use tree_sitter::Language;
        let grammars: Vec<(&str, &ConstantSpec, Vec<Language>)> = vec![
            (
                "python",
                &crate::ast::python::PYTHON_CONSTANTS,
                vec![tree_sitter_python::LANGUAGE.into()],
            ),
            (
                "javascript",
                &crate::ast::javascript::JS_CONSTANTS,
                vec![
                    tree_sitter_javascript::LANGUAGE.into(),
                    tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                    tree_sitter_typescript::LANGUAGE_TSX.into(),
                ],
            ),
            (
                "java",
                &crate::ast::java::JAVA_CONSTANTS,
                vec![tree_sitter_java::LANGUAGE.into()],
            ),
            (
                "php",
                &crate::ast::php::PHP_CONSTANTS,
                vec![tree_sitter_php::LANGUAGE_PHP.into()],
            ),
            (
                "ruby",
                &crate::ast::ruby::RUBY_CONSTANTS,
                vec![tree_sitter_ruby::LANGUAGE.into()],
            ),
            (
                "c++",
                &crate::ast::c_cpp::C_CONSTANTS,
                vec![tree_sitter_cpp::LANGUAGE.into()],
            ),
            (
                "kotlin",
                &crate::ast::kotlin::KOTLIN_CONSTANTS,
                vec![tree_sitter_kotlin_ng::LANGUAGE.into()],
            ),
            (
                "c#",
                &crate::ast::csharp::CSHARP_CONSTANTS,
                vec![tree_sitter_c_sharp::LANGUAGE.into()],
            ),
            (
                "scala",
                &crate::ast::scala::SCALA_CONSTANTS,
                vec![tree_sitter_scala::LANGUAGE.into()],
            ),
            (
                "swift",
                &crate::ast::swift::SWIFT_CONSTANTS,
                vec![crate::ast::swift::grammar_for_node_kinds()],
            ),
            (
                "objective-c",
                &crate::ast::objc::OBJC_CONSTANTS,
                vec![tree_sitter_objc::LANGUAGE.into()],
            ),
        ];
        for (name, spec, languages) in grammars {
            let lists = [
                spec.blocks,
                spec.wrappers,
                spec.numbers,
                spec.signs,
                spec.assignments,
                spec.targets,
                spec.calls,
                spec.returns,
                spec.collections,
                spec.collection_holders,
                spec.pairs,
                spec.keys,
            ];
            for language in &languages {
                for kind in lists.iter().flat_map(|l| l.iter()) {
                    assert_ne!(
                        language.id_for_node_kind(kind, true),
                        0,
                        "{name}: `{kind}` is not a named node kind of the grammar"
                    );
                }
            }
        }
    }
}
