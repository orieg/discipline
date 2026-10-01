//! The fixed-format credential table shared by `pii` (`secrets = true`) and
//! `shell-secrets`.
//!
//! One table, so a format added here is checked in every text file by `pii` and in shell,
//! Dockerfile and CI files by `shell-secrets`. Only formats with a fixed prefix and
//! shape belong here. The heuristic rules (password flags, named-secret assignments,
//! environment-variable argv) stay in `shell-secrets`: they guess at intent and carry a
//! warning severity, which a whole-tree gate must not inherit.
//!
//! Nothing here measures entropy: a string is reported because it has a provider's token
//! shape, never because it looks random.

use anyhow::Result;
use regex::{Captures, Regex};

/// One fixed-format credential class.
pub struct TokenClass {
    /// Stable internal id; `shell-secrets` maps it to its rule id.
    pub id: &'static str,
    /// What `pii` reports: the token class, never the value.
    pub label: &'static str,
    /// The regular expression. Anchored on the provider's prefix.
    pub pattern: &'static str,
    /// Capture group holding the credential value. A match whose value is a variable
    /// reference or a documented placeholder (`$TOKEN`, `${{ secrets.X }}`, `<token>`) is
    /// not a hit. `None`: the prefix itself is the evidence.
    pub value_group: Option<usize>,
}

/// Every fixed-format class, in the order `shell-secrets` reports them.
///
/// Where `pii` and `shell-secrets` used to differ, the table holds the union:
/// AWS prefixes `AKIA|ASIA|ABIA|ACCA`, GitHub `gh[pousr]_` with no upper length bound plus
/// `github_pat_`, Slack `xox[baprs]-`, private-key headers with any label words.
pub const TOKEN_CLASSES: &[TokenClass] = &[
    TokenClass {
        id: "github-token",
        label: "GitHub token",
        pattern: r"\b(?:gh[pousr]_[A-Za-z0-9_]{36,}|github_pat_[A-Za-z0-9_]{82})\b",
        value_group: None,
    },
    TokenClass {
        id: "aws-access-key",
        label: "AWS access key ID",
        pattern: r"\b(?:AKIA|ASIA|ABIA|ACCA)[0-9A-Z]{16}\b",
        value_group: None,
    },
    TokenClass {
        id: "slack-token",
        label: "Slack token",
        pattern: r"\bxox[baprs]-[0-9a-zA-Z-]{10,}\b",
        value_group: None,
    },
    TokenClass {
        id: "llm-api-token",
        label: "OpenAI or Anthropic API key",
        pattern: r"\bsk-(?:proj-|ant-api[0-9]{2}-)?[a-zA-Z0-9_-]{20,}\b",
        value_group: None,
    },
    TokenClass {
        id: "private-key",
        label: "private key header",
        pattern: r"-----BEGIN (?:[A-Z0-9 _-]+)?PRIVATE KEY(?: BLOCK)?-----",
        value_group: None,
    },
    TokenClass {
        id: "literal-bearer",
        label: "literal Authorization Bearer token",
        pattern: r#"(?i)\bAuthorization:\s*Bearer\s+['"]?([A-Za-z0-9_\-\.]{12,})['"]?"#,
        value_group: Some(1),
    },
];

/// A [`TokenClass`] with its expression compiled.
pub struct CompiledTokenClass {
    pub class: &'static TokenClass,
    pub re: Regex,
}

impl CompiledTokenClass {
    /// Whether `line` holds a literal credential of this class.
    pub fn matches_literal(&self, line: &str) -> bool {
        let found = self
            .re
            .captures_iter(line)
            .any(|caps| is_literal_hit(self.class, &caps));
        found
    }
}

/// Compile the whole table.
pub fn compile_token_classes() -> Result<Vec<CompiledTokenClass>> {
    TOKEN_CLASSES
        .iter()
        .map(|class| {
            Ok(CompiledTokenClass {
                class,
                re: Regex::new(class.pattern)?,
            })
        })
        .collect()
}

/// Whether a captured credential value is a variable reference or a documented
/// placeholder rather than a literal.
pub fn is_placeholder_or_var(val: &str) -> bool {
    let s = val.trim().trim_matches(|c| c == '\'' || c == '"');
    if s.is_empty() || s == "--password-stdin" {
        return true;
    }
    if s.starts_with('$') || s.starts_with("${{") || s.contains("${{ secrets.") {
        return true;
    }
    if s.starts_with("$(") || s.starts_with('`') {
        return true;
    }
    if s.starts_with('<') && s.ends_with('>') {
        return true;
    }
    if s.len() >= 4 && s.chars().all(|c| c == 'x' || c == 'X') {
        return true;
    }
    let lower = s.to_ascii_lowercase();
    let known_placeholders = [
        "changeme",
        "dummy",
        "example",
        "sample",
        "test",
        "foo",
        "bar",
        "placeholder",
        "your_api_key",
        "your-api-key",
        "your_token",
        "your-token",
        "password",
        "secret",
        "token",
    ];
    if known_placeholders
        .iter()
        .any(|&p| lower == p || lower.starts_with("todo") || lower.starts_with("fixme"))
    {
        return true;
    }
    false
}

