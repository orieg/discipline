//! JUnit XML formatter for test result collectors (Argo, Jenkins, GitLab, Azure DevOps).

use crate::config::Severity;
use crate::guards::CheckSummary;

/// Format CheckSummary as standard JUnit XML.
pub fn format_junit(summary: &CheckSummary, fail_on_warnings: bool) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");

    let total_tests = summary.outcomes.len();
    let mut total_failures = 0;

    for o in &summary.outcomes {
        if !o.enabled {
            // Skipped counts belong to individual testsuite elements
        } else if !o.violations.is_empty() {
            let has_failure = o.violations.iter().any(|v| match v.severity {
                Severity::Error => true,
                Severity::Warning => fail_on_warnings,
            });
            if has_failure {
                total_failures += 1;
            }
        }
    }

    out.push_str(&format!(
        "<testsuites name=\"discipline\" tests=\"{total_tests}\" failures=\"{total_failures}\" errors=\"0\" time=\"0.0\">\n"
    ));

    // Group by suite
    let mut suites: std::collections::BTreeMap<&str, Vec<&crate::guards::GateOutcome>> =
        std::collections::BTreeMap::new();
    for o in &summary.outcomes {
        suites.entry(o.suite).or_default().push(o);
    }

    for (suite_name, outcomes) in suites {
        let suite_tests = outcomes.len();
        let mut suite_failures = 0;
        let mut suite_skipped = 0;
        let suite_examined: usize = outcomes.iter().map(|o| o.examined).sum();
        let suite_overrides: usize = outcomes.iter().map(|o| o.overrides.len()).sum();

        for o in &outcomes {
            if !o.enabled {
                suite_skipped += 1;
            } else if !o.violations.is_empty() {
                let has_failure = o.violations.iter().any(|v| match v.severity {
                    Severity::Error => true,
                    Severity::Warning => fail_on_warnings,
                });
                if has_failure {
                    suite_failures += 1;
                }
            }
        }

        out.push_str(&format!(
            "  <testsuite name=\"{suite_name}\" tests=\"{suite_tests}\" failures=\"{suite_failures}\" errors=\"0\" skipped=\"{suite_skipped}\" time=\"0.0\">\n"
        ));
        out.push_str("    <properties>\n");
        out.push_str(&format!(
            "      <property name=\"examined\" value=\"{suite_examined}\"/>\n"
        ));
        out.push_str(&format!(
            "      <property name=\"overrides\" value=\"{suite_overrides}\"/>\n"
        ));
        for o in &outcomes {
            out.push_str(&format!(
                "      <property name=\"{}:examined\" value=\"{}\"/>\n",
                o.gate, o.examined
            ));
            for (idx, ov) in o.overrides.iter().enumerate() {
                out.push_str(&format!(
                    "      <property name=\"{}:override:{}\" value=\"{}\"/>\n",
                    o.gate,
                    idx + 1,
                    escape_xml(&format!("[{}] {}", ov.source, ov.reason))
                ));
            }
        }
        out.push_str("    </properties>\n");

        for o in outcomes {
            let classname = format!("discipline.{suite_name}");
            let gate_name = o.gate;

            if !o.enabled {
                out.push_str(&format!(
                    "    <testcase name=\"{gate_name}\" classname=\"{classname}\" time=\"0.0\">\n"
                ));
                out.push_str("      <skipped message=\"gate disabled in configuration\"/>\n");
                out.push_str("    </testcase>\n");
            } else if o.violations.is_empty() {
                if o.examined > 0 || !o.overrides.is_empty() {
                    out.push_str(&format!(
                        "    <testcase name=\"{gate_name}\" classname=\"{classname}\" time=\"0.0\">\n"
                    ));
                    out.push_str(&format!(
                        "      <system-out>examined: {}; overrides: {}</system-out>\n",
                        o.examined,
                        o.overrides.len()
                    ));
                    out.push_str("    </testcase>\n");
                } else {
                    out.push_str(&format!(
                        "    <testcase name=\"{gate_name}\" classname=\"{classname}\" time=\"0.0\"/>\n"
                    ));
                }
            } else {
                out.push_str(&format!(
                    "    <testcase name=\"{gate_name}\" classname=\"{classname}\" time=\"0.0\">\n"
                ));
                out.push_str(&format!(
                    "      <system-out>examined: {}; overrides: {}</system-out>\n",
                    o.examined,
                    o.overrides.len()
                ));
                for v in &o.violations {
                    let sev_type = match v.severity {
                        Severity::Error => "error",
                        Severity::Warning => "warning",
                    };
                    let title = escape_xml(&v.title);
                    let mut details = String::new();
                    if let Some(file) = &v.file {
                        if let Some(line) = v.line {
                            details.push_str(&format!("[{file}:{line}] "));
                        } else {
                            details.push_str(&format!("[{file}] "));
                        }
                    }
                    details.push_str(&v.message);
                    if let Some(rem) = &v.remediation {
                        details.push_str(&format!("\nRemediation: {rem}"));
                    }

                    out.push_str(&format!(
                        "      <failure message=\"{title}\" type=\"{sev_type}\">{}</failure>\n",
                        escape_xml(&details)
                    ));
                }
                out.push_str("    </testcase>\n");
            }
        }

        out.push_str("  </testsuite>\n");
    }

    out.push_str("</testsuites>\n");
    out
}

fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guards::{GateOutcome, Violation};

    #[test]
    fn formats_clean_summary_as_valid_junit_xml() {
        let summary = CheckSummary {
            base: "origin/main".to_string(),
            errors: 0,
            warnings: 0,
            overrides: 0,
            outcomes: vec![
                GateOutcome {
                    gate: "agents-md",
                    suite: "agent-guard",
                    enabled: true,
                    examined: 1,
                    inline_exemptions: 0,
                    notes: Vec::new(),
                    violations: Vec::new(),
                    overrides: Vec::new(),
                },
                GateOutcome {
                    gate: "miri",
                    suite: "verification",
                    enabled: false,
                    examined: 0,
                    inline_exemptions: 0,
                    notes: Vec::new(),
                    violations: Vec::new(),
                    overrides: Vec::new(),
                },
            ],
            planned_gates: Vec::new(),
        };

        let xml = format_junit(&summary, false);
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(xml.contains("<testsuites name=\"discipline\" tests=\"2\" failures=\"0\""));
        assert!(xml.contains(
            "<testcase name=\"agents-md\" classname=\"discipline.agent-guard\" time=\"0.0\">"
        ));
        assert!(xml.contains("<system-out>examined: 1; overrides: 0</system-out>"));
        assert!(xml.contains("<skipped message=\"gate disabled in configuration\"/>"));
    }

    #[test]
    fn formats_failures_with_escaped_xml() {
        let summary = CheckSummary {
            base: "origin/main".to_string(),
            errors: 1,
            warnings: 0,
            overrides: 0,
            outcomes: vec![GateOutcome {
                gate: "assertion-reduction",
                suite: "agent-guard",
                enabled: true,
                examined: 1,
                inline_exemptions: 0,
                notes: Vec::new(),
                violations: vec![Violation {
                    gate: "assertion-reduction",
                    severity: Severity::Error,
                    title: "Effective Asserts < Expected & \"Dangerous\"".to_string(),
                    file: Some("tests/a.rs".to_string()),
                    line: Some(10),
                    message: "assertions dropped from 2 < 1".to_string(),
                    remediation: Some("use allow-assertion-drop: foo <reason>".to_string()),
                }],
                overrides: Vec::new(),
            }],
            planned_gates: Vec::new(),
        };

        let xml = format_junit(&summary, false);
        assert!(xml.contains("failures=\"1\""));
        assert!(xml.contains("&lt; Expected &amp; &quot;Dangerous&quot;"));
        assert!(xml.contains("[tests/a.rs:10]"));
    }
}
