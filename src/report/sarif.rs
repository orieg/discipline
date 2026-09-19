//! OASIS SARIF v2.1.0 JSON formatter for GitHub Code Scanning, Gitea, and IDEs.

use crate::config::{gate_info, Severity};
use crate::guards::CheckSummary;
use serde_json::{json, Value};

/// Format CheckSummary as an OASIS SARIF 2.1.0 Value.
pub fn format_sarif(summary: &CheckSummary) -> Value {
    let mut rules = Vec::new();
    let mut results = Vec::new();

    // 1. Build rules list from gates
    for o in &summary.outcomes {
        let desc = gate_info(o.gate).map(|g| g.summary).unwrap_or(o.gate);
        rules.push(json!({
            "id": o.gate,
            "name": to_pascal_case(o.gate),
            "shortDescription": {
                "text": desc
            },
            "helpUri": "https://github.com/orieg/discipline"
        }));
    }

    // 2. Build results list from violations
    for v in summary.violations() {
        let level = match v.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };

        let mut message_text = v.message.clone();
        if let Some(rem) = &v.remediation {
            message_text.push_str(&format!("\nRemediation: {rem}"));
        }

        let mut location = json!({});
        if let Some(file) = &v.file {
            let line = v.line.unwrap_or(1);
            location = json!({
                "physicalLocation": {
                    "artifactLocation": {
                        "uri": file
                    },
                    "region": {
                        "startLine": line
                    }
                }
            });
        }

        let mut result = json!({
            "ruleId": v.gate,
            "level": level,
            "message": {
                "text": message_text
            }
        });

        if v.file.is_some() {
            result["locations"] = json!([location]);
        }

        results.push(result);
    }

    json!({
        "$schema": "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/master/Schemata/sarif-schema-2.1.0.json",
        "version": "2.1.0",
        "runs": [
            {
                "tool": {
                    "driver": {
                        "name": "discipline",
                        "informationUri": "https://github.com/orieg/discipline",
                        "rules": rules
                    }
                },
                "results": results
            }
        ]
    })
}

fn to_pascal_case(s: &str) -> String {
    let mut out = String::new();
    for part in s.split('-') {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.push_str(&first.to_uppercase().to_string());
            out.push_str(chars.as_str());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guards::{GateOutcome, Violation};

    #[test]
    fn formats_sarif_v2_schema_compliant() {
        let summary = CheckSummary {
            base: "origin/main".to_string(),
            errors: 1,
            warnings: 0,
            overrides: 0,
            outcomes: vec![GateOutcome {
                gate: "unsafe-safety-comment",
                suite: "agent-guard",
                enabled: true,
                examined: 1,
                inline_exemptions: 0,
                notes: Vec::new(),
                violations: vec![Violation {
                    gate: "unsafe-safety-comment",
                    severity: Severity::Error,
                    title: "Undocumented unsafe".to_string(),
                    file: Some("src/lib.rs".to_string()),
                    line: Some(42),
                    message: "unsafe block missing SAFETY comment".to_string(),
                    remediation: Some("Add // SAFETY: ...".to_string()),
                }],
                overrides: Vec::new(),
            }],
            planned_gates: Vec::new(),
        };

        let val = format_sarif(&summary);
        assert_eq!(val["version"], "2.1.0");
        assert_eq!(val["runs"][0]["tool"]["driver"]["name"], "discipline");
        assert_eq!(val["runs"][0]["results"].as_array().unwrap().len(), 1);
        let res = &val["runs"][0]["results"][0];
        assert_eq!(res["ruleId"], "unsafe-safety-comment");
        assert_eq!(res["level"], "error");
        assert_eq!(
            res["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
            "src/lib.rs"
        );
        assert_eq!(
            res["locations"][0]["physicalLocation"]["region"]["startLine"],
            42
        );
    }
}
