//! GitHub Actions expression lexing and path extraction.
//!
//! Evaluates the expression syntax used in GitHub Actions workflows (`${{ ... }}` and
//! `if:` expressions). Consolidates lexing between exposure checks (`ci_exposure`) and
//! skip set evaluation (`ci_skip_set`).

/// A token in a GitHub Actions expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Dot,
    Star,
    And,
    Or,
    Not,
    Eq,
    Ne,
    Str(String),
    Num(f64),
    Ident(String),
    Other(char),
}

/// Tokenize a GitHub Actions expression string.
pub fn lex(src: &str) -> Vec<Tok> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' | '\r' => i += 1,
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            '[' => {
                out.push(Tok::LBracket);
                i += 1;
            }
            ']' => {
                out.push(Tok::RBracket);
                i += 1;
            }
            ',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            '.' => {
                out.push(Tok::Dot);
                i += 1;
            }
            '*' => {
                out.push(Tok::Star);
                i += 1;
            }
            '&' if chars.get(i + 1) == Some(&'&') => {
                out.push(Tok::And);
                i += 2;
            }
            '|' if chars.get(i + 1) == Some(&'|') => {
                out.push(Tok::Or);
                i += 2;
            }
            '=' if chars.get(i + 1) == Some(&'=') => {
                out.push(Tok::Eq);
                i += 2;
            }
            '!' if chars.get(i + 1) == Some(&'=') => {
                out.push(Tok::Ne);
                i += 2;
            }
            '!' => {
                out.push(Tok::Not);
                i += 1;
            }
            '\'' => {
                let mut s = String::new();
                i += 1;
                let mut closed = false;
                while i < chars.len() {
                    if chars[i] == '\'' && chars.get(i + 1) == Some(&'\'') {
                        s.push('\'');
                        i += 2;
                    } else if chars[i] == '\'' {
                        i += 1;
                        closed = true;
                        break;
                    } else {
                        s.push(chars[i]);
                        i += 1;
                    }
                }
                if closed {
                    out.push(Tok::Str(s));
                } else {
                    out.push(Tok::Other('\''));
                }
            }
            c if c.is_ascii_digit() => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                if let Ok(n) = text.parse::<f64>() {
                    out.push(Tok::Num(n));
                } else {
                    out.push(Tok::Other(c));
                }
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '-')
                {
                    i += 1;
                }
                out.push(Tok::Ident(chars[start..i].iter().collect()));
            }
            other => {
                out.push(Tok::Other(other));
                i += 1;
            }
        }
    }
    out
}

/// Tokenize for strict parsing (e.g. `ci_skip_set`), failing on invalid/unexpected tokens.
pub fn lex_strict(src: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' | '\r' => i += 1,
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            '[' => {
                out.push(Tok::LBracket);
                i += 1;
            }
            ']' => {
                out.push(Tok::RBracket);
                i += 1;
            }
            ',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            '.' => {
                out.push(Tok::Dot);
                i += 1;
            }
            '*' => {
                out.push(Tok::Star);
                i += 1;
            }
            '&' if chars.get(i + 1) == Some(&'&') => {
                out.push(Tok::And);
                i += 2;
            }
            '|' if chars.get(i + 1) == Some(&'|') => {
                out.push(Tok::Or);
                i += 2;
            }
            '=' if chars.get(i + 1) == Some(&'=') => {
                out.push(Tok::Eq);
                i += 2;
            }
            '!' if chars.get(i + 1) == Some(&'=') => {
                out.push(Tok::Ne);
                i += 2;
            }
            '!' => {
                out.push(Tok::Not);
                i += 1;
            }
            '\'' => {
                let mut s = String::new();
                i += 1;
                loop {
                    match chars.get(i) {
                        None => return Err("unterminated string literal".into()),
                        Some('\'') if chars.get(i + 1) == Some(&'\'') => {
                            s.push('\'');
                            i += 2;
                        }
                        Some('\'') => {
                            i += 1;
                            break;
                        }
                        Some(ch) => {
                            s.push(*ch);
                            i += 1;
                        }
                    }
                }
                out.push(Tok::Str(s));
            }
            c if c.is_ascii_digit() => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                let n = text
                    .parse::<f64>()
                    .map_err(|_| format!("number literal `{text}`"))?;
                out.push(Tok::Num(n));
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '-')
                {
                    i += 1;
                }
                out.push(Tok::Ident(chars[start..i].iter().collect()));
            }
            other => return Err(format!("operator or character `{other}`")),
        }
    }
    Ok(out)
}

