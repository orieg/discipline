//! OASIS SARIF v2.1.0 JSON formatter for GitHub Code Scanning, Gitea, and IDEs.

use crate::config::{gate_info, Severity};
use crate::guards::CheckSummary;
use serde_json::{json, Value};

/// Format CheckSummary as an OASIS SARIF 2.1.0 Value.
pub fn format_sarif(summary: &CheckSummary) -> Value {
    let mut rules = Vec::new();
    let mut results = Vec::new();

    // 1. One rule per finding code (`crate::findings`) of every gate that ran, so code
    //    scanning can triage, dismiss and track each kind of finding on its own.
    for o in &summary.outcomes {
        let gate_summary = gate_info(o.gate).map(|g| g.summary).unwrap_or(o.gate);
        for kind in crate::findings::FINDINGS
            .iter()
            .filter(|k| k.gates.first() == Some(&o.gate))
        {
            let id = format!("{}/{}", o.gate, kind.code);
            let short = kind.title;
            rules.push(json!({
                "id": id,
                "name": to_pascal_case(&id),
                "shortDescription": {
                    "text": short
                },
                "fullDescription": {
                    "text": gate_summary
                },
                "helpUri": format!("https://orieg.github.io/discipline/gates/#{}", o.gate),
                "properties": {
                    "gate": o.gate,
                    "tags": [o.gate],
                    "examined": o.examined
                }
            }));
        }
    }

    // 2. Build results list from violations
    for v in summary.violations() {
        let level = match v.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        };

        let mut message_text = v.message.clone();
        if let Some(rem) = &v.remediation {
            message_text.push_str(&format!("\nRemediation: {rem}"));
        }

        let mut location = json!({});
        if let Some(file) = &v.file {
            let line = v.line.unwrap_or(1);
            let uri = sanitize_sarif_uri(file);
            location = json!({
                "physicalLocation": {
                    "artifactLocation": {
                        "uri": uri
                    },
                    "region": {
                        "startLine": line
                    }
                }
            });
        }

        let mut result = json!({
            "ruleId": v.code,
            "level": level,
            "message": {
                "text": message_text
            }
        });
        if !v.fingerprint.is_empty() {
            // The baseline fingerprint: stable across line moves and title changes, so an
            // alert keeps its identity (and its dismissal) from one run to the next.
            result["partialFingerprints"] = json!({ "disciplineFingerprint/v2": v.fingerprint });
        }

        if v.file.is_some() {
            result["locations"] = json!([location]);
        }

        results.push(result);
    }

    // 3. Build invocations and overrides data
    let mut overrides = Vec::new();
    let mut total_examined = 0;
    let mut total_overrides = 0;

    for o in &summary.outcomes {
        total_examined += o.examined;
        total_overrides += o.overrides.len();
        for ov in &o.overrides {
            overrides.push(json!({
                "descriptor": {
                    "id": o.gate
                },
                "configuration": {
                    "level": "none"
                },
                "properties": {
                    "source": ov.source.to_string(),
                    "directive": ov.directive.clone(),
                    "subject": ov.subject.clone(),
                    "reason": ov.reason.clone()
                }
            }));
        }
    }

    json!({
        "$schema": "https://docs.oasis-open.org/sarif/sarif/v2.1.0/errata01/os/schemas/sarif-schema-2.1.0.json",
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
                "invocations": [
                    {
                        "executionSuccessful": summary.errors == 0,
                        "ruleConfigurationOverrides": overrides,
                        "properties": {
                            "totalExamined": total_examined,
                            "totalOverrides": total_overrides
                        }
                    }
                ],
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

fn sanitize_sarif_uri(file: &str) -> String {
    if file == "<pr-body>" {
        "pr-body".to_string()
    } else if file == "<pr-title>" {
        "pr-title".to_string()
    } else if file.starts_with('<') && file.ends_with('>') {
        file.trim_matches(|c| c == '<' || c == '>').to_string()
    } else {
        file.replace('<', "%3C").replace('>', "%3E")
    }
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
            notes: 0,
            overrides: 0,
            baselined: 0,
            outcomes: vec![GateOutcome {
                gate: "unsafe-safety-comment",
                suite: "agent-guard",
                enabled: true,
                examined: 1,
                inline_exemptions: 0,
                baselined: 0,
                notes: Vec::new(),
                violations: vec![Violation {
                    gate: "unsafe-safety-comment",
                    code: "unsafe-safety-comment/fixture".to_string(),
                    fingerprint: String::new(),
                    legacy_title: None,
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
            policy_failures: Vec::new(),
            deprecations: Vec::new(),
        };

        let val = format_sarif(&summary);
        assert_eq!(val["version"], "2.1.0");
        assert_eq!(val["runs"][0]["tool"]["driver"]["name"], "discipline");
        assert_eq!(val["runs"][0]["results"].as_array().unwrap().len(), 1);
        let res = &val["runs"][0]["results"][0];
        assert_eq!(res["ruleId"], "unsafe-safety-comment/fixture");
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

    #[test]
    fn sanitizes_synthetic_pr_body_uri() {
        let summary = CheckSummary {
            base: "origin/main".to_string(),
            errors: 1,
            warnings: 0,
            notes: 0,
            overrides: 0,
            baselined: 0,
            outcomes: vec![GateOutcome {
                gate: "time-estimates",
                suite: "hygiene",
                enabled: true,
                examined: 1,
                inline_exemptions: 0,
                baselined: 0,
                notes: Vec::new(),
                violations: vec![Violation {
                    gate: "time-estimates",
                    code: "time-estimates/fixture".to_string(),
                    fingerprint: String::new(),
                    legacy_title: None,
                    severity: Severity::Error,
                    title: "Time estimate in PR body".to_string(),
                    file: Some("<pr-body>".to_string()),
                    line: Some(1),
                    message: "Banned duration".to_string(),
                    remediation: Some("Remove duration".to_string()),
                }],
                overrides: Vec::new(),
            }],
            planned_gates: Vec::new(),
            policy_failures: Vec::new(),
            deprecations: Vec::new(),
        };

        let val = format_sarif(&summary);
        let uri = &val["runs"][0]["results"][0]["locations"][0]["physicalLocation"]
            ["artifactLocation"]["uri"];
        assert_eq!(uri, "pr-body");
        assert!(!uri.as_str().unwrap().contains('<'));
        assert!(!uri.as_str().unwrap().contains('>'));
    }
}
