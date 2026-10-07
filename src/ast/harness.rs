//! Forms by which code the test runner loads makes a failing run report success.
//!
//! The `harness-tampering` gate reads these from the syntax tree of a file the runner
//! loads (`src/guards/harness_tampering.rs` decides which files those are). Each form is
//! an exact shape of the tree. Nothing here follows a value from where it is made to
//! where it is used: a form written another way is not seen, and each reader says in
//! its own comment what it leaves out.
//!
//! - Go: a `TestMain` that never calls `m.Run()`, or calls it, drops the result and
//!   ends in an `os.Exit` that does not carry it ([`go_test_file`]).
//! - pytest: a report hook that assigns a report's `outcome`, and a collection hook
//!   that removes items and reads no marker, keyword or option ([`python`]).
//! - `unittest`: `addFailure`, `addError` or `wasSuccessful` assigned, or overridden in
//!   a `TestResult` subclass by a body that does nothing ([`python`]).
//! - An exit with status zero that no condition guards: `sys.exit(0)`, `os._exit(0)`
//!   ([`python`]), `process.exit(0)` ([`js_exit_zero`]), `std::process::exit(0)`
//!   ([`rust_exit_zero`]).

use super::ancestry::Ancestry;
use tree_sitter::Node;

/// The version of the forms read here. It is written into the gate's notes, and
/// changes whenever a form is added, removed or read differently.
pub const PATTERN_VERSION: u32 = 1;

/// One form of tampering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Form {
    /// A Go `TestMain` with no call of `m.Run()`.
    GoRunNeverCalled,
    /// A Go `TestMain` that calls `m.Run()`, drops its result, and exits.
    GoResultDiscarded,
    /// A pytest report hook assigns a report's `outcome`.
    PytestOutcomeAssigned,
    /// A pytest collection hook removes items and reads no marker, keyword or option.
    PytestItemsRemoved,
    /// A `unittest` result method is assigned.
    UnittestMethodAssigned,
    /// A `TestResult` subclass overrides a result method with a body that does nothing.
    UnittestMethodNeutralised,
    /// An exit with status zero that no condition guards.
    ExitZero,
}

/// One place a form was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    pub form: Form,
    /// 1-based line.
    pub line: usize,
    /// The function, hook or class the form is in; `<module>` at the top level.
    pub subject: String,
    /// What was read, as a clause that follows the file's name.
    pub what: String,
}

/// What one file holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scan {
    pub sites: Vec<Site>,
    /// What could not be judged, and why.
    pub notes: Vec<String>,
    /// The tree has error nodes: the file was read in part.
    pub parse_errors: bool,
}

/// Which of the Python forms to read in a file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PythonChecks {
    /// pytest hooks (a `conftest.py`).
    pub hooks: bool,
    /// Exits with status zero (a `conftest.py`, a `sitecustomize.py`).
    pub exit: bool,
    /// `unittest` result methods.
    pub unittest: bool,
}

fn text<'a>(node: Node, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

fn line(node: Node) -> usize {
    node.start_position().row + 1
}

/// Every node below `root`, `root` first, in source order. `stop` names the kinds whose
/// children are left out (the node itself is kept).
fn preorder<'a>(root: Node<'a>, stop: &[&str]) -> Vec<Node<'a>> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        out.push(node);
        if node.id() != root.id() && stop.contains(&node.kind()) {
            continue;
        }
        let mut cursor = node.walk();
        let children: Vec<Node<'a>> = node.children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    out
}

/// The named children of `node` that are not comments.
fn operands<'a>(node: Node<'a>) -> Vec<Node<'a>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|c| !c.kind().contains("comment"))
        .collect()
}

/// How one language spells what makes an exit conditional.
struct ExitSpec {
    /// An ancestor of one of these kinds makes the exit conditional: a branch, a loop,
    /// an exception handler.
    conditional: &'static [&'static str],
    /// The binary-expression kind and the operators that run their right side on a
    /// condition.
    short_circuit: (&'static str, &'static [&'static str]),
    /// Kinds that hold a function body: a guard clause is looked for inside one, and
    /// the nearest named one is the site's subject.
    functions: &'static [&'static str],
    /// Function kinds that are conditional when they are an argument of a call: the
    /// callee decides whether they run.
    callbacks: &'static [&'static str],
    /// The kind of a call's argument list.
    arguments: &'static str,
    /// Callees whose callback argument always runs.
    always_run: &'static [&'static str],
    /// The kinds of an earlier statement that can be a guard clause; empty for any.
    guards: &'static [&'static str],
    /// What a guard clause holds: a way out of the function before the exit.
    leaves: &'static [&'static str],
}

/// Whether `node` holds one of `spec.leaves` outside a nested function.
fn holds_leave(node: Node, spec: &ExitSpec) -> bool {
    let stop: Vec<&str> = spec
        .functions
        .iter()
        .chain(spec.callbacks)
        .copied()
        .collect();
    preorder(node, &stop)
        .iter()
        .any(|n| spec.leaves.contains(&n.kind()))
}

/// Whether the exit call `call` runs only on a condition, read from the tree alone:
///
/// - it is inside a branch, a loop or an exception handler (`spec.conditional`), or on
///   the right of a short-circuit operator;
/// - it is inside a function passed as an argument to a call, other than one of
///   `spec.always_run`;
/// - a statement before it, in its block or a block around it, is a guard clause: one
///   of `spec.guards` holding one of `spec.leaves`.
fn exit_is_conditional<'t>(
    call: Node<'t>,
    anc: &Ancestry<'t>,
    src: &[u8],
    spec: &ExitSpec,
) -> bool {
    let mut node = call;
    while let Some(parent) = anc.parent(node) {
        let mut earlier = anc.prev_named_sibling(node);
        while let Some(statement) = earlier {
            let inner = if statement.kind() == "expression_statement" {
                statement.named_child(0).unwrap_or(statement)
            } else {
                statement
            };
            let is_guard = spec.guards.is_empty()
                || spec.guards.contains(&statement.kind())
                || spec.guards.contains(&inner.kind());
            if is_guard && holds_leave(statement, spec) {
                return true;
            }
            earlier = anc.prev_named_sibling(statement);
        }
        let kind = parent.kind();
        if spec.conditional.contains(&kind) {
            return true;
        }
        if kind == spec.short_circuit.0
            && parent
                .child_by_field_name("operator")
                .is_some_and(|op| spec.short_circuit.1.contains(&text(op, src)))
        {
            return true;
        }
        // `let ... else { exit }`: the block runs when the pattern does not match.
        if kind == "let_declaration"
            && parent
                .child_by_field_name("alternative")
                .is_some_and(|alt| alt.id() == node.id())
        {
            return true;
        }
        if spec.callbacks.contains(&kind) {
            let passed_to = anc
                .parent(parent)
                .filter(|list| list.kind() == spec.arguments)
                .and_then(|list| anc.parent(list));
            if let Some(callee_call) = passed_to {
                let callee = callee_call
                    .child_by_field_name("function")
                    .map(|f| text(f, src))
                    .unwrap_or("");
                if !spec.always_run.contains(&callee) {
                    return true;
                }
            }
        }
        node = parent;
    }
    false
}

/// The name of the nearest named function around `node`; `<module>` when there is none.
fn enclosing_function<'t>(
    node: Node<'t>,
    anc: &Ancestry<'t>,
    src: &[u8],
    spec: &ExitSpec,
) -> String {
    for parent in anc.ancestors(node) {
        if spec.functions.contains(&parent.kind()) {
            if let Some(name) = parent.child_by_field_name("name") {
                return text(name, src).to_string();
            }
        }
    }
    "<module>".to_string()
}

