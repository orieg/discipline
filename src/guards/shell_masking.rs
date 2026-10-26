//! Exit-code masking in a shell script, read from its bash syntax tree.
//!
//! Shared by `gate-command-lint` (a plan's verification command) and `ci-integrity`
//! (a GitHub `run:` script, a GitLab job's script lines). The script is parsed with
//! tree-sitter-bash; every judgement is made on command nodes, so text inside a string,
//! a comment or a here-document never counts (`echo "set -o pipefail"` sets nothing,
//! `# || true` masks nothing).
//!
//! A command *cannot fail* when it is `true`, `:` (by any path: `/bin/true`), `exit` or
//! `return` with a literal status that is 0 modulo 256 (`exit 00`), `echo` (whatever
//! its arguments), `printf` with only literal arguments, a `{ ...; }` or `( ... )` group
//! whose last command cannot fail, an `a || b` list where either side cannot fail, an
//! `a && b` list where both cannot, or a pipeline whose status (its last stage, or every
//! stage under `pipefail`) cannot fail.
//!
//! What is reported:
//! - [`MaskKind::OrUnfailable`]: an `a || b` list whose right side cannot fail, unless a
//!   later `&& c` that can fail takes the failure back (`a || echo x && exit 1`).
//! - [`MaskKind::FinalUnfailable`]: when `errexit` is off from the start (a plan's gate
//!   command), the script's last top-level command cannot fail (`cmd; true`, `true`).
//! - [`MaskKind::ErrexitOff`]: `set +e` (or `set +o errexit`) while `errexit` was on, with
//!   no later line that tests or exits with the status it stops acting on.
//! - [`MaskKind::PipelineWithoutPipefail`]: a pipeline with a stage before the last that
//!   can fail, while `pipefail` is off. Which filter the last stage runs does not matter.

use tree_sitter::Node;

/// How a script masks a command's exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskKind {
    OrUnfailable,
    FinalUnfailable,
    ErrexitOff,
    PipelineWithoutPipefail,
}

impl MaskKind {
    /// What the construct does, for a finding's message.
    pub fn describe(self) -> &'static str {
        match self {
            MaskKind::OrUnfailable => "the right side of `||` cannot fail",
            MaskKind::FinalUnfailable => "the last command cannot fail",
            MaskKind::ErrexitOff => {
                "`set +e` turns errexit off and no later line checks the status"
            }
            MaskKind::PipelineWithoutPipefail => {
                "the pipeline exits with its last stage's status, without `pipefail`"
            }
        }
    }
}

/// One masking construct: what it is, its text (first line) and its 1-based line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mask {
    pub kind: MaskKind,
    pub snippet: String,
    pub line: usize,
}

/// What [`analyze`] found in a script.
#[derive(Debug, Clone, Default)]
pub struct Analysis {
    pub masks: Vec<Mask>,
    /// The script did not parse cleanly: what it masks could not be fully verified.
    /// Masks found in the parts that did parse are still reported.
    pub unparsed: bool,
}

impl Analysis {
    /// The first construct that masks an exit status (`||`, a final command, `set +e`),
    /// pipelines aside.
    pub fn exit_masked(&self) -> Option<&Mask> {
        self.masks
            .iter()
            .find(|m| m.kind != MaskKind::PipelineWithoutPipefail)
    }

    /// The first pipeline whose upstream failure is lost.
    pub fn pipeline_masked(&self) -> Option<&Mask> {
        self.masks
            .iter()
            .find(|m| m.kind == MaskKind::PipelineWithoutPipefail)
    }
}

/// How the script is run.
#[derive(Debug, Clone, Copy)]
pub struct ShellMode {
    /// `errexit` (`set -e`) is on when the script starts: GitHub's `bash -e {0}`,
    /// GitLab's `set -eo pipefail`. Off for a plan's gate command run by `sh -c`.
    pub errexit: bool,
    /// `pipefail` is on when the script starts.
    pub pipefail: bool,
}

