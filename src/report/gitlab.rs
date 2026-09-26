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

// -----------------------------------------------------------------------------
// Pure-Rust SHA-256 Implementation (FIPS 180-4, zero external dependencies)
// -----------------------------------------------------------------------------

/// Compute the lowercase hex-encoded SHA-256 digest of `data`.
pub fn sha256_hex(data: &[u8]) -> String {
    let digest = sha256_digest(data);
    let mut hex = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(hex, "{:02x}", b);
    }
    hex
}

fn sha256_digest(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = Vec::with_capacity(data.len() + 64);
    msg.extend_from_slice(data);
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0x00);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    #[allow(clippy::chunks_exact_to_as_chunks)]
    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut h_val = h[7];

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h_val
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h_val = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(h_val);
    }

    let mut out = [0u8; 32];
    for (i, val) in h.iter().enumerate() {
        out[i * 4..(i + 1) * 4].copy_from_slice(&val.to_be_bytes());
    }
    out
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
            legacy_title: None,
            severity: Severity::Error,
            title: "Strong Assertions Decreased".to_string(),
            file: Some("tests/trie_traversal.rs".to_string()),
            line: Some(48),
            message: "2 assertions removed from test_leaf_split without replacement".to_string(),
            remediation: Some("Restore the assertions or provide allow-assertion-drop".to_string()),
        });

        let summary = CheckSummary {
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
}