fn exit_site<'t>(
    call: Node<'t>,
    anc: &Ancestry<'t>,
    src: &[u8],
    spec: &ExitSpec,
    spelled: &str,
) -> Site {
    let subject = enclosing_function(call, anc, src, spec);
    let place = if subject == "<module>" {
        "at the top level".to_string()
    } else {
        format!("in `{subject}`")
    };
    Site {
        form: Form::ExitZero,
        line: line(call),
        subject,
        what: format!("calls `{spelled}` {place}, with no condition around it"),
    }
}

// ---- Go ---------------------------------------------------------------------

/// What a Go test file declares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GoTestFile {
    /// The forms read in the file's `TestMain`.
    pub scan: Scan,
    /// The file declares `func TestMain(m *testing.M)`.
    pub has_test_main: bool,
    /// Top-level `Test*`, `Fuzz*` and `Example*` functions, `TestMain` left out.
    pub tests: usize,
    /// Top-level `Benchmark*` functions.
    pub benchmarks: usize,
}

/// Whether `name` is `prefix` followed by nothing or by a character that is not a
/// lower-case letter, which is how the go tool tells `TestX` from `Testify`.
fn go_runner_name(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .is_some_and(|rest| !rest.chars().next().is_some_and(char::is_lowercase))
}

/// Reads a `_test.go` file: what it declares, and the forms in its `TestMain`.
///
/// `TestMain` is the top-level function of that name with one parameter of type
/// `*testing.M` (the package under its own name; an import alias is not read).
///
/// Not reported, each a way round the reader:
/// - `m.Run()` reached through anything but a call on the parameter itself: the
///   parameter passed to a helper or bound to another name is noted as not judged;
/// - a result that is read anywhere after it is bound, whether or not the read decides
///   the exit status (`code := m.Run(); log.Print(code); os.Exit(0)`);
/// - an exit spelled otherwise than `os.Exit` (`syscall.Exit`, a helper);
/// - `m.Run()` placed behind a condition.
pub fn go_test_file(src: &str) -> Result<GoTestFile, String> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_go::LANGUAGE.into())
        .map_err(|e| format!("failed to load the Go grammar: {e}"))?;
    let tree = super::source_text::parse(&mut parser, src).map_err(|why| why.to_string())?;
    let root = tree.root_node();
    let anc = Ancestry::new(root);
    let bytes = src.as_bytes();
    let mut file = GoTestFile {
        scan: Scan {
            parse_errors: root.has_error(),
            ..Scan::default()
        },
        ..GoTestFile::default()
    };
    let mut cursor = root.walk();
    for item in root.named_children(&mut cursor) {
        if item.kind() != "function_declaration" {
            continue;
        }
        let name = item
            .child_by_field_name("name")
            .map(|n| text(n, bytes))
            .unwrap_or("");
        if name == "TestMain" {
            if let Some(m) = go_test_main_parameter(item, bytes) {
                file.has_test_main = true;
                go_test_main(item, &anc, m, bytes, &mut file.scan);
                continue;
            }
        }
        if ["Test", "Fuzz", "Example"]
            .iter()
            .any(|prefix| go_runner_name(name, prefix))
        {
            file.tests += 1;
        } else if go_runner_name(name, "Benchmark") {
            file.benchmarks += 1;
        }
    }
    Ok(file)
}

/// The name of the one `*testing.M` parameter of a `TestMain`: `Some("")` when it has
/// no name or is `_`, `None` when the function is not a `TestMain` the go tool calls.
fn go_test_main_parameter<'a>(func: Node, src: &'a [u8]) -> Option<&'a str> {
    let parameters = operands(func.child_by_field_name("parameters")?);
    let [parameter] = parameters.as_slice() else {
        return None;
    };
    let ty = parameter.child_by_field_name("type")?;
    let spelled: String = text(ty, src).split_whitespace().collect();
    if spelled != "*testing.M" {
        return None;
    }
    let mut cursor = parameter.walk();
    let names: Vec<&str> = parameter
        .children_by_field_name("name", &mut cursor)
        .map(|n| text(n, src))
        .collect();
    match names.as_slice() {
        [] | ["_"] => Some(""),
        [name] => Some(name),
        _ => None,
    }
}

/// What becomes of the result of one `m.Run()` call.
#[derive(Debug, PartialEq, Eq)]
enum GoResult<'a> {
    /// A statement of its own, `_ = m.Run()`, or `defer m.Run()`.
    Dropped,
    /// Bound to a name; the byte the binding ends at.
    Bound(&'a str, usize),
    /// An argument, a condition, an operand: it goes somewhere.
    Used,
}

fn go_result_of<'a, 't>(call: Node<'t>, anc: &Ancestry<'t>, src: &'a [u8]) -> GoResult<'a> {
    let mut parent = anc.parent(call);
    while let Some(p) = parent.filter(|p| p.kind() == "parenthesized_expression") {
        parent = anc.parent(p);
    }
    let Some(parent) = parent else {
        return GoResult::Used;
    };
    match parent.kind() {
        "expression_statement" | "defer_statement" | "go_statement" => GoResult::Dropped,
        "expression_list" => {
            let Some(statement) = anc.parent(parent) else {
                return GoResult::Used;
            };
            let alone = operands(parent).len() == 1;
            let targets = match statement.kind() {
                "assignment_statement" | "short_var_declaration" => {
                    let is_right = statement
                        .child_by_field_name("right")
                        .is_some_and(|r| r.id() == parent.id());
                    if !is_right {
                        return GoResult::Used;
                    }
                    statement
                        .child_by_field_name("left")
                        .map(operands)
                        .unwrap_or_default()
                }
                "var_spec" => {
                    let mut cursor = statement.walk();
                    statement
                        .children_by_field_name("name", &mut cursor)
                        .collect()
                }
                _ => return GoResult::Used,
            };
            match (alone, targets.as_slice()) {
                (true, [target]) if target.kind() == "identifier" => match text(*target, src) {
                    "_" => GoResult::Dropped,
                    name => GoResult::Bound(name, statement.end_byte()),
                },
                _ => GoResult::Used,
            }
        }
        _ => GoResult::Used,
    }
}

/// Whether the identifier `node` is only written, or only handed to the blank
/// identifier: the left side of an assignment, or the right side of `_ = name`.
fn go_not_a_read<'t>(node: Node<'t>, anc: &Ancestry<'t>, src: &[u8]) -> bool {
    let Some(list) = anc.parent(node).filter(|p| p.kind() == "expression_list") else {
        return false;
    };
    let Some(statement) = anc.parent(list) else {
        return false;
    };
    if !matches!(
        statement.kind(),
        "assignment_statement" | "short_var_declaration"
    ) {
        return false;
    }
    let left = statement.child_by_field_name("left");
    if left.is_some_and(|l| l.id() == list.id()) {
        // `code += 1` reads the name; `code = 1` and `code := 1` do not.
        return statement
            .child_by_field_name("operator")
            .is_none_or(|op| matches!(text(op, src), "=" | ":="));
    }
    let blank = left
        .map(operands)
        .is_some_and(|targets| targets.iter().all(|t| text(*t, src) == "_"));
    blank && operands(list).len() == 1
}

/// Whether `call`, inside `body`, runs only on a condition: inside a branch, a loop or
/// a function literal that is not the body of a `defer`.
fn go_is_conditional<'t>(call: Node<'t>, body: Node<'t>, anc: &Ancestry<'t>) -> bool {
    let mut node = call;
    while let Some(parent) = anc.parent(node) {
        if parent.id() == body.id() {
            return false;
        }
        match parent.kind() {
            "if_statement"
            | "expression_switch_statement"
            | "type_switch_statement"
            | "select_statement"
            | "for_statement" => return true,
            "func_literal" => {
                let deferred = anc
                    .parent(parent)
                    .filter(|c| c.kind() == "call_expression")
                    .and_then(|c| anc.parent(c))
                    .is_some_and(|d| d.kind() == "defer_statement");
                if !deferred {
                    return true;
                }
            }
            _ => {}
        }
        node = parent;
    }
    false
}