/// A plan's gate command: `sh -c "<cmd>"`, neither option on.
pub const GATE_COMMAND: ShellMode = ShellMode {
    errexit: false,
    pipefail: false,
};

/// A CI script: `errexit` on. (`pipefail` is not judged for CI scripts.)
pub const CI_SCRIPT: ShellMode = ShellMode {
    errexit: true,
    pipefail: false,
};

/// Replaces each CI template expression (GitHub `${{ ... }}`, GitLab `$[[ ... ]]`) with
/// as many `x` bytes, so the shell parser reads a plain word and no byte offset moves.
pub fn neutralize_ci_expressions(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    loop {
        let next = [("${{", "}}"), ("$[[", "]]")]
            .iter()
            .filter_map(|(open, close)| rest.find(open).map(|at| (at, *open, *close)))
            .min_by_key(|(at, _, _)| *at);
        let Some((at, open, close)) = next else {
            out.push_str(rest);
            return out;
        };
        let Some(end) = rest[at + open.len()..].find(close) else {
            out.push_str(rest);
            return out;
        };
        let end = at + open.len() + end + close.len();
        out.push_str(&rest[..at]);
        // The expression is replaced byte for byte; a newline inside it stays.
        out.extend(rest[at..end].chars().map(|c| {
            if c == '\n' {
                "\n".to_string()
            } else {
                "x".repeat(c.len_utf8())
            }
        }));
        rest = &rest[end..];
    }
}

/// Analyses a CI script (`errexit` on) after [`neutralize_ci_expressions`]. Each mask's
/// snippet is its whole source line as written, template expressions included, so it can
/// be found in the CI file.
pub fn analyze_ci_script(script: &str) -> Analysis {
    let mut a = analyze(&neutralize_ci_expressions(script), CI_SCRIPT);
    for m in &mut a.masks {
        if let Some(line) = script.lines().nth(m.line - 1) {
            m.snippet = line.trim().to_string();
        }
    }
    a
}

/// Analyses `script` run in `mode`.
pub fn analyze(script: &str, mode: ShellMode) -> Analysis {
    let unread = Analysis {
        masks: Vec::new(),
        unparsed: true,
    };
    let Ok(tree) = crate::pretool::parse_shell_tree(script) else {
        return unread;
    };
    let root = tree.root_node();
    // The walks below recurse; a tree nested deeper than any real script is not read.
    if depth_exceeds(root, MAX_DEPTH) {
        return unread;
    }
    let mut a = Analyzer {
        src: script,
        mode,
        options: Vec::new(),
        masks: Vec::new(),
        functions: Vec::new(),
        substitution_depth: 0,
    };
    a.collect_options(root);
    // Twice, so a condition function that calls one defined after it is recognised.
    a.collect_functions(root);
    a.collect_functions(root);
    a.walk(root, false);
    a.final_statement(root);
    a.masks.sort_by_key(|m| m.line);
    Analysis {
        masks: a.masks,
        unparsed: root.has_error(),
    }
}

/// The words of each simple command in `script`, in source order, its name first. A
/// literal word is read as the shell reads it, quotes removed (`'not slow'` is
/// `not slow`); a word that expands at run time is kept as written. `None` when the
/// script does not parse.
pub fn command_words(script: &str) -> Option<Vec<Vec<String>>> {
    let tree = crate::pretool::parse_shell_tree(script).ok()?;
    let root = tree.root_node();
    if root.has_error() || depth_exceeds(root, MAX_DEPTH) {
        return None;
    }
    let read = |n: Node<'_>| literal_word(n, script).unwrap_or_else(|| node_text(n, script));
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "command" {
            let mut words = Vec::new();
            if let Some(name) = node.child_by_field_name("name") {
                words.push(read(
                    named_children(name).into_iter().next().unwrap_or(name),
                ));
            }
            let mut c = node.walk();
            words.extend(node.children_by_field_name("argument", &mut c).map(read));
            out.push((node.start_byte(), words));
        }
        let mut c = node.walk();
        stack.extend(node.named_children(&mut c));
    }
    out.sort_by_key(|(at, _)| *at);
    Some(out.into_iter().map(|(_, words)| words).collect())
}

