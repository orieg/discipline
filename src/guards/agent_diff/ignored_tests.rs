//! `ignored-tests`: a test the change disables or skips.

use super::{leaf_name, Located, TestPair};
use crate::config::GateSettings;
use crate::guards::{exempt_filter, GateOutcome};
use crate::tokens;
use anyhow::Result;

pub fn evaluate_ignored_tests(
    pairs: &[TestPair],
    added: &[Located],
    settings: &crate::config::IgnoredTestsGate,
    directives: &[crate::tokens::ParsedDirective],
    is_staged: bool,
) -> Result<GateOutcome> {
    const GATE: &str = "ignored-tests";
    let exempt = exempt_filter(settings)?;
    let mut out = GateOutcome::new(GATE);
    out.examined = pairs.len() + added.len();

    let newly_ignored_existing = pairs
        .iter()
        .filter(|p| p.head.ignored && !p.base.ignored)
        .map(|p| (p.path, p.head, false));
    let newly_ignored_added = added
        .iter()
        .filter(|a| a.test.ignored)
        .map(|a| (a.path, a.test, true));

    for (path, test, arrives_ignored) in newly_ignored_existing.chain(newly_ignored_added) {
        if exempt.matches(path) {
            continue;
        }
        let lifts = if arrives_ignored {
            &crate::findings::IGNORED_TEST_ADDED
        } else {
            &crate::findings::EXISTING_TEST_SKIPPED
        };
        let subject = leaf_name(test);
        if let Some(d) = directives.iter().find(|d| {
            tokens::ALLOW_IGNORE
                .iter()
                .any(|n| n.eq_ignore_ascii_case(&d.directive))
                && d.names_subject(subject)
        }) {
            let cleaned = d.reason.trim().trim_matches(['"', '\'', '`']);
            let explanation = cleaned
                .strip_prefix(subject)
                .map(|s| s.trim_start_matches(|c: char| c == ':' || c == '-' || c.is_whitespace()))
                .unwrap_or(cleaned)
                .trim();
            if explanation.is_empty()
                || explanation.eq_ignore_ascii_case("todo")
                || explanation.eq_ignore_ascii_case("tbd")
                || explanation.eq_ignore_ascii_case("fix later")
                || explanation.eq_ignore_ascii_case("temporary")
                || explanation.eq_ignore_ascii_case("wip")
                || !tokens::is_valid_rationale(explanation)
            {
                out.push(
                    settings.severity(),
                    &crate::findings::SKIP_JUSTIFICATION_INSUFFICIENT,
                    Some(path),
                    Some(test.line),
                    format!(
                        "Directive for skipped test `{}` lacks a substantive rationale or issue tracker reference (got `{}`).",
                        test.name, d.reason
                    ),
                    "Provide a substantive explanation or linked issue reference (e.g. `allow-ignore: <test> #123 fix broken upstream API`).",
                );
                continue;
            }
            if let Some(record) =
                tokens::find_override(directives, GATE, lifts, tokens::ALLOW_IGNORE, subject)
            {
                out.overrides.push(record);
                continue;
            }
        }
        let severity = if is_staged {
            crate::config::Severity::Warning
        } else {
            settings.severity()
        };
        let (title, message) = if arrives_ignored {
            (
                &crate::findings::IGNORED_TEST_ADDED,
                format!("Test `{}` arrives ignored.", test.name),
            )
        } else {
            (
                &crate::findings::EXISTING_TEST_SKIPPED,
                format!("Test `{}` no longer runs.", test.name),
            )
        };
        out.push(
            severity,
            title,
            Some(path),
            Some(test.line),
            message,
            &format!(
                "Fix the test, or justify it on its own line in the PR body or a commit \
                 message: `allow-ignore: {} <reason>`.",
                leaf_name(test)
            ),
        );
    }

    // A test made green by running it again. A retry marker does not skip the test, but
    // it lets a failure through as often as the marker allows.
    // A delay added to a test: the shape of a race fixed by waiting for it.
    let newly_slept = pairs
        .iter()
        .filter(|p| p.head.sleeps > p.base.sleeps)
        .map(|p| (p.path, p.head, p.base.sleeps))
        .chain(
            added
                .iter()
                .filter(|a| a.test.sleeps > 0)
                .map(|a| (a.path, a.test, 0)),
        );
    for (path, test, before) in newly_slept {
        if exempt.matches(path) {
            continue;
        }
        out.lift_or_push(
            tokens::find_override(
                directives,
                GATE,
                &crate::findings::TEST_SLEEP_ADDED,
                tokens::ALLOW_IGNORE,
                leaf_name(test),
            ),
            crate::config::Severity::Warning,
            &crate::findings::TEST_SLEEP_ADDED,
            (Some(path), Some(test.line)),
            format!(
                "Test `{}` carries {} hard-coded delay(s) (was {before}); a timing-dependent pass slows the suite and hides the race.",
                test.name, test.sleeps
            ),
            &format!(
                "Synchronise on the event the test waits for, or justify the delay: `allow-ignore: {} <reason>`.",
                leaf_name(test)
            ),
        );
    }

    let newly_retried = pairs
        .iter()
        .filter(|p| p.head.retries.is_some() && p.base.retries.is_none())
        .map(|p| (p.path, p.head))
        .chain(
            added
                .iter()
                .filter(|a| a.test.retries.is_some())
                .map(|a| (a.path, a.test)),
        );
    for (path, test) in newly_retried {
        if exempt.matches(path) {
            continue;
        }
        if let Some(record) = tokens::find_override(
            directives,
            GATE,
            &crate::findings::TEST_RETRY_ADDED,
            tokens::ALLOW_IGNORE,
            leaf_name(test),
        ) {
            out.overrides.push(record);
            continue;
        }
        let marker = test.retries.as_deref().unwrap_or("");
        out.push(
            if is_staged {
                crate::config::Severity::Warning
            } else {
                settings.severity()
            },
            &crate::findings::TEST_RETRY_ADDED,
            Some(path),
            Some(test.line),
            format!(
                "Test `{}` carries a retry marker (`{marker}`); a failure passes on a later attempt.",
                test.name
            ),
            &format!(
                "Fix the cause of the flakiness, or justify the retry on its own line in the PR body or a commit message: `allow-ignore: {} <reason>`.",
                leaf_name(test)
            ),
        );
    }

    // A conditional skip is new when the base side had none, and also when the base side
    // had one that no CI variable decided and the head side's is CI-conditional: the test
    // stops running in CI. The two sides are compared by that classification, never by
    // their text, so a reworded or reordered condition is not a change. A condition that
    // was already CI-conditional on the base side is reported again only when the head
    // side reads a CI variable the base side did not and `approved_predicates` does not
    // list it: the test stops running on one more CI system. A condition that narrows
    // (`CI` to `CI && short`) reads no new CI variable. A skip that holds only outside CI
    // (`if os.Getenv("CI") == ""`) is not CI-conditional: the test still runs there.
    let approves = |var: &str| {
        settings.approved_predicates.iter().any(|p| {
            p.eq_ignore_ascii_case(var)
                || crate::ast::cond_contains_ident(var, p)
                || crate::ast::cond_contains_ident(p, var)
        })
    };
    let added_ci_vars = |p: &TestPair| -> Vec<String> {
        if !(p.base.is_ci_skip() && p.head.is_ci_skip()) {
            return Vec::new();
        }
        let before = p.base.ci_skip_vars();
        p.head
            .ci_skip_vars()
            .into_iter()
            .filter(|var| !before.contains(var) && !approves(var))
            .collect()
    };
    let newly_cond_ignored = pairs
        .iter()
        .filter(|p| p.head.conditional_ignore.is_some() && !p.head.ignored)
        .filter_map(|p| {
            let added_vars = added_ci_vars(p);
            (p.base.conditional_ignore.is_none()
                || (p.head.is_ci_skip() && !p.base.is_ci_skip())
                || !added_vars.is_empty())
            .then_some((p.path, p.head, added_vars))
        })
        .chain(
            added
                .iter()
                .filter(|a| a.test.conditional_ignore.is_some() && !a.test.ignored)
                .map(|a| (a.path, a.test, Vec::new())),
        );
    for (path, test, added_vars) in newly_cond_ignored {
        if exempt.matches(path) {
            continue;
        }
        let cond = test.conditional_ignore.as_deref().unwrap_or("condition");
        let ci_vars = test.ci_skip_vars();
        let is_ci = test.is_ci_skip();

        let is_approved = if !added_vars.is_empty() {
            // Each added variable is one `approved_predicates` does not list.
            false
        } else if is_ci {
            !settings.approved_predicates.is_empty() && ci_vars.iter().all(|var| approves(var))
        } else {
            settings
                .approved_predicates
                .iter()
                .any(|p| crate::ast::cond_contains_ident(cond, p))
        };
        if is_approved {
            continue;
        }
        if let Some(record) = tokens::find_override(
            directives,
            GATE,
            &crate::findings::TEST_CONDITIONALLY_SKIPPED,
            tokens::ALLOW_IGNORE,
            leaf_name(test),
        ) {
            out.overrides.push(record);
            continue;
        }
        let severity = if is_ci {
            if is_staged {
                crate::config::Severity::Warning
            } else {
                settings.ci_skip_severity()
            }
        } else {
            crate::config::Severity::Note
        };
        let remediation = if is_ci {
            format!(
                "Fix the test, approve the predicate under `approved_predicates`, or justify the skip on its own line: `allow-ignore: {} <reason>`.",
                leaf_name(test)
            )
        } else {
            "Conditional skips are monitored. If this was unintended, remove the conditional ignore attribute.".to_string()
        };
        out.push(
            severity,
            &crate::findings::TEST_CONDITIONALLY_SKIPPED,
            Some(path),
            Some(test.line),
            if !added_vars.is_empty() {
                format!(
                    "Test `{}` is conditionally skipped under predicate `{}`, which adds CI variable `{}` to a skip that was already CI-conditional.",
                    test.name,
                    cond,
                    added_vars.join("`, `")
                )
            } else if is_ci && !ci_vars.iter().any(|v| crate::ast::cond_contains_ident(cond, v)) {
                // The condition reaches the variable through a name of its own.
                format!(
                    "Test `{}` is conditionally skipped under predicate `{}`, which reads CI variable `{}`.",
                    test.name,
                    cond,
                    ci_vars.join("`, `")
                )
            } else {
                format!(
                    "Test `{}` is conditionally skipped under predicate `{}`.",
                    test.name, cond
                )
            },
            &remediation,
        );
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::TestFn;

    #[test]
    fn test_ignored_tests_pure() {
        let settings = crate::config::IgnoredTestsGate::default();
        let b = TestFn {
            name: "active".to_string(),
            line: 1,
            total_asserts: 1,
            strong_asserts: 1,
            tautologies: 0,
            ignored: false,
            should_panic: None,
            ..Default::default()
        };
        let h_ignored = TestFn {
            name: "active".to_string(),
            line: 1,
            total_asserts: 1,
            strong_asserts: 1,
            tautologies: 0,
            ignored: true,
            should_panic: None,
            ..Default::default()
        };

        let pairs = [TestPair {
            path: "tests/a.rs",
            base: &b,
            head: &h_ignored,
            forced: false,
        }];
        let out = evaluate_ignored_tests(&pairs, &[], &settings, &[], false).unwrap();
        assert_eq!(out.violations.len(), 1);
        assert_eq!(out.violations[0].title, "Existing Test Skipped");
        assert!(out.violations[0].message.contains("no longer runs"));

        // Test arriving ignored
        let added_ignored = [Located {
            path: "tests/new.rs",
            file_survives: true,
            test: &h_ignored,
        }];
        let out_added = evaluate_ignored_tests(&[], &added_ignored, &settings, &[], false).unwrap();
        assert_eq!(out_added.violations.len(), 1);
        assert_eq!(out_added.violations[0].title, "Ignored Test Added");
        assert!(out_added.violations[0].message.contains("arrives ignored"));

        // Test conditional ignore with and without approved_predicates
        let h_miri = TestFn {
            name: "miri_test".to_string(),
            line: 10,
            total_asserts: 1,
            strong_asserts: 1,
            tautologies: 0,
            ignored: false,
            conditional_ignore: Some("miri".to_string()),
            should_panic: None,
            ..Default::default()
        };
        let added_miri = [Located {
            path: "tests/miri.rs",
            file_survives: true,
            test: &h_miri,
        }];
        // Without approved predicate -> warning violation
        let out_unapproved =
            evaluate_ignored_tests(&[], &added_miri, &settings, &[], false).unwrap();
        assert_eq!(out_unapproved.violations.len(), 1);
        assert_eq!(
            out_unapproved.violations[0].title,
            "Test Conditionally Skipped"
        );

        // With approved predicate -> 0 violations
        let mut approved_settings = settings.clone();
        approved_settings.approved_predicates = vec!["miri".to_string()];
        let out_approved =
            evaluate_ignored_tests(&[], &added_miri, &approved_settings, &[], false).unwrap();
        assert_eq!(out_approved.violations.len(), 0);

        let directives = [crate::tokens::ParsedDirective {
            directive: "allow-ignore".to_string(),
            reason: "active flaky upstream".to_string(),
            source: crate::tokens::OverrideSource::PrBody,
            hidden: false,
        }];
        let out_override =
            evaluate_ignored_tests(&pairs, &[], &settings, &directives, false).unwrap();
        assert_eq!(out_override.violations.len(), 0);
        assert_eq!(out_override.overrides.len(), 1);
    }

    #[test]
    fn ignored_tests_ci_skip_is_error_and_generic_skip_is_note() {
        let ci_test = TestFn {
            name: "test_ci".to_string(),
            line: 5,
            conditional_ignore: Some("os.Getenv(\"CI\") != \"\"".to_string()),
            ..Default::default()
        };
        let generic_test = TestFn {
            name: "test_generic".to_string(),
            line: 15,
            conditional_ignore: Some("os.Getenv(\"CUSTOM\") != \"\"".to_string()),
            ..Default::default()
        };
        let added = [
            Located {
                path: "tests/suite.go",
                file_survives: true,
                test: &ci_test,
            },
            Located {
                path: "tests/suite.go",
                file_survives: true,
                test: &generic_test,
            },
        ];

        let default_settings = crate::config::IgnoredTestsGate::default();

        // 1. Positive control: CI skip is Error by default
        let out = evaluate_ignored_tests(&[], &added, &default_settings, &[], false).unwrap();
        assert_eq!(out.violations.len(), 2);
        let ci_v = out
            .violations
            .iter()
            .find(|v| v.message.contains("test_ci"))
            .unwrap();
        assert_eq!(ci_v.severity, crate::config::Severity::Error);
        assert!(ci_v
            .remediation
            .as_deref()
            .unwrap()
            .contains("approved_predicates"));
        assert!(ci_v
            .remediation
            .as_deref()
            .unwrap()
            .contains("allow-ignore: test_ci <reason>"));

        // 2. Negative control: Generic skip is Note
        let generic_v = out
            .violations
            .iter()
            .find(|v| v.message.contains("test_generic"))
            .unwrap();
        assert_eq!(generic_v.severity, crate::config::Severity::Note);

        // 3. Staged mode softens CI skip to Warning
        let staged_out = evaluate_ignored_tests(&[], &added, &default_settings, &[], true).unwrap();
        let staged_ci_v = staged_out
            .violations
            .iter()
            .find(|v| v.message.contains("test_ci"))
            .unwrap();
        assert_eq!(staged_ci_v.severity, crate::config::Severity::Warning);

        // 4. Negative control: approved_predicates waives CI condition
        let mut approved_settings = default_settings.clone();
        approved_settings.approved_predicates = vec!["CI".to_string()];
        let approved_out =
            evaluate_ignored_tests(&[], &added, &approved_settings, &[], false).unwrap();
        assert_eq!(approved_out.violations.len(), 1);
        assert!(approved_out.violations[0].message.contains("test_generic"));

        // 5. Negative control: allow-ignore directive lifts CI condition
        let directive = [crate::tokens::ParsedDirective {
            directive: "allow-ignore".to_string(),
            reason: "test_ci flaky in CI runner".to_string(),
            source: crate::tokens::OverrideSource::PrBody,
            hidden: false,
        }];
        let lifted_out =
            evaluate_ignored_tests(&[], &added, &default_settings, &directive, false).unwrap();
        assert_eq!(lifted_out.violations.len(), 1);
        assert!(lifted_out.violations[0].message.contains("test_generic"));
        assert_eq!(lifted_out.overrides.len(), 1);
    }

    #[test]
    fn ignored_tests_ci_skip_severity_configuration() {
        let ci_test = TestFn {
            name: "test_ci".to_string(),
            line: 5,
            conditional_ignore: Some("os.Getenv(\"CI\") != \"\"".to_string()),
            ..Default::default()
        };
        let added = [Located {
            path: "tests/suite.go",
            file_survives: true,
            test: &ci_test,
        }];

        let settings = crate::config::IgnoredTestsGate {
            ci_skip_severity: Some(crate::config::Severity::Warning),
            ..Default::default()
        };
        let out = evaluate_ignored_tests(&[], &added, &settings, &[], false).unwrap();
        assert_eq!(out.violations.len(), 1);
        assert_eq!(out.violations[0].severity, crate::config::Severity::Warning);
    }

    #[test]
    fn ignored_tests_approved_predicates_boundary_matching_and_negative_controls() {
        let miri_ci_test = TestFn {
            name: "test_miri_ci".to_string(),
            line: 1,
            conditional_ignore: Some("cfg!(miri) || std::env::var(\"CI\").is_ok()".to_string()),
            ..Default::default()
        };
        let gitlab_test = TestFn {
            name: "test_gitlab".to_string(),
            line: 10,
            conditional_ignore: Some("os.Getenv(\"GITLAB_CI\") != \"\"".to_string()),
            ..Default::default()
        };
        let miri_like_ci_test = TestFn {
            name: "test_miri_like_ci".to_string(),
            line: 20,
            conditional_ignore: Some(
                "if os.Getenv(\"SKIP_MIRI_LIKE\") != \"\" || os.Getenv(\"CI\") != \"\"".to_string(),
            ),
            ..Default::default()
        };
        let ci_test = TestFn {
            name: "test_ci".to_string(),
            line: 30,
            conditional_ignore: Some("os.Getenv(\"CI\") != \"\"".to_string()),
            ..Default::default()
        };

        let added = [
            Located {
                path: "t.rs",
                file_survives: true,
                test: &miri_ci_test,
            },
            Located {
                path: "t.go",
                file_survives: true,
                test: &gitlab_test,
            },
            Located {
                path: "t.go",
                file_survives: true,
                test: &miri_like_ci_test,
            },
            Located {
                path: "t.go",
                file_survives: true,
                test: &ci_test,
            },
        ];

        let default_settings = crate::config::IgnoredTestsGate::default();

        // 1. Negative control: approving "miri" does NOT waive "cfg!(miri) || CI"
        let mut miri_approved = default_settings.clone();
        miri_approved.approved_predicates = vec!["miri".to_string()];
        let out_miri = evaluate_ignored_tests(&[], &added, &miri_approved, &[], false).unwrap();
        let v_miri_ci = out_miri
            .violations
            .iter()
            .find(|v| v.message.contains("test_miri_ci"))
            .unwrap();
        assert_eq!(v_miri_ci.severity, crate::config::Severity::Error);

        // 2. Negative control: approving "CI" does NOT waive "GITLAB_CI"
        let mut ci_approved = default_settings.clone();
        ci_approved.approved_predicates = vec!["CI".to_string()];
        let out_ci = evaluate_ignored_tests(&[], &added, &ci_approved, &[], false).unwrap();
        let v_gitlab = out_ci
            .violations
            .iter()
            .find(|v| v.message.contains("test_gitlab"))
            .unwrap();
        assert_eq!(v_gitlab.severity, crate::config::Severity::Error);

        // 3. Negative control: approving "SKIP" does NOT waive "SKIP_MIRI_LIKE"
        let mut skip_approved = default_settings.clone();
        skip_approved.approved_predicates = vec!["SKIP".to_string()];
        let out_skip = evaluate_ignored_tests(&[], &added, &skip_approved, &[], false).unwrap();
        let v_miri_like = out_skip
            .violations
            .iter()
            .find(|v| v.message.contains("test_miri_like_ci"))
            .unwrap();
        assert_eq!(v_miri_like.severity, crate::config::Severity::Error);

        // 4. Negative control: approving "C" does NOT waive "CI"
        let mut c_approved = default_settings.clone();
        c_approved.approved_predicates = vec!["C".to_string()];
        let out_c = evaluate_ignored_tests(&[], &added, &c_approved, &[], false).unwrap();
        let v_ci = out_c
            .violations
            .iter()
            .find(|v| v.message.contains("test_ci"))
            .unwrap();
        assert_eq!(v_ci.severity, crate::config::Severity::Error);

        // 5. Positive controls: approving all required CI vars waives them
        let mut all_approved = default_settings.clone();
        all_approved.approved_predicates = vec![
            "miri".to_string(),
            "CI".to_string(),
            "GITLAB_CI".to_string(),
            "SKIP_MIRI_LIKE".to_string(),
        ];
        let out_all = evaluate_ignored_tests(&[], &added, &all_approved, &[], false).unwrap();
        assert_eq!(out_all.violations.len(), 0);
    }

    /// A paired test whose conditional skip is `base` on the base side and `head` on the
    /// head side, evaluated with `settings`.
    fn changed_condition_outcome(
        base: &str,
        head: &str,
        settings: &crate::config::IgnoredTestsGate,
        directives: &[crate::tokens::ParsedDirective],
        is_staged: bool,
    ) -> GateOutcome {
        let b = TestFn {
            name: "TestA".to_string(),
            line: 9,
            conditional_ignore: Some(base.to_string()),
            ..Default::default()
        };
        let h = TestFn {
            name: "TestA".to_string(),
            line: 9,
            conditional_ignore: Some(head.to_string()),
            ..Default::default()
        };
        let pairs = [TestPair {
            path: "p_test.go",
            base: &b,
            head: &h,
            forced: false,
        }];
        evaluate_ignored_tests(&pairs, &[], settings, directives, is_staged).unwrap()
    }

    const SHORT: &str = "testing.Short()";
    const SHORT_OR_CI: &str = "testing.Short() || os.Getenv(\"CI\") != \"\"";

    #[test]
    fn ignored_tests_ci_condition_added_to_an_existing_conditional_skip_is_reported() {
        let settings = crate::config::IgnoredTestsGate::default();
        let out = changed_condition_outcome(SHORT, SHORT_OR_CI, &settings, &[], false);
        assert_eq!(out.violations.len(), 1);
        let v = &out.violations[0];
        assert_eq!(v.title, "Test Conditionally Skipped");
        assert_eq!(v.severity, crate::config::Severity::Error);
        assert!(v.message.contains("TestA"));
        assert!(v.message.contains(SHORT_OR_CI));
        assert!(v
            .remediation
            .as_deref()
            .unwrap()
            .contains("allow-ignore: TestA <reason>"));

        // Staged mode softens it, as for a newly added CI-conditional skip.
        let staged = changed_condition_outcome(SHORT, SHORT_OR_CI, &settings, &[], true);
        assert_eq!(staged.violations.len(), 1);
        assert_eq!(
            staged.violations[0].severity,
            crate::config::Severity::Warning
        );

        // `ci_skip_severity` sets the severity.
        let warning = crate::config::IgnoredTestsGate {
            ci_skip_severity: Some(crate::config::Severity::Warning),
            ..Default::default()
        };
        let out_warning = changed_condition_outcome(SHORT, SHORT_OR_CI, &warning, &[], false);
        assert_eq!(out_warning.violations.len(), 1);
        assert_eq!(
            out_warning.violations[0].severity,
            crate::config::Severity::Warning
        );

        // An approved CI predicate waives it.
        let approved = crate::config::IgnoredTestsGate {
            approved_predicates: vec!["CI".to_string()],
            ..Default::default()
        };
        let out_approved = changed_condition_outcome(SHORT, SHORT_OR_CI, &approved, &[], false);
        assert_eq!(out_approved.violations.len(), 0);

        // The directive lifts it and is recorded.
        let directive = [crate::tokens::ParsedDirective {
            directive: "allow-ignore".to_string(),
            reason: "TestA flaky on the shared runner".to_string(),
            source: crate::tokens::OverrideSource::PrBody,
            hidden: false,
        }];
        let lifted = changed_condition_outcome(SHORT, SHORT_OR_CI, &settings, &directive, false);
        assert_eq!(lifted.violations.len(), 0);
        assert_eq!(lifted.overrides.len(), 1);
    }

    #[test]
    fn ignored_tests_conditional_skip_that_gains_no_ci_condition_is_not_reported() {
        let settings = crate::config::IgnoredTestsGate::default();
        for (base, head) in [
            // Unchanged, CI-conditional on both sides: the gate is delta-only.
            (SHORT_OR_CI, SHORT_OR_CI),
            // Unchanged, not CI-conditional.
            (SHORT, SHORT),
            // Reformatted CI condition.
            (SHORT_OR_CI, "os.Getenv(\"CI\") != \"\" || testing.Short()"),
            // Changed, neither side CI-conditional.
            (SHORT, "testing.Short() || runtime.GOOS == \"windows\""),
            // CI condition removed: a tightening.
            (SHORT_OR_CI, SHORT),
        ] {
            let out = changed_condition_outcome(base, head, &settings, &[], false);
            assert_eq!(out.violations.len(), 0, "`{base}` -> `{head}`");
            assert_eq!(out.overrides.len(), 0, "`{base}` -> `{head}`");
        }
    }

    const CI_ONLY: &str = "os.Getenv(\"CI\") != \"\"";
    const CI_OR_GITHUB: &str = "os.Getenv(\"CI\") != \"\" || os.Getenv(\"GITHUB_ACTIONS\") != \"\"";

    /// #597: a skip that was already CI-conditional and reads one more CI variable stops
    /// the test on one more CI system (`CI` to `CI || GITHUB_ACTIONS`, the row that was
    /// pinned as not reported before the rule existed).
    #[test]
    fn ignored_tests_ci_variable_added_to_a_ci_conditional_skip_is_reported_unless_approved() {
        let settings = crate::config::IgnoredTestsGate::default();
        let out = changed_condition_outcome(CI_ONLY, CI_OR_GITHUB, &settings, &[], false);
        assert_eq!(out.violations.len(), 1);
        let v = &out.violations[0];
        assert_eq!(v.title, "Test Conditionally Skipped");
        assert_eq!(v.severity, crate::config::Severity::Error);
        assert_eq!(
            v.message,
            format!("Test `TestA` is conditionally skipped under predicate `{CI_OR_GITHUB}`, which adds CI variable `GITHUB_ACTIONS` to a skip that was already CI-conditional.")
        );

        // The base variable approved, the added one not: still reported.
        let ci_approved = crate::config::IgnoredTestsGate {
            approved_predicates: vec!["CI".to_string()],
            ..Default::default()
        };
        let out = changed_condition_outcome(CI_ONLY, CI_OR_GITHUB, &ci_approved, &[], false);
        assert_eq!(out.violations.len(), 1);
        assert!(out.violations[0]
            .message
            .contains("adds CI variable `GITHUB_ACTIONS`"));

        // The added variable approved: not reported, whether or not the base one is.
        for approved in [vec!["GITHUB_ACTIONS"], vec!["CI", "GITHUB_ACTIONS"]] {
            let both = crate::config::IgnoredTestsGate {
                approved_predicates: approved.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            };
            let out = changed_condition_outcome(CI_ONLY, CI_OR_GITHUB, &both, &[], false);
            assert_eq!(out.violations.len(), 0, "{approved:?}");
        }

        // `ci_skip_severity`, staged mode and the directive apply as to any CI skip.
        let warning = crate::config::IgnoredTestsGate {
            ci_skip_severity: Some(crate::config::Severity::Warning),
            ..Default::default()
        };
        let out = changed_condition_outcome(CI_ONLY, CI_OR_GITHUB, &warning, &[], false);
        assert_eq!(out.violations[0].severity, crate::config::Severity::Warning);
        let staged = changed_condition_outcome(CI_ONLY, CI_OR_GITHUB, &settings, &[], true);
        assert_eq!(
            staged.violations[0].severity,
            crate::config::Severity::Warning
        );
        let directive = [crate::tokens::ParsedDirective {
            directive: "allow-ignore".to_string(),
            reason: "TestA flaky on the shared runner".to_string(),
            source: crate::tokens::OverrideSource::PrBody,
            hidden: false,
        }];
        let lifted = changed_condition_outcome(CI_ONLY, CI_OR_GITHUB, &settings, &directive, false);
        assert_eq!(lifted.violations.len(), 0);
        assert_eq!(lifted.overrides.len(), 1);

        // Controls: no CI variable is added.
        for (base, head) in [
            (CI_OR_GITHUB, CI_OR_GITHUB),
            (CI_OR_GITHUB, CI_ONLY),
            (CI_ONLY, "os.Getenv(\"CI\") != \"\" && testing.Short()"),
            (CI_ONLY, "\"\" != os.Getenv(\"CI\")"),
        ] {
            let out = changed_condition_outcome(base, head, &settings, &[], false);
            assert_eq!(out.violations.len(), 0, "`{base}` -> `{head}`");
        }
    }
}
