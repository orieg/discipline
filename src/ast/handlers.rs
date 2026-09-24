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
    /// `empty-handler` or `discarded-result`.
    pub kind: &'static str,
    /// The handler's first line, trimmed.
    pub snippet: String,
}

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
            if inner.is_empty() || spec.trivial.contains(&inner) {
                Some("empty-handler")
            } else if is_logging_statement(inner) {
                Some("logging-handler")
            } else {
                None
            }
        }
        1 => {
            let t = text(stmts[0], src).trim().trim_end_matches(';').trim();
            if spec.trivial.contains(&t) {
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

/// Sites in `root`, in source order. `is_test_line` excludes handlers inside tests.
pub fn extract(
    root: Node,
    src: &str,
    spec: &HandlerSpec,
    is_test_line: &dyn Fn(usize) -> bool,
) -> Vec<SwallowSite> {
    let mut out = Vec::new();
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
                    if after.is_empty() || spec.trivial.contains(&after) {
                        Some("empty-handler")
                    } else if is_logging_statement(after) {
                        Some("logging-handler")
                    } else {
                        None
                    }
                }
            };
            if let Some(kind) = swallows
                .filter(|_| !expects_the_error(node, src) && !catches_only_signals(node, src))
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
            if (spec.silences)(t) && !result_is_tested(node, src) {
                out.push(SwallowSite {
                    line,
                    kind: "silenced-error",
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
    out
}

/// The expect-this-to-raise idiom: the handler is the passing path and the code around it
/// fails when nothing was raised. Either the `try` has an `else` that raises or fails, or
/// the handler is `continue` / `pass` and the statement after the `try` records a failure.
fn expects_the_error(handler: Node, src: &str) -> bool {
    let Some(try_stmt) = handler.parent() else {
        return false;
    };
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
        .all(|l| l.is_empty() || l == "continue" || l == "pass" || l == "...");
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

pub fn no_discard(_: &str) -> bool {
    false
}

/// PHP: `@call()` silences every error the call raises. The node kind is exact
/// (`error_suppression_expression`), so any text qualifies.
pub fn php_silences(_: &str) -> bool {
    true
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
/// `f`'s last value, sorted by `f`'s name. A type assertion, map index or channel
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
            sort_by_callee(name, GO_FALLIBLE_CALLEES, GO_OK_CALLEES)
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

/// Go: `_ = err`, `_, _ = f()`, `x, _ := f()` where the dropped value is the error.
pub fn go_discards(t: &str) -> bool {
    let t = t.trim();
    if t == "_ = err" || t.starts_with("_ = err") {
        return true;
    }
    // A trailing `_` on the left of `=` / `:=` of a multi-value call drops the last value.
    if let Some((lhs, rhs)) = t.split_once([':', '=']) {
        let lhs = lhs.trim().trim_end_matches(':').trim();
        let rhs = rhs.trim_start_matches('=').trim();
        return lhs.contains(',') && lhs.ends_with('_') && rhs.contains('(');
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
    feature = "lang-kotlin"
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
}