/// The property paths an expression reads, lower-cased: `github.event.issue.title`,
/// `inputs.name`. Index syntax (`github['head_ref']`, `x[0]`) is read as a path segment;
/// string literals and function names are not paths.
pub fn expression_paths(expr: &str) -> Vec<Vec<String>> {
    let toks = lex(expr);
    let mut paths = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let rooted = i == 0 || !matches!(toks[i - 1], Tok::Dot | Tok::LBracket);
        let is_call = toks.get(i + 1) == Some(&Tok::LParen);
        if let (Tok::Ident(root), true, false) = (&toks[i], rooted, is_call) {
            let mut path = vec![root.to_ascii_lowercase()];
            i += 1;
            loop {
                match (toks.get(i), toks.get(i + 1)) {
                    (Some(Tok::Dot), Some(Tok::Ident(s))) => {
                        path.push(s.to_ascii_lowercase());
                        i += 2;
                    }
                    (Some(Tok::Dot), Some(Tok::Star)) => {
                        path.push("*".to_string());
                        i += 2;
                    }
                    (Some(Tok::LBracket), _) => {
                        let mut j = i + 1;
                        let mut inner_str = None;
                        while j < toks.len() && toks[j] != Tok::RBracket {
                            if j == i + 1 {
                                match &toks[j] {
                                    Tok::Str(s) => inner_str = Some(s.to_ascii_lowercase()),
                                    Tok::Ident(s) => inner_str = Some(s.to_ascii_lowercase()),
                                    _ => {}
                                }
                            }
                            j += 1;
                        }
                        if j < toks.len() && toks[j] == Tok::RBracket {
                            match (j == i + 2, inner_str) {
                                (true, Some(s)) => path.push(s),
                                _ => path.push("*".to_string()),
                            }
                            i = j + 1;
                        } else {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            paths.push(path);
        } else {
            i += 1;
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexer_tokenizes_expressions() {
        let toks = lex("github.event['pull_request'].body == 'test' && !cancelled()");
        assert!(toks.contains(&Tok::Ident("github".into())));
        assert!(toks.contains(&Tok::Dot));
        assert!(toks.contains(&Tok::Ident("event".into())));
        assert!(toks.contains(&Tok::LBracket));
        assert!(toks.contains(&Tok::Str("pull_request".into())));
        assert!(toks.contains(&Tok::RBracket));
        assert!(toks.contains(&Tok::Eq));
        assert!(toks.contains(&Tok::Str("test".into())));
        assert!(toks.contains(&Tok::And));
        assert!(toks.contains(&Tok::Not));
        assert!(toks.contains(&Tok::Ident("cancelled".into())));
        assert!(toks.contains(&Tok::LParen));
        assert!(toks.contains(&Tok::RParen));
    }

    #[test]
    fn expression_paths_extracts_paths() {
        assert_eq!(
            expression_paths("github.event.issue.title"),
            vec![vec!["github", "event", "issue", "title"]]
        );
        assert_eq!(
            expression_paths("github['head_ref']"),
            vec![vec!["github", "head_ref"]]
        );
        assert_eq!(
            expression_paths("format('{0}', github.event.comment.body)"),
            vec![vec!["github", "event", "comment", "body"]]
        );
        assert_eq!(
            expression_paths("'github.event.issue.title'"),
            Vec::<Vec<String>>::new()
        );
    }

    #[test]
    fn lex_strict_tokenizes_and_rejects_invalid() {
        let toks = lex_strict("github['event_name'] == 'push' && true").unwrap();
        assert_eq!(toks[0], Tok::Ident("github".into()));
        assert_eq!(toks[1], Tok::LBracket);
        assert_eq!(toks[2], Tok::Str("event_name".into()));
        assert_eq!(toks[3], Tok::RBracket);

        let err = lex_strict("github.event ~ 'push'").unwrap_err();
        assert!(err.contains("operator or character `~`"), "{err}");

        let err2 = lex_strict("'unterminated").unwrap_err();
        assert!(err2.contains("unterminated string literal"), "{err2}");
    }
}