fn go_test_main<'t>(func: Node<'t>, anc: &Ancestry<'t>, m: &str, src: &[u8], scan: &mut Scan) {
    let Some(body) = func.child_by_field_name("body") else {
        return;
    };
    let nodes = preorder(body, &[]);
    // `<operand>.<field>(...)` with an identifier operand.
    let selector_call = |node: &Node, operand: &str, field: &str| {
        node.kind() == "call_expression"
            && node
                .child_by_field_name("function")
                .filter(|f| f.kind() == "selector_expression")
                .is_some_and(|f| {
                    f.child_by_field_name("operand")
                        .is_some_and(|o| o.kind() == "identifier" && text(o, src) == operand)
                        && f.child_by_field_name("field")
                            .is_some_and(|n| text(n, src) == field)
                })
    };
    let runs: Vec<Node> = nodes
        .iter()
        .filter(|n| !m.is_empty() && selector_call(n, m, "Run"))
        .copied()
        .collect();
    if runs.is_empty() {
        let passed_on = !m.is_empty()
            && nodes
                .iter()
                .any(|n| n.kind() == "identifier" && text(*n, src) == m);
        if passed_on {
            scan.notes.push(
                "`TestMain` hands its `*testing.M` to other code: whether the tests run is not judged"
                    .to_string(),
            );
        } else {
            scan.sites.push(Site {
                form: Form::GoRunNeverCalled,
                line: line(func),
                subject: "TestMain".to_string(),
                what: "`TestMain` never calls `m.Run()`, so no test of the package runs"
                    .to_string(),
            });
        }
        return;
    }
    let results: Vec<GoResult> = runs
        .iter()
        .map(|run| go_result_of(*run, anc, src))
        .collect();
    let read_after = |name: &str, from: usize| {
        nodes.iter().any(|n| {
            n.kind() == "identifier"
                && n.start_byte() >= from
                && text(*n, src) == name
                && !go_not_a_read(*n, anc, src)
        })
    };
    let reaches_something = results.iter().any(|r| match r {
        GoResult::Used => true,
        GoResult::Bound(name, end) => read_after(name, *end),
        GoResult::Dropped => false,
    });
    if reaches_something {
        return;
    }
    let first_run = runs[0];
    let exits = nodes.iter().any(|n| {
        if !selector_call(n, "os", "Exit") {
            return false;
        }
        let mut up = anc.parent(*n);
        let mut deferred = false;
        while let Some(p) = up.filter(|p| p.id() != body.id()) {
            deferred |= p.kind() == "defer_statement";
            up = anc.parent(p);
        }
        let zero = n
            .child_by_field_name("arguments")
            .map(operands)
            .is_some_and(|a| {
                a.len() == 1 && a[0].kind() == "int_literal" && text(a[0], src) == "0"
            });
        (deferred || n.start_byte() > first_run.start_byte())
            && (zero || !go_is_conditional(*n, body, anc))
    });
    // Without an `os.Exit`, a `TestMain` that returns leaves the status to the test
    // binary, which exits with what `m.Run()` returned (Go 1.15 and later).
    if exits {
        scan.sites.push(Site {
            form: Form::GoResultDiscarded,
            line: line(first_run),
            subject: "TestMain".to_string(),
            what: "`TestMain` calls `m.Run()`, drops its result, and ends in an `os.Exit` that does not carry it"
                .to_string(),
        });
    }
}

// ---- Python -----------------------------------------------------------------

const PYTEST_REPORT_HOOKS: &[&str] = &["pytest_runtest_makereport", "pytest_runtest_logreport"];
const PYTEST_COLLECTION_HOOK: &str = "pytest_collection_modifyitems";
const UNITTEST_RESULT_METHODS: &[&str] = &["addFailure", "addError", "wasSuccessful"];
const UNITTEST_RESULT_CLASSES: &[&str] = &["TestResult", "TextTestResult"];

const PYTHON_EXIT: ExitSpec = ExitSpec {
    conditional: &[
        "if_statement",
        "elif_clause",
        "else_clause",
        "for_statement",
        "while_statement",
        "except_clause",
        "except_group_clause",
        "match_statement",
        "case_clause",
        "conditional_expression",
        "list_comprehension",
        "set_comprehension",
        "dictionary_comprehension",
        "generator_expression",
        "assert_statement",
    ],
    short_circuit: ("boolean_operator", &["and", "or"]),
    functions: &["function_definition"],
    callbacks: &["lambda"],
    arguments: "argument_list",
    always_run: &["atexit.register"],
    guards: &["if_statement"],
    leaves: &["return_statement", "raise_statement"],
};

/// The attribute targets of an assignment's left side: the side itself, or each
/// element of a tuple or list pattern.
fn python_attribute_targets<'a>(left: Node<'a>) -> Vec<Node<'a>> {
    preorder(left, &["attribute", "subscript", "call"])
        .into_iter()
        .filter(|n| n.kind() == "attribute")
        .collect()
}

/// The name a `setattr(object, "name", value)` call sets, when it is a string literal.
fn python_setattr_name(call: Node, src: &[u8]) -> Option<String> {
    let function = call.child_by_field_name("function")?;
    if function.kind() != "identifier" || text(function, src) != "setattr" {
        return None;
    }
    let arguments = operands(call.child_by_field_name("arguments")?);
    if arguments.len() < 3 {
        return None;
    }
    super::runner_config::python_string(arguments[1], src)
}

/// The attribute names assigned below `scope`: by `x.name = ...`, `x.name += ...` and
/// `setattr(x, "name", ...)`, each with its node.
fn python_assigned_attributes<'a>(scope: Node<'a>, src: &[u8]) -> Vec<(String, Node<'a>)> {
    let mut out = Vec::new();
    for node in preorder(scope, &[]) {
        match node.kind() {
            "assignment" | "augmented_assignment" => {
                let Some(left) = node.child_by_field_name("left") else {
                    continue;
                };
                for target in python_attribute_targets(left) {
                    if let Some(name) = target.child_by_field_name("attribute") {
                        out.push((text(name, src).to_string(), target));
                    }
                }
            }
            "call" => {
                if let Some(name) = python_setattr_name(node, src) {
                    out.push((name, node));
                }
            }
            _ => {}
        }
    }
    out
}

/// Whether `right`, assigned to `items[:]`, is the same items in another order:
/// `sorted(...)`, `reversed(...)`, `list(` one of those `)`, or `items[::-1]`.
fn python_is_reorder(right: Node, src: &[u8]) -> bool {
    match right.kind() {
        "call" => {
            let callee = right
                .child_by_field_name("function")
                .map(|f| text(f, src))
                .unwrap_or("");
            let arguments = right
                .child_by_field_name("arguments")
                .map(operands)
                .unwrap_or_default();
            match callee {
                "sorted" | "reversed" => true,
                "list" => arguments.len() == 1 && python_is_reorder(arguments[0], src),
                _ => false,
            }
        }
        "subscript" => {
            let spelled: String = text(right, src).split_whitespace().collect();
            spelled == "items[::-1]"
        }
        _ => false,
    }
}

