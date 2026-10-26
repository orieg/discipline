//! pytest's selection options in `addopts`: `-k EXPRESSION` (keyword) and `-m MARKEXPR`
//! (marker). Either one deselects every test its expression does not match, as
//! `--deselect` does by node id.
//!
//! The words of `addopts` are read with the shell grammar (pytest splits the option as a
//! shell does), and each expression with pytest's own grammar: `or`, `and`, `not`,
//! parentheses, and names. Whether the head expression selects fewer tests than the base
//! one is decided over every truth assignment of the names: it does when some
//! assignment the base expression accepts is one the head expression rejects. A `-k`
//! name matches a test whose name contains it, case-insensitively, so an assignment
//! where a name holds and a name it contains does not is skipped.

use serde_json::Value;

/// Names an expression may hold before its truth table is not built.
const MAX_NAMES: usize = 14;

/// The selection options pytest reads, with the name each is reported under.
const OPTIONS: &[(&str, &str)] = &[("-k", "keyword"), ("-m", "marker")];

/// The words of an `addopts` value. A string is split as a shell splits it; a list entry
/// that starts with `-` is split the same way (an INI continuation line), and any other
/// entry is one argument (a TOML list such as `["-k", "not slow"]`).
fn addopts_words(v: &Value) -> Vec<String> {
    let split = |s: &str| match crate::guards::shell_masking::command_words(s) {
        Some(commands) => commands.concat(),
        None => s.split_whitespace().map(str::to_string).collect(),
    };
    match v {
        Value::String(s) => split(s),
        Value::Array(a) => a
            .iter()
            .flat_map(|x| match x {
                Value::String(s) if s.trim_start().starts_with('-') => split(s),
                Value::String(s) => vec![s.clone()],
                other => vec![other.to_string()],
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The expression of the last `option` in `words` (pytest keeps the last): `-k EXPR`,
/// `-kEXPR` or `-k=EXPR`. An empty expression selects every test, so it is no
/// selection.
fn selection(words: &[String], option: &str) -> Option<String> {
    let mut found = None;
    let mut i = 0;
    while i < words.len() {
        let w = &words[i];
        if w == option {
            found = words.get(i + 1).cloned();
            i += 2;
            continue;
        }
        if let Some(rest) = w.strip_prefix(option) {
            found = Some(rest.strip_prefix('=').unwrap_or(rest).to_string());
        }
        i += 1;
    }
    found.filter(|e| !e.trim().is_empty())
}

#[derive(Debug, Clone, PartialEq)]
enum Expr {
    Name(usize),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Open,
    Close,
    Or,
    And,
    Not,
    Name(String),
}

/// A character pytest allows in a name.
fn name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | ':' | '+' | '-' | '.' | '[' | ']' | '\\' | '/')
}

fn tokens(src: &str) -> Option<Vec<Token>> {
    let mut out = Vec::new();
    let mut chars = src.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '(' {
            chars.next();
            out.push(Token::Open);
        } else if c == ')' {
            chars.next();
            out.push(Token::Close);
        } else if name_char(c) {
            let mut name = String::new();
            while let Some(&c) = chars.peek().filter(|c| name_char(**c)) {
                name.push(c);
                chars.next();
            }
            out.push(match name.as_str() {
                "or" => Token::Or,
                "and" => Token::And,
                "not" => Token::Not,
                _ => Token::Name(name),
            });
        } else {
            return None;
        }
    }
    Some(out)
}

/// A recursive-descent reader of pytest's expression grammar:
/// `expr := and ('or' and)*`, `and := not ('and' not)*`,
/// `not := 'not' not | '(' expr ')' | name`.
struct Reader<'a> {
    tokens: &'a [Token],
    at: usize,
    names: &'a mut Vec<String>,
    fold_case: bool,
}

impl Reader<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn expr(&mut self, depth: usize) -> Option<Expr> {
        let mut left = self.and(depth)?;
        while self.peek() == Some(&Token::Or) {
            self.at += 1;
            left = Expr::Or(Box::new(left), Box::new(self.and(depth)?));
        }
        Some(left)
    }

    fn and(&mut self, depth: usize) -> Option<Expr> {
        let mut left = self.not(depth)?;
        while self.peek() == Some(&Token::And) {
            self.at += 1;
            left = Expr::And(Box::new(left), Box::new(self.not(depth)?));
        }
        Some(left)
    }

    fn not(&mut self, depth: usize) -> Option<Expr> {
        if depth > 64 {
            return None;
        }
        let token = self.peek()?.clone();
        self.at += 1;
        match token {
            Token::Not => Some(Expr::Not(Box::new(self.not(depth + 1)?))),
            Token::Open => {
                let inner = self.expr(depth + 1)?;
                (self.peek() == Some(&Token::Close)).then_some(())?;
                self.at += 1;
                Some(inner)
            }
            Token::Name(name) => {
                let name = if self.fold_case {
                    name.to_lowercase()
                } else {
                    name
                };
                let index = match self.names.iter().position(|n| *n == name) {
                    Some(i) => i,
                    None => {
                        self.names.push(name);
                        self.names.len() - 1
                    }
                };
                Some(Expr::Name(index))
            }
            _ => None,
        }
    }
}

fn parse(src: &str, names: &mut Vec<String>, fold_case: bool) -> Option<Expr> {
    let tokens = tokens(src)?;
    let mut reader = Reader {
        tokens: &tokens,
        at: 0,
        names,
        fold_case,
    };
    let e = reader.expr(0)?;
    (reader.at == tokens.len()).then_some(e)
}