/// Whether `caps`, a match of `class`, holds a literal credential: true unless the class
/// names a value group and that value is a variable reference or placeholder.
pub fn is_literal_hit(class: &TokenClass, caps: &Captures) -> bool {
    match class.value_group {
        None => true,
        Some(g) => caps
            .get(g)
            .is_some_and(|v| !is_placeholder_or_var(v.as_str())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(id: &str) -> CompiledTokenClass {
        compile_token_classes()
            .unwrap()
            .into_iter()
            .find(|c| c.class.id == id)
            .unwrap()
    }

    fn hit(id: &str, line: &str) -> bool {
        class(id).matches_literal(line)
    }

    // Tokens are assembled at runtime so this file never holds a live-format literal.
    fn gh(n: usize) -> String {
        format!("{}_{}", "ghp", "a1B2".repeat(n))
    }

    #[test]
    fn github_token_positive_and_negative() {
        assert!(hit("github-token", &format!("token = \"{}\"", gh(9))));
        assert!(hit(
            "github-token",
            &format!("{}_{}", "github_pat", "A1b2C3d4E5".repeat(8) + "xy")
        ));
        // Too short, a placeholder, and a prefix inside a longer word.
        assert!(!hit("github-token", "ghp_short"));
        assert!(!hit("github-token", "token = ghp_<your-token>"));
        assert!(!hit("github-token", &format!("x{}", gh(9))));
    }

    #[test]
    fn github_token_has_no_upper_length_bound() {
        // pii used to stop at 255 characters, shell-secrets did not: the union has none.
        assert!(hit("github-token", &gh(80)));
    }

    #[test]
    fn aws_access_key_covers_the_union_of_prefixes() {
        for prefix in ["AKIA", "ASIA", "ABIA", "ACCA"] {
            assert!(
                hit("aws-access-key", &format!("id={prefix}ABCDEF0123456789")),
                "{prefix}"
            );
        }
        assert!(!hit("aws-access-key", "id=AKIAABCDEF012345"));
        assert!(!hit("aws-access-key", "id=AKIA<ACCESS_KEY_ID>"));
        assert!(!hit("aws-access-key", "id=AGPAABCDEF0123456789"));
        // A sha256 hex digest in a lockfile has no such prefix.
        assert!(!hit(
            "aws-access-key",
            "checksum = \"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08\""
        ));
    }

    #[test]
    fn slack_token_positive_and_negative() {
        // Assembled at run time so this file holds no token-shaped literal.
        let slack = |kind: &str, rest: &str| format!("{}{kind}-{rest}", "xox");
        assert!(hit(
            "slack-token",
            &slack("b", "123456789012-123456789012-abcdefABCDEF")
        ));
        assert!(hit("slack-token", &slack("p", "1234567890-abcdefghij")));
        assert!(!hit("slack-token", &slack("b", "short")));
        assert!(!hit(
            "slack-token",
            &slack("z", "123456789012-123456789012")
        ));
    }

    #[test]
    fn llm_api_token_positive_and_negative() {
        let key = |mid: &str, tail: &str| format!("key = {}-{mid}{tail}", "sk");
        assert!(hit("llm-api-token", &key("", "abcdefghijklmnopqrstuvwx")));
        assert!(hit(
            "llm-api-token",
            &key("proj-", "abcdefghijklmnopqrstuvwx")
        ));
        assert!(hit(
            "llm-api-token",
            &key("ant-api03-", "abcdefghijklmnopqrstuvwx")
        ));
        // The documented placeholder, a short value, and a word that ends in `sk-`.
        assert!(!hit("llm-api-token", "key = sk-..."));
        assert!(!hit("llm-api-token", "key = sk-short"));
        assert!(!hit(
            "llm-api-token",
            "see disk-usage-report-for-the-whole-cluster"
        ));
    }

    #[test]
    fn private_key_header_variants() {
        for label in ["", "RSA ", "EC ", "OPENSSH ", "ENCRYPTED ", "DSA "] {
            let line = format!("{}BEGIN {label}PRIVATE KEY{}", "-----", "-----");
            assert!(hit("private-key", &line), "{label}");
        }
        let block = format!("{}BEGIN PGP PRIVATE KEY BLOCK{}", "-----", "-----");
        assert!(hit("private-key", &block));
        let public = format!("{}BEGIN PUBLIC KEY{}", "-----", "-----");
        assert!(!hit("private-key", &public));
        let cert = format!("{}BEGIN CERTIFICATE{}", "-----", "-----");
        assert!(!hit("private-key", &cert));
    }

    #[test]
    fn bearer_literal_rejects_variables_and_placeholders() {
        let header = |value: &str| format!("{}: Bearer {value}", "Authorization");
        assert!(hit(
            "literal-bearer",
            &format!("curl -H '{}'", header("abcdef0123456789"))
        ));
        assert!(hit(
            "literal-bearer",
            &header("eyJhbGciOiJIUzI1NiJ9.abc.def")
        ));
        assert!(!hit("literal-bearer", "Authorization: Bearer $TOKEN"));
        assert!(!hit(
            "literal-bearer",
            "Authorization: Bearer ${TOKEN_VALUE}"
        ));
        assert!(!hit(
            "literal-bearer",
            "Authorization: Bearer ${{ secrets.API_TOKEN }}"
        ));
        assert!(!hit("literal-bearer", "Authorization: Bearer <token>"));
        assert!(!hit(
            "literal-bearer",
            "Authorization: Bearer xxxxxxxxxxxxxxxx"
        ));
        assert!(!hit("literal-bearer", "Authorization: Bearer changeme"));
        // Prose that mentions the scheme without a header.
        assert!(!hit("literal-bearer", "send the Bearer scheme in a header"));
    }

    #[test]
    fn every_class_compiles_and_has_a_unique_id() {
        let all = compile_token_classes().unwrap();
        let mut ids: Vec<_> = all.iter().map(|c| c.class.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), TOKEN_CLASSES.len());
    }
}