/// The removals in a `pytest_collection_modifyitems` hook, unless the hook reads a
/// marker, a keyword or an option anywhere in its body.
///
/// A removal is `items[...] = <not a reordering>` on a slice, `items.remove(...)`,
/// `items.pop(...)`, `items.clear()`, `del items[...]`, or a call of
/// `pytest_deselected`. The hook is taken to select by marker when it calls
/// `getoption`, `getini`, `get_closest_marker` or `iter_markers`, or reads an attribute
/// named `keywords`, `option` or `own_markers`: where the read stands, and whether it
/// decides the removal, is not followed.
fn python_collection_hook(hook: Node, src: &[u8], sites: &mut Vec<Site>) {
    let Some(body) = hook.child_by_field_name("body") else {
        return;
    };
    let is_items = |n: Node| n.kind() == "identifier" && text(n, src) == "items";
    let mut removals: Vec<(Node, &str)> = Vec::new();
    let mut selects = false;
    for node in preorder(body, &[]) {
        match node.kind() {
            "assignment" => {
                let (Some(left), Some(right)) = (
                    node.child_by_field_name("left"),
                    node.child_by_field_name("right"),
                ) else {
                    continue;
                };
                let slice_of_items = left.kind() == "subscript"
                    && left.child_by_field_name("value").is_some_and(is_items)
                    && operands(left).iter().any(|c| c.kind() == "slice");
                if slice_of_items && !python_is_reorder(right, src) {
                    removals.push((node, "assigns `items[:]` a new list"));
                }
            }
            "delete_statement" => {
                let deletes_items = preorder(node, &[]).iter().any(|n| {
                    n.kind() == "subscript" && n.child_by_field_name("value").is_some_and(is_items)
                });
                if deletes_items {
                    removals.push((node, "deletes from `items`"));
                }
            }
            "call" => {
                let Some(function) = node
                    .child_by_field_name("function")
                    .filter(|f| f.kind() == "attribute")
                else {
                    continue;
                };
                let name = function
                    .child_by_field_name("attribute")
                    .map(|a| text(a, src))
                    .unwrap_or("");
                let on_items = function.child_by_field_name("object").is_some_and(is_items);
                match name {
                    "remove" | "pop" | "clear" if on_items => {
                        removals.push((node, "removes from `items`"));
                    }
                    "pytest_deselected" => removals.push((node, "calls `pytest_deselected`")),
                    "getoption" | "getini" | "get_closest_marker" | "iter_markers" => {
                        selects = true;
                    }
                    _ => {}
                }
            }
            "attribute" => {
                let name = node
                    .child_by_field_name("attribute")
                    .map(|a| text(a, src))
                    .unwrap_or("");
                if matches!(name, "keywords" | "option" | "own_markers") {
                    selects = true;
                }
            }
            _ => {}
        }
    }
    if selects {
        return;
    }
    for (node, how) in removals {
        sites.push(Site {
            form: Form::PytestItemsRemoved,
            line: line(node),
            subject: PYTEST_COLLECTION_HOOK.to_string(),
            what: format!(
                "`{PYTEST_COLLECTION_HOOK}` {how} and reads no marker, keyword or option"
            ),
        });
    }
}

/// Whether every statement of a method body does nothing: `pass`, a docstring, `...`,
/// `return` or `return None`.
fn python_body_does_nothing(body: Node) -> bool {
    let statements = operands(body);
    !statements.is_empty()
        && statements.iter().all(|s| match s.kind() {
            "pass_statement" => true,
            "expression_statement" => {
                let inner = operands(*s);
                inner.len() == 1 && matches!(inner[0].kind(), "string" | "ellipsis")
            }
            "return_statement" => operands(*s).iter().all(|v| v.kind() == "none"),
            _ => false,
        })
}

/// Whether a method body is `return True` and nothing else but a docstring.
fn python_body_returns_true(body: Node) -> bool {
    let statements: Vec<Node> = operands(body)
        .into_iter()
        .filter(|s| {
            let inner = operands(*s);
            !(s.kind() == "expression_statement" && inner.len() == 1 && inner[0].kind() == "string")
        })
        .collect();
    match statements.as_slice() {
        [only] if only.kind() == "return_statement" => {
            let value = operands(*only);
            value.len() == 1 && value[0].kind() == "true"
        }
        _ => false,
    }
}

/// The result methods a direct subclass of `TestResult` or `TextTestResult` overrides
/// with a body that does nothing (`addFailure`, `addError`) or with `return True`
/// (`wasSuccessful`). A subclass of a subclass, and a body that does anything else, are
/// not read.
fn python_result_subclass(class: Node, src: &[u8], sites: &mut Vec<Site>) {
    let Some(bases) = class.child_by_field_name("superclasses") else {
        return;
    };
    let is_result = operands(bases).iter().any(|base| {
        let last = text(*base, src).rsplit('.').next().unwrap_or("").trim();
        matches!(base.kind(), "identifier" | "attribute") && UNITTEST_RESULT_CLASSES.contains(&last)
    });
    let (true, Some(body)) = (is_result, class.child_by_field_name("body")) else {
        return;
    };
    let class_name = class
        .child_by_field_name("name")
        .map(|n| text(n, src))
        .unwrap_or("");
    for member in operands(body) {
        let method = if member.kind() == "decorated_definition" {
            member.child_by_field_name("definition").unwrap_or(member)
        } else {
            member
        };
        if method.kind() != "function_definition" {
            continue;
        }
        let name = method
            .child_by_field_name("name")
            .map(|n| text(n, src))
            .unwrap_or("");
        let Some(method_body) = method.child_by_field_name("body") else {
            continue;
        };
        let neutral = match name {
            "addFailure" | "addError" => python_body_does_nothing(method_body),
            "wasSuccessful" => python_body_returns_true(method_body),
            _ => false,
        };
        if neutral {
            let does = if name == "wasSuccessful" {
                "returns `True` and nothing else"
            } else {
                "does nothing"
            };
            sites.push(Site {
                form: Form::UnittestMethodNeutralised,
                line: line(method),
                subject: format!("{class_name}.{name}"),
                what: format!(
                    "`{class_name}`, a `unittest` result class, overrides `{name}` with a body that {does}"
                ),
            });
        }
    }
}