fn eval(e: &Expr, bits: u32) -> bool {
    match e {
        Expr::Name(i) => bits & (1 << i) != 0,
        Expr::Not(x) => !eval(x, bits),
        Expr::And(a, b) => eval(a, bits) && eval(b, bits),
        Expr::Or(a, b) => eval(a, bits) || eval(b, bits),
    }
}

/// Whether `head` rejects a test `base` accepts. `None` when either expression cannot be
/// read, or holds more names than the truth table is built for.
fn narrows(base: &str, head: &str, option: &str) -> Option<bool> {
    let keyword = option == "-k";
    let mut names = Vec::new();
    let b = parse(base, &mut names, keyword)?;
    let h = parse(head, &mut names, keyword)?;
    if names.len() > MAX_NAMES {
        return None;
    }
    // `-k`: a test whose name contains `a_b` also contains `a`.
    let implied: Vec<(usize, usize)> = if keyword {
        let mut pairs = Vec::new();
        for (i, long) in names.iter().enumerate() {
            for (j, short) in names.iter().enumerate() {
                if i != j && long.contains(short.as_str()) {
                    pairs.push((i, j));
                }
            }
        }
        pairs
    } else {
        Vec::new()
    };
    Some((0..1u32 << names.len()).any(|bits| {
        let possible = implied
            .iter()
            .all(|(long, short)| bits & (1 << long) == 0 || bits & (1 << short) != 0);
        possible && eval(&b, bits) && !eval(&h, bits)
    }))
}

/// What the head `addopts` deselects that the base one did not: a `-k` or `-m`
/// expression gained, or one that now rejects a test the base expression accepted. An
/// expression that changed and cannot be read is reported as changed.
pub(super) fn deselected(base: Option<&Value>, head: Option<&Value>) -> Vec<String> {
    let (bw, hw) = (
        base.map(addopts_words).unwrap_or_default(),
        head.map(addopts_words).unwrap_or_default(),
    );
    let mut out = Vec::new();
    for (option, kind) in OPTIONS {
        let Some(h) = selection(&hw, option) else {
            continue;
        };
        match selection(&bw, option) {
            None => out.push(format!(
                "gained `{option} {h}` (a test its {kind} expression does not match is deselected)"
            )),
            Some(b) if b.split_whitespace().eq(h.split_whitespace()) => {}
            Some(b) => match narrows(&b, &h, option) {
                Some(false) => {}
                Some(true) => out.push(format!(
                    "narrowed `{option}` from `{b}` to `{h}` (a test the base {kind} expression matched is now deselected)"
                )),
                None => out.push(format!(
                    "changed `{option}` from `{b}` to `{h}`, a {kind} expression that could not be read"
                )),
            },
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(base: Option<&str>, head: &str) -> Vec<String> {
        let b = base.map(|s| Value::String(s.to_string()));
        deselected(b.as_ref(), Some(&Value::String(head.to_string())))
    }

    #[test]
    fn a_gained_keyword_or_marker_expression_deselects() {
        assert_eq!(
            found(None, "-ra -k 'not test_f'"),
            ["gained `-k not test_f` (a test its keyword expression does not match is deselected)"]
        );
        assert_eq!(
            found(Some("-q"), "-q -m \"not slow\""),
            ["gained `-m not slow` (a test its marker expression does not match is deselected)"]
        );
        // `-kEXPR` and `-k=EXPR` are the same option.
        assert_eq!(found(None, "-ktest_g").len(), 1);
        assert_eq!(found(None, "-m=integration").len(), 1);
        // A TOML list: each entry is one argument.
        let list = serde_json::json!(["-q", "-k", "not test_f"]);
        assert_eq!(deselected(None, Some(&list)).len(), 1);
        // No selection, an empty one, or a long option that only starts with `-m`.
        assert!(found(None, "-q --strict-markers").is_empty());
        assert!(found(None, "-k ''").is_empty());
        assert!(found(None, "--maxfail=1").is_empty());
    }

    #[test]
    fn an_expression_that_selects_fewer_tests_is_narrowed() {
        // A `not` term gained, an `or` alternative lost, an `and` term gained.
        assert_eq!(found(Some("-k 'a or b'"), "-k 'a or b and not c'").len(), 1);
        assert_eq!(found(Some("-m 'unit or integration'"), "-m unit").len(), 1);
        assert_eq!(found(Some("-m unit"), "-m 'unit and not slow'").len(), 1);
        assert_eq!(found(Some("-k test"), "-k test_f").len(), 1);
        // An `or` alternative gained selects more; a `not` term dropped selects more; a
        // reformatted or reordered expression selects the same tests.
        assert!(found(Some("-k 'a or b'"), "-k 'a or b or c'").is_empty());
        assert!(found(Some("-m 'unit and not slow'"), "-m unit").is_empty());
        assert!(found(Some("-k '(a or b)'"), "-k 'b  or a'").is_empty());
        assert!(found(Some("-k 'not test_f'"), "-q").is_empty());
        // A `-k` name that contains another selects no more than it does.
        assert!(found(Some("-k test_f"), "-k test").is_empty());
        assert!(found(Some("-k TEST_F"), "-k test_f").is_empty());
    }

    #[test]
    fn an_expression_that_cannot_be_read_is_reported_as_changed() {
        assert_eq!(
            found(Some("-m slow"), "-m 'slow(x=1)'"),
            ["changed `-m` from `slow` to `slow(x=1)`, a marker expression that could not be read"]
        );
        assert!(found(Some("-m 'slow(x=1)'"), "-m 'slow(x=1)'").is_empty());
        assert!(parse("a or", &mut Vec::new(), false).is_none());
        assert!(parse("(a", &mut Vec::new(), false).is_none());
    }
}