fn node_text(n: Node<'_>, src: &str) -> String {
    src.get(n.start_byte()..n.end_byte())
        .unwrap_or("")
        .to_string()
}

/// A word as the shell reads it, quotes removed; `None` when it expands at run time.
fn literal_word(n: Node<'_>, src: &str) -> Option<String> {
    let unquoted = |n: Node<'_>| {
        let t = node_text(n, src);
        t.get(1..t.len().saturating_sub(1))
            .unwrap_or("")
            .to_string()
    };
    match n.kind() {
        "word" | "number" => Some(node_text(n, src)),
        "raw_string" => Some(unquoted(n)),
        "string" => named_children(n)
            .iter()
            .all(|c| c.kind() == "string_content")
            .then(|| unquoted(n)),
        "concatenation" => named_children(n)
            .into_iter()
            .map(|c| literal_word(c, src))
            .collect::<Option<Vec<_>>>()
            .map(|parts| parts.concat()),
        _ => None,
    }
}

/// Deepest syntax tree the analysis reads.
const MAX_DEPTH: usize = 256;

/// Whether any node of the tree is more than `limit` levels below `root` (iterative).
fn depth_exceeds(root: Node<'_>, limit: usize) -> bool {
    let mut cursor = root.walk();
    let mut depth = 0usize;
    loop {
        if depth > limit {
            return true;
        }
        if cursor.goto_first_child() {
            depth += 1;
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return false;
            }
            depth -= 1;
        }
    }
}

/// A `set` command's effect at a byte offset.
#[derive(Debug, Clone, Copy)]
struct OptionChange {
    at: usize,
    errexit: Option<bool>,
    pipefail: Option<bool>,
}

struct Analyzer<'s> {
    src: &'s str,
    mode: ShellMode,
    options: Vec<OptionChange>,
    masks: Vec<Mask>,
    /// Names of the functions the script defines whose bodies are conditions only: a
    /// call to one is a test the script branches on, not a verification command.
    functions: Vec<String>,
    /// How many command substitutions (`$(...)`, backquotes) enclose the node walked.
    substitution_depth: usize,
}

fn named_children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut c = node.walk();
    node.named_children(&mut c)
        .filter(|n| n.kind() != "comment" && n.kind() != "heredoc_body")
        .collect()
}

impl<'s> Analyzer<'s> {
    fn text(&self, node: Node<'_>) -> &'s str {
        self.src
            .get(node.start_byte()..node.end_byte())
            .unwrap_or("")
    }

    fn mask(&mut self, kind: MaskKind, node: Node<'_>) {
        let snippet = self
            .text(node)
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        self.masks.push(Mask {
            kind,
            snippet,
            line: node.start_position().row + 1,
        });
    }

    /// The literal value of an argument node, or None when it expands at run time.
    fn literal(&self, node: Node<'_>) -> Option<String> {
        match node.kind() {
            "word" | "number" => Some(self.text(node).to_string()),
            "raw_string" => Some(self.text(node).trim_matches('\'').to_string()),
            "string" => named_children(node)
                .iter()
                .all(|c| c.kind() == "string_content")
                .then(|| self.text(node).trim_matches('"').to_string()),
            _ => None,
        }
    }

    /// The command's name with any directory stripped, and its argument nodes, skipping
    /// `command` / `builtin` / `exec` prefixes. None when the name expands at run time.
    fn name_and_args<'t>(&self, cmd: Node<'t>) -> Option<(String, Vec<Node<'t>>)> {
        let name = cmd.child_by_field_name("name")?;
        let name_word = named_children(name).into_iter().next().unwrap_or(name);
        let mut words: Vec<Node<'t>> = vec![name_word];
        let mut c = cmd.walk();
        words.extend(cmd.children_by_field_name("argument", &mut c));
        loop {
            let first = self.literal(*words.first()?)?;
            let base = first.rsplit('/').next().unwrap_or(&first).to_string();
            if matches!(base.as_str(), "command" | "builtin" | "exec") && words.len() > 1 {
                words.remove(0);
                continue;
            }
            words.remove(0);
            return Some((base, words));
        }
    }