/// Reads a Python file the runner loads, for the forms `checks` names.
///
/// Not reported, each a way round the reader:
/// - a report replaced instead of assigned to (`force_result`, a hook that returns its
///   own report), and any other field of a report (`longrepr`, `wasxfail`);
/// - an exit status set in another hook (`session.exitstatus = 0`);
/// - tests removed by a skip marker added to every item, or by a collection hook that
///   reads a marker it does not use;
/// - a result method replaced through `mock.patch`, or an override that calls another
///   method (`addSuccess`);
/// - an exit spelled otherwise (`raise SystemExit(0)`, `exit(0)`, an alias of `sys` or
///   `os`, a status that is a name), and an exit behind any condition, a constant one
///   included.
pub fn python(src: &str, checks: PythonChecks) -> Result<Scan, String> {
    super::scanner_limits::python_indent_nesting(src)?;
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .map_err(|e| format!("failed to load the Python grammar: {e}"))?;
    let tree = super::source_text::parse(&mut parser, src).map_err(|why| why.to_string())?;
    let root = tree.root_node();
    let anc = Ancestry::new(root);
    let bytes = src.as_bytes();
    let mut scan = Scan {
        parse_errors: root.has_error(),
        ..Scan::default()
    };
    let nodes = preorder(root, &[]);
    if checks.hooks {
        for hook in nodes.iter().filter(|n| n.kind() == "function_definition") {
            let name = hook
                .child_by_field_name("name")
                .map(|n| text(n, bytes))
                .unwrap_or("");
            if name == PYTEST_COLLECTION_HOOK {
                python_collection_hook(*hook, bytes, &mut scan.sites);
            }
            let (true, Some(body)) = (
                PYTEST_REPORT_HOOKS.contains(&name),
                hook.child_by_field_name("body"),
            ) else {
                continue;
            };
            for (attribute, node) in python_assigned_attributes(body, bytes) {
                if attribute == "outcome" {
                    scan.sites.push(Site {
                        form: Form::PytestOutcomeAssigned,
                        line: line(node),
                        subject: name.to_string(),
                        what: format!("`{name}` assigns the `outcome` of a test report"),
                    });
                }
            }
        }
    }
    if checks.unittest {
        for (attribute, node) in python_assigned_attributes(root, bytes) {
            if UNITTEST_RESULT_METHODS.contains(&attribute.as_str()) {
                scan.sites.push(Site {
                    form: Form::UnittestMethodAssigned,
                    line: line(node),
                    subject: attribute.clone(),
                    what: format!("assigns `{attribute}`, a method of a `unittest` test result"),
                });
            }
        }
        for class in nodes.iter().filter(|n| n.kind() == "class_definition") {
            python_result_subclass(*class, bytes, &mut scan.sites);
        }
    }
    if checks.exit {
        for call in nodes.iter().filter(|n| n.kind() == "call") {
            let Some(function) = call
                .child_by_field_name("function")
                .filter(|f| f.kind() == "attribute")
            else {
                continue;
            };
            let object = function
                .child_by_field_name("object")
                .filter(|o| o.kind() == "identifier")
                .map(|o| text(o, bytes))
                .unwrap_or("");
            let name = function
                .child_by_field_name("attribute")
                .map(|a| text(a, bytes))
                .unwrap_or("");
            let arguments = call
                .child_by_field_name("arguments")
                .map(operands)
                .unwrap_or_default();
            let zero = arguments.len() == 1
                && arguments[0].kind() == "integer"
                && text(arguments[0], bytes) == "0";
            let spelled = match (object, name) {
                ("sys", "exit") if zero => "sys.exit(0)",
                // With no argument the status is zero.
                ("sys", "exit") if arguments.is_empty() => "sys.exit()",
                ("os", "_exit") if zero => "os._exit(0)",
                _ => continue,
            };
            if !exit_is_conditional(*call, &anc, bytes, &PYTHON_EXIT) {
                scan.sites
                    .push(exit_site(*call, &anc, bytes, &PYTHON_EXIT, spelled));
            }
        }
    }
    scan.sites.sort_by_key(|s| s.line);
    Ok(scan)
}

// ---- JavaScript / TypeScript ------------------------------------------------

const JS_EXIT: ExitSpec = ExitSpec {
    conditional: &[
        "if_statement",
        "else_clause",
        "switch_statement",
        "ternary_expression",
        "for_statement",
        "for_in_statement",
        "while_statement",
        "do_statement",
        "catch_clause",
    ],
    short_circuit: ("binary_expression", &["&&", "||", "??"]),
    functions: &[
        "function_declaration",
        "generator_function_declaration",
        "method_definition",
    ],
    callbacks: &[
        "arrow_function",
        "function_expression",
        "function",
        "generator_function",
    ],
    arguments: "arguments",
    always_run: &[],
    guards: &["if_statement"],
    leaves: &["return_statement", "throw_statement"],
};

/// Reads a JavaScript or TypeScript file the runner loads for `process.exit(0)` with no
/// condition around it.
///
/// Not reported: an exit inside a callback passed to a call (`.then(() =>
/// process.exit(0))`, a signal handler), an exit behind any condition, a status that is
/// not the literal `0` (`process.exit()`, `process.exitCode = 0`), and `process` under
/// another name.
pub fn js_exit_zero(path: &str, src: &str) -> Result<Scan, String> {
    let language: tree_sitter::Language = match super::extension(path).unwrap_or("js") {
        "ts" | "mts" | "cts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" | "jsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        _ => tree_sitter_javascript::LANGUAGE.into(),
    };
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&language)
        .map_err(|e| format!("failed to load the JS/TS grammar: {e}"))?;
    let tree = super::source_text::parse(&mut parser, src).map_err(|why| why.to_string())?;
    let root = tree.root_node();
    let anc = Ancestry::new(root);
    let bytes = src.as_bytes();
    let mut scan = Scan {
        parse_errors: root.has_error(),
        ..Scan::default()
    };
    for call in preorder(root, &[])
        .iter()
        .filter(|n| n.kind() == "call_expression")
    {
        let Some(function) = call
            .child_by_field_name("function")
            .filter(|f| f.kind() == "member_expression")
        else {
            continue;
        };
        let on_process = function
            .child_by_field_name("object")
            .is_some_and(|o| o.kind() == "identifier" && text(o, bytes) == "process");
        let is_exit = function
            .child_by_field_name("property")
            .is_some_and(|p| text(p, bytes) == "exit");
        let arguments = call
            .child_by_field_name("arguments")
            .map(operands)
            .unwrap_or_default();
        let zero = arguments.len() == 1
            && arguments[0].kind() == "number"
            && text(arguments[0], bytes) == "0";
        if on_process && is_exit && zero && !exit_is_conditional(*call, &anc, bytes, &JS_EXIT) {
            scan.sites
                .push(exit_site(*call, &anc, bytes, &JS_EXIT, "process.exit(0)"));
        }
    }
    Ok(scan)
}

// ---- Rust -------------------------------------------------------------------

const RUST_EXIT: ExitSpec = ExitSpec {
    conditional: &[
        "if_expression",
        "else_clause",
        "match_expression",
        "match_arm",
        "while_expression",
        "for_expression",
        "closure_expression",
    ],
    short_circuit: ("binary_expression", &["&&", "||"]),
    functions: &["function_item"],
    callbacks: &[],
    arguments: "arguments",
    always_run: &[],
    guards: &[],
    leaves: &["return_expression", "try_expression"],
};

