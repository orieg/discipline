//! GitLab Code Quality (Code Climate compliant) JSON reporter.
//!
//! Generates `gl-codequality.json` consumed by GitLab Merge Request Code Quality widgets.
//! Specifications: https://docs.gitlab.com/ee/ci/testing/code_quality.html

use crate::config::Severity;
use crate::guards::CheckSummary;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitlabCodeQualityIssue {
    pub description: String,
    pub check_name: String,
    pub fingerprint: String,
    pub severity: String,
    pub location: GitlabLocation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitlabLocation {
    pub path: String,
    pub lines: GitlabLines,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitlabLines {
    pub begin: usize,
}

/// Serialize all violations in `CheckSummary` into a Code Climate JSON array.
pub fn format_gitlab(summary: &CheckSummary) -> String {
    let issues = generate_gitlab_issues(summary);
    serde_json::to_string_pretty(&issues).unwrap_or_else(|_| "[]".to_string())
}

/// Convert `CheckSummary` violations into structured GitLab Code Quality issues.
pub fn generate_gitlab_issues(summary: &CheckSummary) -> Vec<GitlabCodeQualityIssue> {
    let mut issues = Vec::new();

    for outcome in &summary.outcomes {
        let suite_title = match outcome.suite {
            "agent-guard" => "Agent Guard",
            "hygiene" => "Hygiene",
            "integrity" => "Integrity",
            "bench" => "Bench",
            "quality" => "Quality",
            "verification" => "Verification",
            other => other,
        };

        for v in &outcome.violations {
            let path = v
                .file
                .clone()
                .unwrap_or_else(|| "discipline.toml".to_string());
            let line = v.line.unwrap_or(1);

            let description = if v.message.starts_with(suite_title) {
                v.message.clone()
            } else {
                format!("{suite_title}: {}", v.message)
            };

            let severity = match v.severity {
                Severity::Error => "major",
                Severity::Warning => "minor",
                Severity::Note => "info",
            };

            // The baseline fingerprint (stable across line moves and title changes), or,
            // for a finding built outside a check run, one over code, path, line and message.
            let fingerprint = if v.fingerprint.is_empty() {
                let fp_source = format!("{}:{path}:{line}:{}", v.code, v.message);
                sha256_hex(fp_source.as_bytes())
            } else {
                v.fingerprint.clone()
            };

            issues.push(GitlabCodeQualityIssue {
                description,
                check_name: v.code.clone(),
                fingerprint,
                severity: severity.to_string(),
                location: GitlabLocation {
                    path,
                    lines: GitlabLines { begin: line },
                },
            });
        }
    }

    issues
}

/// Compute the lowercase hex-encoded SHA-256 digest of `data`.
pub fn sha256_hex(data: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, data);
    let mut hex = String::with_capacity(64);
    for b in digest.as_ref() {
        use std::fmt::Write;
        let _ = write!(hex, "{:02x}", b);
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guards::{GateOutcome, Violation};

    #[test]
    fn test_sha256_nist_vectors() {
        // Empty string
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        // "abc"
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn test_format_gitlab_empty() {
        let summary = CheckSummary {
            schema_version: crate::output_schema::REPORT_SCHEMA_VERSION,
            could_not_check: None,
            base: "main".to_string(),
            errors: 0,
            warnings: 0,
            notes: 0,
            overrides: 0,
            baselined: 0,
            planned_gates: Vec::new(),
            outcomes: Vec::new(),
            policy_failures: Vec::new(),
            deprecations: Vec::new(),
            directive_notes: Vec::new(),
            unused_directives: Vec::new(),
        };
        let json = format_gitlab(&summary);
        assert_eq!(json.trim(), "[]");
    }

    #[test]
    fn test_format_gitlab_with_violations() {
        let mut outcome = GateOutcome::new("assertion-reduction");
        outcome.violations.push(Violation {
            gate: "assertion-reduction",
            code: "assertion-reduction/fixture".to_string(),
            fingerprint: String::new(),
            anchor: None,
            legacy_title: None,
            severity: Severity::Error,
            title: "Strong Assertions Decreased".to_string(),
            file: Some("tests/trie_traversal.rs".to_string()),
            line: Some(48),
            message: "2 assertions removed from test_leaf_split without replacement".to_string(),
            remediation: Some("Restore the assertions or provide allow-assertion-drop".to_string()),
        });

        let summary = CheckSummary {
            schema_version: crate::output_schema::REPORT_SCHEMA_VERSION,
            could_not_check: None,
            base: "origin/main".to_string(),
            errors: 1,
            warnings: 0,
            notes: 0,
            overrides: 0,
            baselined: 0,
            planned_gates: Vec::new(),
            outcomes: vec![outcome],
            policy_failures: Vec::new(),
            deprecations: Vec::new(),
            directive_notes: Vec::new(),
            unused_directives: Vec::new(),
        };

        let json = format_gitlab(&summary);
        let parsed: Vec<GitlabCodeQualityIssue> =
            serde_json::from_str(&json).expect("valid code quality json");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].check_name, "assertion-reduction/fixture");
        assert_eq!(
            parsed[0].description,
            "Agent Guard: 2 assertions removed from test_leaf_split without replacement"
        );
        assert_eq!(parsed[0].severity, "major");
        assert_eq!(parsed[0].location.path, "tests/trie_traversal.rs");
        assert_eq!(parsed[0].location.lines.begin, 48);
        assert_eq!(parsed[0].fingerprint.len(), 64);
    }

    #[test]
    fn gitlab_escaping_handles_special_characters_in_messages() {
        // A message with quotes, newlines, control and markup characters reaches the
        // report unchanged: this module carries it into `description` and the
        // fingerprint and leaves the escaping to the JSON writer.
        const SUITE: &str = "Agent Guard";
        const MESSAGE: &str = "Found /Users/test\ntab\tand \"quotes\" & <angle>"; // discipline:allow(pii)
        let mut outcome = GateOutcome::new("agents-md");
        outcome.violations.push(Violation {
            gate: "agents-md",
            code: "agents-md/fixture".to_string(),
            fingerprint: String::new(),
            anchor: None,
            legacy_title: None,
            severity: Severity::Warning,
            title: "Agents markdown found".to_string(),
            file: Some("src/main.rs".to_string()),
            line: Some(42),
            message: MESSAGE.to_string(),
            remediation: None,
        });

        let summary = CheckSummary {
            schema_version: crate::output_schema::REPORT_SCHEMA_VERSION,
            could_not_check: None,
            base: "origin/main".to_string(),
            errors: 0,
            warnings: 1,
            notes: 0,
            overrides: 0,
            baselined: 0,
            planned_gates: Vec::new(),
            outcomes: vec![outcome],
            policy_failures: Vec::new(),
            deprecations: Vec::new(),
            directive_notes: Vec::new(),
            unused_directives: Vec::new(),
        };

        let json = format_gitlab(&summary);
        // Must parse as valid JSON (serde_json escapes all special chars).
        let parsed: Vec<GitlabCodeQualityIssue> =
            serde_json::from_str(&json).expect("valid code quality json with escaped chars");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].description, format!("{SUITE}: {MESSAGE}"));
        assert_eq!(
            parsed[0].fingerprint,
            sha256_hex(format!("agents-md/fixture:src/main.rs:42:{MESSAGE}").as_bytes())
        );

        // A backslash and a NUL, on a finding with no file and no line.
        const MESSAGE2: &str = "Message with backslash \\ and null byte escape \0";
        let mut outcome2 = GateOutcome::new("agents-md");
        outcome2.violations.push(Violation {
            gate: "agents-md",
            code: "agents-md/fixture".to_string(),
            fingerprint: String::new(),
            anchor: None,
            legacy_title: None,
            severity: Severity::Error,
            title: "Time estimate found".to_string(),
            file: None,
            line: None,
            message: MESSAGE2.to_string(),
            remediation: None,
        });

        let summary2 = CheckSummary {
            schema_version: crate::output_schema::REPORT_SCHEMA_VERSION,
            could_not_check: None,
            base: "origin/main".to_string(),
            errors: 1,
            warnings: 0,
            notes: 0,
            overrides: 0,
            baselined: 0,
            planned_gates: Vec::new(),
            outcomes: vec![outcome2],
            policy_failures: Vec::new(),
            deprecations: Vec::new(),
            directive_notes: Vec::new(),
            unused_directives: Vec::new(),
        };

        let json2 = format_gitlab(&summary2);
        let parsed2 = serde_json::from_str::<Vec<GitlabCodeQualityIssue>>(&json2)
            .expect("valid code quality json with backslash");
        assert_eq!(parsed2.len(), 1);
        assert_eq!(parsed2[0].description, format!("{SUITE}: {MESSAGE2}"));
        assert_eq!(
            parsed2[0].fingerprint,
            sha256_hex(format!("agents-md/fixture:discipline.toml:1:{MESSAGE2}").as_bytes())
        );
        // A message that differs only in a special character is a different finding.
        assert_ne!(parsed[0].fingerprint, parsed2[0].fingerprint);
    }
}