    fn option_state(&self, at: usize) -> (bool, bool) {
        let (mut errexit, mut pipefail) = (self.mode.errexit, self.mode.pipefail);
        for ch in self.options.iter().filter(|c| c.at < at) {
            if let Some(e) = ch.errexit {
                errexit = e;
            }
            if let Some(p) = ch.pipefail {
                pipefail = p;
            }
        }
        (errexit, pipefail)
    }

    /// Records every `set` command's effect on `errexit` and `pipefail`.
    fn collect_options(&mut self, node: Node<'_>) {
        if node.kind() == "command" {
            if let Some((name, args)) = self.name_and_args(node) {
                if name == "set" {
                    let change = self.set_change(node.start_byte(), &args);
                    self.options.push(change);
                }
            }
        }
        for child in named_children(node) {
            self.collect_options(child);
        }
    }

    fn set_change(&self, at: usize, args: &[Node<'_>]) -> OptionChange {
        let mut change = OptionChange {
            at,
            errexit: None,
            pipefail: None,
        };
        let words: Vec<String> = args
            .iter()
            .map(|a| self.literal(*a).unwrap_or_default())
            .collect();
        let mut i = 0;
        while i < words.len() {
            let w = &words[i];
            if w == "--" {
                break;
            }
            let on = match w.chars().next() {
                Some('-') => true,
                Some('+') => false,
                _ => break,
            };
            let flags = &w[1..];
            if flags.contains('e') {
                change.errexit = Some(on);
            }
            if flags.contains('o') {
                match words.get(i + 1).map(String::as_str) {
                    Some("pipefail") => change.pipefail = Some(on),
                    Some("errexit") => change.errexit = Some(on),
                    _ => {}
                }
                i += 1;
            }
            i += 1;
        }
        change
    }

    /// Whether `node` prints a fallback value (`echo`, `printf`): inside a command
    /// substitution, `X=$(git merge-base a b || echo "")` supplies a default that the
    /// script reads, rather than masking a check. `|| true` and `|| exit 0` there
    /// still mask.
    fn supplies_a_value(&self, node: Node<'_>) -> bool {
        node.kind() == "command"
            && self
                .name_and_args(node)
                .is_some_and(|(name, _)| matches!(name.as_str(), "echo" | "printf"))
    }

    /// Records each function the script defines whose body is made of conditions only
    /// (`edited() { case ... esac; return 1; }`). A function that runs a verification
    /// command (`t() { cargo test; }`) is not one, so `t || echo ok` still masks.
    fn collect_functions(&mut self, node: Node<'_>) {
        if node.kind() == "function_definition" {
            if let (Some(name), Some(body)) = (
                node.child_by_field_name("name"),
                node.child_by_field_name("body"),
            ) {
                let name = self.text(name).to_string();
                if !self.functions.contains(&name) && self.only_conditions(body) {
                    self.functions.push(name);
                }
            }
        }
        for child in named_children(node) {
            self.collect_functions(child);
        }
    }

    /// Whether every command whose failure can reach `node`'s exit status is a
    /// condition rather than a verification command: a test (`[`, `[[`, `test`,
    /// `((`), an existence check (`command -v`, `type`, `hash`), a negated command,
    /// a `case`, a `return` with a literal status, or a call to a function whose body
    /// is made of those. `p && echo yes || echo no` and
    /// `[ -f f ] || echo missing` branch on a condition; they mask no check.
    fn only_conditions(&self, node: Node<'_>) -> bool {
        if self.cannot_fail(node) {
            return true;
        }
        match node.kind() {
            "test_command" | "case_statement" => true,
            "negated_command" => named_children(node)
                .first()
                .is_some_and(|inner| self.only_conditions(*inner)),
            "command" => {
                let Some(name) = node.child_by_field_name("name") else {
                    return false;
                };
                let first = self.text(name);
                if matches!(first, "[" | "test" | "type" | "hash") {
                    return true;
                }
                if first == "return" {
                    let mut c = node.walk();
                    let args: Vec<_> = node.children_by_field_name("argument", &mut c).collect();
                    return args.len() == 1 && self.literal(args[0]).is_some();
                }
                if first == "command" {
                    let mut c = node.walk();
                    return node
                        .children_by_field_name("argument", &mut c)
                        .next()
                        .is_some_and(|a| matches!(self.text(a), "-v" | "-V"));
                }
                self.functions.iter().any(|f| f == first)
            }
            "redirected_statement" => node
                .child_by_field_name("body")
                .is_some_and(|b| self.only_conditions(b)),
            "subshell" | "compound_statement" => named_children(node)
                .last()
                .is_some_and(|last| self.only_conditions(*last)),
            "list" => {
                let parts = named_children(node);
                let (Some(l), Some(r)) = (parts.first(), parts.last()) else {
                    return false;
                };
                match list_operator(node) {
                    Some("&&") => self.only_conditions(*l) && self.only_conditions(*r),
                    Some("||") => self.only_conditions(*r),
                    _ => false,
                }
            }
            "pipeline" => {
                let stages = named_children(node);
                let (_, pipefail) = self.option_state(node.start_byte());
                if pipefail {
                    stages.iter().all(|s| self.only_conditions(*s))
                } else {
                    stages.last().is_some_and(|s| self.only_conditions(*s))
                }
            }
            _ => false,
        }
    }

    /// Whether `node`'s exit status is 0 however the commands in it behave.
    fn cannot_fail(&self, node: Node<'_>) -> bool {
        match node.kind() {
            "command" => {
                let Some((name, args)) = self.name_and_args(node) else {
                    return false;
                };
                match name.as_str() {
                    "true" | ":" | "echo" => true,
                    "printf" => args.iter().all(|a| self.literal(*a).is_some()),
                    "exit" | "return" => args.first().is_some_and(|a| {
                        self.literal(*a)
                            .and_then(|v| v.trim().parse::<u64>().ok())
                            .is_some_and(|v| v % 256 == 0)
                    }),
                    _ => false,
                }
            }
            "redirected_statement" => node
                .child_by_field_name("body")
                .is_some_and(|b| self.cannot_fail(b)),
            "subshell" | "compound_statement" => named_children(node)
                .last()
                .is_some_and(|last| self.cannot_fail(*last)),
            "list" => {
                let parts = named_children(node);
                let (Some(l), Some(r)) = (parts.first(), parts.last()) else {
                    return false;
                };
                match list_operator(node) {
                    Some("||") => self.cannot_fail(*l) || self.cannot_fail(*r),
                    Some("&&") => self.cannot_fail(*l) && self.cannot_fail(*r),
                    _ => false,
                }
            }
            "pipeline" => {
                let stages = named_children(node);
                let (_, pipefail) = self.option_state(node.start_byte());
                if pipefail {
                    stages.iter().all(|s| self.cannot_fail(*s))
                } else {
                    stages.last().is_some_and(|s| self.cannot_fail(*s))
                }
            }
            _ => false,
        }
    }

    /// Walks the tree top-down. `restored` says the success this node yields is taken
    /// back by an enclosing `&& c` whose `c` can fail (the node is the left operand of
    /// that list, through any `||` / `&&` lists between): `a || echo x && exit 1` still
    /// fails when `a` does.
    fn walk(&mut self, node: Node<'_>, restored: bool) {
        if node.kind() == "list" {
            let parts = named_children(node);
            let op = list_operator(node);
            if let (Some(l), Some(r)) = (parts.first(), parts.last()) {
                if op == Some("||")
                    && self.cannot_fail(*r)
                    && !restored
                    && !self.only_conditions(*l)
                    && !(self.substitution_depth > 0 && self.supplies_a_value(*r))
                {
                    self.mask(MaskKind::OrUnfailable, node);
                }
                let left_restored = restored || (op == Some("&&") && !self.cannot_fail(*r));
                self.walk(*l, left_restored);
                if l.id() != r.id() {
                    self.walk(*r, false);
                }
            }
            return;
        }
        if node.kind() == "command_substitution" {
            self.substitution_depth += 1;
            for child in named_children(node) {
                self.walk(child, false);
            }
            self.substitution_depth -= 1;
            return;
        }
        match node.kind() {
            "pipeline" => {
                let stages = named_children(node);
                let (_, pipefail) = self.option_state(node.start_byte());
                if !pipefail
                    && stages.len() > 1
                    && stages[..stages.len() - 1]
                        .iter()
                        .any(|s| !self.cannot_fail(*s))
                {
                    self.mask(MaskKind::PipelineWithoutPipefail, node);
                }
            }
            "command" => {
                if let Some((name, args)) = self.name_and_args(node) {
                    if name == "set" {
                        let change = self.set_change(node.start_byte(), &args);
                        let (errexit, _) = self.option_state(node.start_byte());
                        let tail = &self.src[node.start_byte()..];
                        if errexit
                            && change.errexit == Some(false)
                            && !status_checked(&tail.lines().collect::<Vec<_>>())
                        {
                            self.mask(MaskKind::ErrexitOff, node);
                        }
                    }
                }
            }
            _ => {}
        }
        for child in named_children(node) {
            self.walk(child, false);
        }
    }

    /// With `errexit` off from the start and never turned on, the script exits with its
    /// last top-level command's status.
    fn final_statement(&mut self, root: Node<'_>) {
        if self.mode.errexit || self.options.iter().any(|c| c.errexit.is_some()) {
            return;
        }
        if let Some(last) = named_children(root).last() {
            if self.cannot_fail(*last) {
                self.mask(MaskKind::FinalUnfailable, *last);
            }
        }
    }
}

fn list_operator(list: Node<'_>) -> Option<&'static str> {
    let mut c = list.walk();
    let op = list
        .children(&mut c)
        .find(|n| !n.is_named() && matches!(n.kind(), "&&" | "||"))
        .map(|n| n.kind());
    op
}

/// Whether the lines from a `set +e` on check the status it stops acting on: `$?` is
/// saved into a variable that a later line tests or exits with, or a later line tests
/// or exits with `$?` itself. `set +e; cmd; rc=$?; set -e; [ "$rc" -eq 0 ]` is a checked
/// negative control, not a masked failure.
pub fn status_checked(lines: &[&str]) -> bool {
    let tests = |l: &str| {
        let l = l.trim_start();
        ["if ", "[ ", "[[ ", "test ", "exit ", "case ", "((", "elif "]
            .iter()
            .any(|k| l.starts_with(k) || l.contains(&format!("; {k}")))
    };
    let assign = regex::Regex::new(r#"\b([A-Za-z_][A-Za-z0-9_]*)="?\$\?"?"#).expect("static regex");
    let mut vars: Vec<String> = Vec::new();
    for l in lines {
        if tests(l) {
            if l.contains("$?") {
                return true;
            }
            if vars
                .iter()
                .any(|v| l.contains(&format!("${v}")) || l.contains(&format!("${{{v}}}")))
            {
                return true;
            }
        }
        for c in assign.captures_iter(l) {
            vars.push(c[1].to_string());
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate(cmd: &str) -> Analysis {
        analyze(cmd, GATE_COMMAND)
    }
    fn ci(cmd: &str) -> Analysis {
        analyze(cmd, CI_SCRIPT)
    }

    #[test]
    fn command_words_are_read_with_quotes_removed() {
        let words = |s: &str| command_words(s).expect("parses");
        assert_eq!(
            words("-q -k 'not test_f' -m \"not slow\" --x=a'b'"),
            [vec!["-q", "-k", "not test_f", "-m", "not slow", "--x=ab"]]
        );
        assert_eq!(
            words("jest --ci || true; exit 0"),
            [vec!["jest", "--ci"], vec!["true"], vec!["exit", "0"]]
        );
        // A word that expands at run time is kept as written; text in a comment is no word.
        assert_eq!(
            words("jest \"$ARGS\" # --passWithNoTests"),
            [vec!["jest", "\"$ARGS\""]]
        );
        assert!(command_words("jest 'unclosed").is_none());
    }

    #[test]
    fn or_with_a_right_side_that_cannot_fail_masks() {
        for cmd in [
            "cargo test || true",
            "cargo test ||true",
            "cargo test || :",
            "cargo test || /bin/true",
            "cargo test || /usr/bin/true",
            "cargo test || exit 0",
            "cargo test || exit 00",
            "cargo test || exit 256",
            "cargo test || exit '0'",
            "cargo test || echo ok",
            "cargo test || echo \"failed with $?\"",
            "cargo test || printf 'failed\\n'",
            "cargo test || { true; }",
            "cargo test || { echo failed; true; }",
            "cargo test || ( exit 0 )",
            "(cargo test || true)",
            "{ cargo test || true; }",
            "cargo test || true > /dev/null",
            "cargo test || command true",
            "cargo test && cargo clippy || true",
            "cargo test || false || true",
        ] {
            for a in [gate(cmd), ci(cmd)] {
                assert_eq!(
                    a.exit_masked().map(|m| m.kind),
                    Some(MaskKind::OrUnfailable),
                    "`{cmd}`: {a:?}"
                );
                assert!(!a.unparsed, "`{cmd}`");
            }
        }
    }

    #[test]
    fn a_branch_on_a_condition_is_not_masked_but_a_wrapped_check_is() {
        // Negative controls: the left side of `||` only tests a condition.
        for script in [
            "[ -f Cargo.lock ] || echo missing",
            "[[ -n \"$X\" ]] || echo unset",
            "test -d target || echo none",
            "command -v cargo >/dev/null || echo 'no cargo'",
            "! [ -f x ] || echo present",
            "BASE=$(git merge-base origin/main HEAD 2>/dev/null || echo \"\")",
            "V=`git describe --tags || printf none`",
            "edited() { case \"$changed\" in *\" $1 \"*) return 0;; esac; return 1; }\ned() { edited \"$1\" && echo true || echo false; }",
        ] {
            assert!(ci(script).exit_masked().is_none(), "{script}");
        }
        // Positive controls: a verification command, directly or inside a function.
        for script in [
            "cargo test || echo failed",
            "t() { cargo test; }\nt || echo ok",
            "t() { cargo test; } ; t && echo yes || echo no",
            "! cargo test || echo ok",
            "[ -f x ] && cargo test || echo skipped",
            "OUT=$(cargo test || true)",
            "OUT=$(cargo test || exit 0)",
        ] {
            assert!(ci(script).exit_masked().is_some(), "{script}");
        }
    }

    #[test]
    fn a_failure_that_still_propagates_is_not_masked() {
        for cmd in [
            "cargo test",
            "cargo test || exit 1",
            "cargo test || exit",
            "cargo test || exit $?",
            "cargo test || { echo failed; exit 1; }",
            "cargo test || echo failed && exit 1",
            "cargo test || rc=$?",
            "cargo test || printf \"$fmt\"",
            "cargo test || retry cargo test",
            "! cargo check",
            "grep -q '|| true' notes.txt",
            "cargo test # || true",
            "cargo test && true",
        ] {
            for a in [gate(cmd), ci(cmd)] {
                assert_eq!(a.exit_masked(), None, "`{cmd}`: {a:?}");
            }
        }
    }

    #[test]
    fn a_final_command_that_cannot_fail_masks_a_gate_command_only() {
        for cmd in [
            "cargo test; true",
            "cargo test ; :",
            "cargo test ; exit 0",
            "true",
            ":",
        ] {
            assert_eq!(
                gate(cmd).exit_masked().map(|m| m.kind),
                Some(MaskKind::FinalUnfailable),
                "`{cmd}`"
            );
            // Under errexit (CI) the first failure ends the script before `true` runs.
            assert_eq!(ci(cmd).exit_masked(), None, "`{cmd}`");
        }
        // errexit turned on by the command itself.
        assert_eq!(gate("set -e; cargo test; true").exit_masked(), None);
        assert_eq!(gate("cargo test; cargo clippy").exit_masked(), None);
    }

    #[test]
    fn set_plus_e_masks_unless_the_status_is_checked() {
        let masked = ci("set +e\n./smoke.sh\necho done\n");
        assert_eq!(
            masked.exit_masked().map(|m| m.kind),
            Some(MaskKind::ErrexitOff)
        );
        assert_eq!(masked.exit_masked().map(|m| m.line), Some(1));
        assert_eq!(
            ci("set +o errexit\n./smoke.sh\n")
                .exit_masked()
                .map(|m| m.kind),
            Some(MaskKind::ErrexitOff)
        );
        let checked = "set +e\n./smoke.sh\nrc=$?\nset -e\nif [ \"$rc\" -eq 0 ]; then exit 1; fi\n";
        assert_eq!(ci(checked).exit_masked(), None);
        // Text that only mentions it.
        assert_eq!(
            ci("echo 'set +e'\n# set +e\ncargo test\n").exit_masked(),
            None
        );
        // errexit already off: nothing to turn off.
        assert_eq!(gate("set +e; cargo test").exit_masked(), None);
    }

    #[test]
    fn a_pipeline_without_pipefail_masks_whatever_the_filter() {
        for cmd in [
            "cargo test | grep passed",
            "cargo test | xargs echo",
            "cargo test | python3 summarize.py",
            "echo \"set -o pipefail\"; cargo test | tee log",
            "set +o pipefail; cargo test | tee log",
        ] {
            assert_eq!(
                gate(cmd).pipeline_masked().map(|m| m.kind),
                Some(MaskKind::PipelineWithoutPipefail),
                "`{cmd}`"
            );
        }
        for cmd in [
            "set -o pipefail; cargo test | grep passed",
            "set -euo pipefail; cargo test | xargs echo",
            "set -eo pipefail\ncargo test | tee log",
            "echo y | cargo test",
            "printf 'y\\n' | cargo test",
        ] {
            assert_eq!(gate(cmd).pipeline_masked(), None, "`{cmd}`");
        }
    }

    #[test]
    fn a_script_that_does_not_parse_is_reported_unparsed() {
        assert!(gate("cargo test || (").unparsed);
        assert!(gate("if cargo test; then").unparsed);
        assert!(!gate("cargo test").unparsed);
        // Nested deeper than the analysis reads: not verified, never a silent pass.
        let deep = format!("{}cargo test || true{}", "( ".repeat(300), " )".repeat(300));
        assert!(gate(&deep).unparsed);
        let shallow = format!("{}cargo test || true{}", "( ".repeat(20), " )".repeat(20));
        assert!(!gate(&shallow).unparsed);
        assert!(gate(&shallow).exit_masked().is_some());
    }

    #[test]
    fn ci_expressions_are_neutralized_byte_for_byte() {
        let s = "${{ matrix.cmd }} || true\necho $[[ inputs.x ]]";
        let n = neutralize_ci_expressions(s);
        assert_eq!(n.len(), s.len());
        assert_eq!(n, "xxxxxxxxxxxxxxxxx || true\necho xxxxxxxxxxxxxxx");
        let a = ci(&n);
        assert!(!a.unparsed);
        assert_eq!(
            a.exit_masked().map(|m| m.kind),
            Some(MaskKind::OrUnfailable)
        );
        assert_eq!(neutralize_ci_expressions("a ${{ b"), "a ${{ b");
    }

    #[test]
    fn non_ascii_text_keeps_offsets() {
        let a = ci("echo 'h\u{e9}llo'\ncargo test || true");
        assert_eq!(a.exit_masked().map(|m| m.line), Some(2));
        assert_eq!(
            a.exit_masked().map(|m| m.snippet.as_str()),
            Some("cargo test || true")
        );
    }
}