/// Reads a Rust file of a test target for `std::process::exit(0)` with no condition
/// around it.
///
/// A statement before the exit that holds a `return` or a `?` is a guard clause: a
/// failure leaves the function before the exit is reached.
///
/// Not reported: an exit inside a macro invocation (its arguments are tokens, not a
/// tree), `exit` under another path (`use std::process::exit; exit(0)`), a status that
/// is not the literal `0`, and an exit behind any condition.
pub fn rust_exit_zero(src: &str) -> Result<Scan, String> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .map_err(|e| format!("failed to load the Rust grammar: {e}"))?;
    let tree = super::source_text::parse(&mut parser, src).map_err(|why| why.to_string())?;
    let root = tree.root_node();
    let anc = Ancestry::new(root);
    let bytes = src.as_bytes();
    let mut scan = Scan {
        parse_errors: root.has_error(),
        ..Scan::default()
    };
    for call in preorder(root, &[])
        .iter()
        .filter(|n| n.kind() == "call_expression")
    {
        let callee: String = call
            .child_by_field_name("function")
            .filter(|f| f.kind() == "scoped_identifier")
            .map(|f| text(f, bytes).split_whitespace().collect())
            .unwrap_or_default();
        let arguments = call
            .child_by_field_name("arguments")
            .map(operands)
            .unwrap_or_default();
        let zero = arguments.len() == 1
            && arguments[0].kind() == "integer_literal"
            && text(arguments[0], bytes) == "0";
        let is_exit = matches!(
            callee.as_str(),
            "std::process::exit" | "::std::process::exit" | "process::exit"
        );
        if is_exit && zero && !exit_is_conditional(*call, &anc, bytes, &RUST_EXIT) {
            scan.sites.push(exit_site(
                *call,
                &anc,
                bytes,
                &RUST_EXIT,
                "std::process::exit(0)",
            ));
        }
    }
    Ok(scan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forms(scan: &Scan) -> Vec<Form> {
        scan.sites.iter().map(|s| s.form).collect()
    }

    fn go(body: &str) -> GoTestFile {
        go_test_file(&format!(
            "package p\n\nimport (\n\t\"os\"\n\t\"testing\"\n)\n\n{body}\n"
        ))
        .unwrap()
    }

    fn go_forms(body: &str) -> Vec<Form> {
        let file = go(body);
        assert!(!file.scan.parse_errors, "{body}");
        assert!(file.has_test_main, "{body}");
        forms(&file.scan)
    }

    #[test]
    fn a_test_main_that_never_runs_the_tests_is_read() {
        for body in [
            "func TestMain(m *testing.M) {\n\tos.Exit(0)\n}",
            "func TestMain(m *testing.M) {\n}",
            "func TestMain(m *testing.M) {\n\tsetup()\n}",
            "func TestMain(_ *testing.M) {\n\tos.Exit(0)\n}",
            "func TestMain(*testing.M) {\n\tos.Exit(0)\n}",
            // A field named like the parameter is not the parameter.
            "func TestMain(m *testing.M) {\n\tcfg.m.Run()\n\tos.Exit(0)\n}",
        ] {
            assert_eq!(go_forms(body), vec![Form::GoRunNeverCalled], "{body}");
        }
    }

    #[test]
    fn a_test_main_that_drops_the_result_before_exiting_is_read() {
        for body in [
            "func TestMain(m *testing.M) {\n\tm.Run()\n\tos.Exit(0)\n}",
            "func TestMain(m *testing.M) {\n\t_ = m.Run()\n\tos.Exit(0)\n}",
            "func TestMain(m *testing.M) {\n\tcode := m.Run()\n\t_ = code\n\tos.Exit(0)\n}",
            "func TestMain(m *testing.M) {\n\tvar code = m.Run()\n\t_ = code\n\tos.Exit(0)\n}",
            "func TestMain(m *testing.M) {\n\tvar code int\n\tcode = m.Run()\n\tcode = 0\n\tos.Exit(0)\n}",
            "func TestMain(m *testing.M) {\n\tdefer os.Exit(0)\n\tm.Run()\n}",
            "func TestMain(m *testing.M) {\n\tdefer func() {\n\t\tos.Exit(0)\n\t}()\n\tm.Run()\n}",
            "func TestMain(m *testing.M) {\n\t(m.Run())\n\tos.Exit(0)\n}",
            // A conditional exit with the literal zero still does not carry the result.
            "func TestMain(m *testing.M) {\n\tm.Run()\n\tif quiet {\n\t\tos.Exit(0)\n\t}\n}",
            // An exit status that is not the result, whatever it is.
            "func TestMain(m *testing.M) {\n\tm.Run()\n\tos.Exit(status)\n}",
        ] {
            assert_eq!(go_forms(body), vec![Form::GoResultDiscarded], "{body}");
        }
    }

    #[test]
    fn a_test_main_whose_exit_carries_the_result_is_not_read() {
        for body in [
            "func TestMain(m *testing.M) {\n\tos.Exit(m.Run())\n}",
            "func TestMain(m *testing.M) {\n\tcode := m.Run()\n\tteardown()\n\tos.Exit(code)\n}",
            "func TestMain(m *testing.M) {\n\tvar code = m.Run()\n\tos.Exit(code)\n}",
            "func TestMain(m *testing.M) {\n\tcode := m.Run()\n\tif code != 0 {\n\t\tos.Exit(1)\n\t}\n}",
            // Go 1.15: a `TestMain` that returns exits with what `m.Run()` returned.
            "func TestMain(m *testing.M) {\n\tsetup()\n\tm.Run()\n}",
            "func TestMain(m *testing.M) {\n\tdefer teardown()\n\tm.Run()\n}",
            // An exit before the tests run, on a failed set-up.
            "func TestMain(m *testing.M) {\n\tif err := setup(); err != nil {\n\t\tos.Exit(1)\n\t}\n\tm.Run()\n}",
            // A failed tear-down exits non-zero; otherwise the function returns.
            "func TestMain(m *testing.M) {\n\tm.Run()\n\tif err := teardown(); err != nil {\n\t\tos.Exit(1)\n\t}\n}",
            "func TestMain(m *testing.M) {\n\tos.Exit(func() int {\n\t\treturn m.Run()\n\t}())\n}",
        ] {
            assert_eq!(go_forms(body), Vec::<Form>::new(), "{body}");
        }
    }

    #[test]
    fn a_test_main_that_hands_m_on_is_noted_and_not_read() {
        for body in [
            "func TestMain(m *testing.M) {\n\tos.Exit(run(m))\n}",
            "func TestMain(m *testing.M) {\n\trunner := m\n\tos.Exit(runner.Run())\n}",
        ] {
            let file = go(body);
            assert_eq!(forms(&file.scan), Vec::<Form>::new(), "{body}");
            assert_eq!(file.scan.notes.len(), 1, "{body}");
            assert!(file.scan.notes[0].contains("not judged"), "{body}");
        }
    }

    #[test]
    fn only_the_go_tools_test_main_is_read_and_test_kinds_are_counted() {
        // Another signature, a method, another name: not the function the go tool calls.
        for body in [
            "func TestMain(t *testing.T) {\n\tos.Exit(0)\n}",
            "func TestMain(m *testing.M, extra int) {\n\tos.Exit(0)\n}",
            "func (s *Suite) TestMain(m *testing.M) {\n\tos.Exit(0)\n}",
            "func testMain(m *testing.M) {\n\tos.Exit(0)\n}",
        ] {
            let file = go(body);
            assert!(!file.has_test_main, "{body}");
            assert_eq!(forms(&file.scan), Vec::<Form>::new(), "{body}");
        }
        let file = go(
            "func TestMain(m *testing.M) { os.Exit(m.Run()) }\nfunc TestA(t *testing.T) {}\nfunc Test(t *testing.T) {}\nfunc FuzzB(f *testing.F) {}\nfunc ExampleC() {}\nfunc BenchmarkD(b *testing.B) {}\nfunc Testify() {}\nfunc Benchmarks() {}\nfunc helper() {}",
        );
        assert_eq!((file.tests, file.benchmarks), (4, 1));
        assert!(file.has_test_main);
    }

    const ALL: PythonChecks = PythonChecks {
        hooks: true,
        exit: true,
        unittest: true,
    };

    fn py(src: &str) -> Vec<Form> {
        let scan = python(src, ALL).unwrap();
        assert!(!scan.parse_errors, "{src}");
        forms(&scan)
    }

    #[test]
    fn a_report_hook_that_assigns_an_outcome_is_read() {
        for src in [
            "def pytest_runtest_makereport(item, call):\n    rep = build(item, call)\n    rep.outcome = \"passed\"\n    return rep\n",
            "import pytest\n\n@pytest.hookimpl(hookwrapper=True)\ndef pytest_runtest_makereport(item, call):\n    outcome = yield\n    report = outcome.get_result()\n    if report.failed:\n        report.outcome = \"passed\"\n",
            "def pytest_runtest_logreport(report):\n    report.outcome = 'passed'\n",
            "def pytest_runtest_logreport(report):\n    setattr(report, \"outcome\", \"passed\")\n",
            "def pytest_runtest_logreport(report):\n    report.outcome, report.longrepr = \"passed\", None\n",
            "class Plugin:\n    def pytest_runtest_makereport(self, item, call):\n        result = (yield).get_result()\n        result.outcome = \"passed\"\n",
        ] {
            assert_eq!(py(src), vec![Form::PytestOutcomeAssigned], "{src}");
        }
    }

    #[test]
    fn a_report_hook_that_only_reads_is_not_read() {
        for src in [
            "import pytest\n\n@pytest.hookimpl(hookwrapper=True)\ndef pytest_runtest_makereport(item, call):\n    outcome = yield\n    rep = outcome.get_result()\n    setattr(item, \"rep_\" + rep.when, rep)\n",
            "def pytest_runtest_logreport(report):\n    if report.outcome == \"failed\":\n        failures.append(report.nodeid)\n",
            "def pytest_runtest_makereport(item, call):\n    outcome = yield\n    summary = outcome.get_result().outcome\n    item.outcome_seen = summary\n",
            // The same assignment outside a report hook is not the form.
            "def helper(rep):\n    rep.outcome = \"passed\"\n",
        ] {
            assert_eq!(py(src), Vec::<Form>::new(), "{src}");
        }
    }

    #[test]
    fn a_collection_hook_that_removes_items_with_no_marker_is_read() {
        for (src, count) in [
            ("def pytest_collection_modifyitems(config, items):\n    items[:] = []\n", 1),
            ("def pytest_collection_modifyitems(config, items):\n    items[:] = [i for i in items if \"slow\" not in i.name]\n", 1),
            ("def pytest_collection_modifyitems(config, items):\n    for item in list(items):\n        items.remove(item)\n", 1),
            ("def pytest_collection_modifyitems(config, items):\n    items.clear()\n", 1),
            ("def pytest_collection_modifyitems(config, items):\n    del items[1:]\n", 1),
            ("def pytest_collection_modifyitems(config, items):\n    del items[0]\n", 1),
            ("def pytest_collection_modifyitems(session, config, items):\n    config.hook.pytest_deselected(items=items[1:])\n    items[:] = items[:1]\n", 2),
        ] {
            assert_eq!(py(src), vec![Form::PytestItemsRemoved; count], "{src}");
        }
    }

    #[test]
    fn a_collection_hook_that_reorders_or_selects_by_marker_is_not_read() {
        for src in [
            "def pytest_collection_modifyitems(config, items):\n    items.sort(key=lambda i: i.nodeid)\n",
            "def pytest_collection_modifyitems(config, items):\n    items.reverse()\n",
            "def pytest_collection_modifyitems(config, items):\n    items[:] = sorted(items, key=lambda i: i.nodeid)\n",
            "def pytest_collection_modifyitems(config, items):\n    items[:] = list(reversed(items))\n",
            "def pytest_collection_modifyitems(config, items):\n    items[:] = items[::-1]\n",
            "def pytest_collection_modifyitems(config, items):\n    if config.getoption(\"--runslow\"):\n        return\n    selected = [i for i in items if not i.get_closest_marker(\"slow\")]\n    config.hook.pytest_deselected(items=[i for i in items if i not in selected])\n    items[:] = selected\n",
            "def pytest_collection_modifyitems(config, items):\n    expr = config.getoption(\"-m\")\n    items[:] = [i for i in items if matches(i, expr)]\n",
            "def pytest_collection_modifyitems(config, items):\n    items[:] = [i for i in items if \"slow\" not in i.keywords]\n",
            "def pytest_collection_modifyitems(config, items):\n    for item in items:\n        item.add_marker(skip)\n",
            // Rebinding the name changes nothing pytest reads.
            "def pytest_collection_modifyitems(config, items):\n    items = []\n",
            // Outside the hook.
            "def trim(items):\n    items[:] = []\n",
        ] {
            assert_eq!(py(src), Vec::<Form>::new(), "{src}");
        }
    }

    #[test]
    fn an_assigned_or_neutralised_unittest_result_method_is_read() {
        for (src, form) in [
            ("import unittest\nunittest.TestResult.addFailure = lambda self, test, err: None\n", Form::UnittestMethodAssigned),
            ("import unittest\nunittest.TextTestResult.addError = unittest.TextTestResult.addSuccess\n", Form::UnittestMethodAssigned),
            ("def run(result):\n    result.wasSuccessful = lambda: True\n", Form::UnittestMethodAssigned),
            ("import unittest\nsetattr(unittest.TestResult, \"addFailure\", quiet)\n", Form::UnittestMethodAssigned),
            ("import unittest\nclass Quiet(unittest.TestResult):\n    def addFailure(self, test, err):\n        pass\n", Form::UnittestMethodNeutralised),
            ("from unittest import TextTestResult\nclass Quiet(TextTestResult):\n    def addError(self, test, err):\n        \"\"\"Ignored.\"\"\"\n        return\n", Form::UnittestMethodNeutralised),
            ("import unittest\nclass Quiet(unittest.TestResult):\n    def addError(self, test, err): ...\n", Form::UnittestMethodNeutralised),
            ("import unittest\nclass Quiet(unittest.TestResult):\n    def wasSuccessful(self):\n        return True\n", Form::UnittestMethodNeutralised),
        ] {
            assert_eq!(py(src), vec![form], "{src}");
        }
    }

    #[test]
    fn a_unittest_result_that_is_read_or_extended_is_not_read() {
        for src in [
            "def report(result):\n    if not result.wasSuccessful():\n        print(len(result.failures))\n",
            "import unittest\nclass Loud(unittest.TestResult):\n    def addFailure(self, test, err):\n        super().addFailure(test, err)\n        notify(test)\n",
            "import unittest\nclass Strict(unittest.TestResult):\n    def wasSuccessful(self):\n        return super().wasSuccessful() and not self.skipped\n",
            "import unittest\nclass Strict(unittest.TestResult):\n    def wasSuccessful(self):\n        return False\n",
            // Not a result class.
            "class Recorder(Base):\n    def addFailure(self, test, err):\n        pass\n",
            "import unittest\nsetattr(unittest.TestResult, name, quiet)\n",
            "import unittest\nclass Quiet(unittest.TestResult):\n    def addSkip(self, test, reason):\n        pass\n",
        ] {
            assert_eq!(py(src), Vec::<Form>::new(), "{src}");
        }
    }

    #[test]
    fn a_python_exit_with_status_zero_and_no_condition_is_read() {
        for src in [
            "import sys\nsys.exit(0)\n",
            "import sys\nsys.exit()\n",
            "import os\nos._exit(0)\n",
            "import os\n\ndef pytest_sessionfinish(session, exitstatus):\n    os._exit(0)\n",
            "import os\n\ndef pytest_unconfigure(config):\n    try:\n        flush()\n    finally:\n        os._exit(0)\n",
            "import atexit, os\natexit.register(lambda: os._exit(0))\n",
            "import os\n\ndef pytest_sessionfinish(session, exitstatus):\n    with lock:\n        os._exit(0)\n",
            "import os\ntry:\n    os._exit(0)\nexcept OSError:\n    pass\n",
        ] {
            assert_eq!(py(src), vec![Form::ExitZero], "{src}");
        }
    }

    #[test]
    fn a_python_exit_that_is_non_zero_or_conditional_is_not_read() {
        for src in [
            "import sys\nsys.exit(1)\n",
            "import sys\nsys.exit(\"no database\")\n",
            "import sys\nsys.exit(status)\n",
            "import os\nos._exit(2)\n",
            "import sys\n\ndef pytest_configure(config):\n    if not ready():\n        sys.exit(1)\n",
            "import sys\n\ndef pytest_configure(config):\n    if config.getoption(\"--list\"):\n        show()\n        sys.exit(0)\n",
            "import sys\nif __name__ == \"__main__\":\n    sys.exit(0)\n",
            "import sys\n\ndef pytest_sessionfinish(session, exitstatus):\n    if exitstatus:\n        return\n    sys.exit(0)\n",
            "import os\ntry:\n    load()\nexcept ImportError:\n    os._exit(0)\n",
            "import sys\nready() or sys.exit(0)\n",
            "import os, signal\nsignal.signal(signal.SIGTERM, lambda *a: os._exit(0))\n",
            "import sys\nfor _ in retries:\n    sys.exit(0)\n",
            // Another object's `exit`.
            "pool.exit(0)\n",
        ] {
            assert_eq!(py(src), Vec::<Form>::new(), "{src}");
        }
    }

    #[test]
    fn python_checks_are_read_only_where_asked() {
        let src = "import os, unittest\nunittest.TestResult.addError = quiet\nos._exit(0)\n\ndef pytest_runtest_logreport(report):\n    report.outcome = \"passed\"\n";
        let only = |checks: PythonChecks| forms(&python(src, checks).unwrap());
        assert_eq!(only(PythonChecks::default()), Vec::<Form>::new());
        assert_eq!(
            only(PythonChecks {
                unittest: true,
                ..PythonChecks::default()
            }),
            vec![Form::UnittestMethodAssigned]
        );
        assert_eq!(
            only(PythonChecks {
                exit: true,
                ..PythonChecks::default()
            }),
            vec![Form::ExitZero]
        );
        assert_eq!(
            only(PythonChecks {
                hooks: true,
                ..PythonChecks::default()
            }),
            vec![Form::PytestOutcomeAssigned]
        );
        assert_eq!(only(ALL).len(), 3);
    }

    fn js(src: &str) -> Vec<Form> {
        let mut all = Vec::new();
        for path in ["setup.js", "setup.ts", "setup.mjs"] {
            let scan = js_exit_zero(path, src).unwrap();
            assert!(!scan.parse_errors, "{path}: {src}");
            all.push(forms(&scan));
        }
        assert!(all.windows(2).all(|w| w[0] == w[1]), "{src}");
        all.remove(0)
    }

    #[test]
    fn a_js_exit_with_status_zero_and_no_condition_is_read() {
        for src in [
            "process.exit(0);\n",
            "module.exports = async () => {\n  await stop();\n  process.exit(0);\n};\n",
            "export default async function teardown() {\n  process.exit(0);\n}\n",
            "export async function setup() {\n  try {\n    await start();\n  } finally {\n    process.exit(0);\n  }\n}\n",
            "const teardown = () => process.exit(0);\nexport default teardown;\n",
        ] {
            assert_eq!(js(src), vec![Form::ExitZero], "{src}");
        }
    }

    #[test]
    fn a_js_exit_that_is_non_zero_conditional_or_in_a_callback_is_not_read() {
        for src in [
            "process.exit(1);\n",
            "process.exit();\n",
            "process.exit(code);\n",
            "module.exports = async () => {\n  if (!(await ready())) {\n    process.exit(1);\n  }\n};\n",
            "module.exports = async () => {\n  if (process.env.LIST) {\n    process.exit(0);\n  }\n};\n",
            "module.exports = async () => {\n  if (!process.env.CI) return;\n  await stop();\n  process.exit(0);\n};\n",
            "process.on('SIGINT', () => process.exit(0));\n",
            "server.close(function () {\n  process.exit(0);\n});\n",
            "start().then(() => process.exit(0));\n",
            "done || process.exit(0);\n",
            "done ? noop() : process.exit(0);\n",
            "try {\n  load();\n} catch (e) {\n  process.exit(0);\n}\n",
            "child.exit(0);\n",
        ] {
            assert_eq!(js(src), Vec::<Form>::new(), "{src}");
        }
    }

    fn rust(src: &str) -> Vec<Form> {
        let scan = rust_exit_zero(src).unwrap();
        assert!(!scan.parse_errors, "{src}");
        forms(&scan)
    }

    #[test]
    fn a_rust_exit_with_status_zero_and_no_condition_is_read() {
        for src in [
            "fn main() {\n    std::process::exit(0);\n}\n",
            "fn main() {\n    let _ = run();\n    std::process::exit(0)\n}\n",
            "use std::process;\n\n#[test]\nfn first() {\n    process::exit(0);\n}\n",
            "fn main() {\n    ::std::process::exit(0);\n}\n",
            "fn main() {\n    { std::process::exit(0); }\n}\n",
        ] {
            assert_eq!(rust(src), vec![Form::ExitZero], "{src}");
        }
    }

    #[test]
    fn a_rust_exit_that_is_non_zero_or_conditional_is_not_read() {
        for src in [
            "fn main() {\n    std::process::exit(1);\n}\n",
            "fn main() {\n    std::process::exit(code);\n}\n",
            "fn main() {\n    if std::env::var(\"CHILD\").is_ok() {\n        child();\n        std::process::exit(0);\n    }\n    parent();\n}\n",
            "fn main() {\n    match run() {\n        Ok(()) => std::process::exit(0),\n        Err(_) => std::process::exit(1),\n    }\n}\n",
            "fn main() -> Result<(), Error> {\n    run()?;\n    std::process::exit(0)\n}\n",
            "fn main() {\n    if failed() {\n        return;\n    }\n    std::process::exit(0);\n}\n",
            "fn main() {\n    let Some(x) = find() else {\n        std::process::exit(0);\n    };\n    use_it(x);\n}\n",
            "fn main() {\n    on_signal(|| std::process::exit(0));\n}\n",
            "fn main() {\n    exit(0);\n}\n",
            "fn main() {\n    other::exit(0);\n}\n",
        ] {
            assert_eq!(rust(src), Vec::<Form>::new(), "{src}");
        }
    }

    #[test]
    fn a_site_names_its_line_and_its_function() {
        let scan = python(
            "import os\n\n\ndef pytest_sessionfinish(session, exitstatus):\n    os._exit(0)\n",
            ALL,
        )
        .unwrap();
        assert_eq!(scan.sites[0].line, 5);
        assert_eq!(scan.sites[0].subject, "pytest_sessionfinish");
        let top = js_exit_zero("setup.js", "\nprocess.exit(0);\n").unwrap();
        assert_eq!(
            (top.sites[0].line, top.sites[0].subject.as_str()),
            (2, "<module>")
        );
        let main = go("func TestMain(m *testing.M) {\n\tsetup()\n\tm.Run()\n\tos.Exit(0)\n}");
        assert_eq!(main.scan.sites[0].line, 10);
        assert_eq!(main.scan.sites[0].subject, "TestMain");
    }

    #[test]
    fn a_file_with_syntax_errors_says_so_and_a_cut_parse_has_no_scan() {
        assert!(python("def f(:\n    pass\n", ALL).unwrap().parse_errors);
        assert!(
            go_test_file("package p\nfunc TestMain(m *testing.M) {")
                .unwrap()
                .scan
                .parse_errors
        );
        assert!(js_exit_zero("a.js", "function (").unwrap().parse_errors);
        assert!(rust_exit_zero("fn main( {").unwrap().parse_errors);
        let cut = crate::ast::source_text::with_step_budget(0, || {
            (
                python(&"x = 1\n".repeat(400), ALL),
                go_test_file(&"var x = 1\n".repeat(400)),
                js_exit_zero("a.js", &"let x = 1;\n".repeat(400)),
                rust_exit_zero(&"fn f() {}\n".repeat(400)),
            )
        });
        for why in [
            cut.0.err(),
            cut.1.map(|_| ()).err(),
            cut.2.err(),
            cut.3.err(),
        ] {
            assert!(why.unwrap().contains("did not finish within its budget"));
        }
    }
}
