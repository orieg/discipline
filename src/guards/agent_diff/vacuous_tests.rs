//! `vacuous-tests`: an added test that checks nothing.

use super::{leaf_name, lines_of, note_self_comparison_scope, Located, SELF_COMPARISON_SCOPE};
use crate::config::GateSettings;
use crate::guards::{exempt_filter, GateOutcome};
use crate::tokens;
use anyhow::Result;

pub fn evaluate_vacuous_tests(
    added: &[Located],
    settings: &crate::config::AssertionGate,
    directives: &[crate::tokens::ParsedDirective],
) -> Result<GateOutcome> {
    const GATE: &str = "vacuous-tests";
    let exempt = exempt_filter(settings)?;
    let mut out = GateOutcome::new(GATE);
    out.examined = added.len();

    for a in added.iter().filter(|a| !exempt.matches(a.path)) {
        // The finding this test would raise, in the order the checks below report it;
        // a sound test raises none, and a directive naming it lifts nothing.
        let mocks_only = a.test.mock_asserts > 0
            && a.test.mock_asserts >= a.test.checks()
            && a.test.strong_asserts == 0
            && a.test.should_panic.is_none()
            && a.test.expected_exceptions.is_empty();
        let trivial_only = a.test.trivial_asserts > 0
            && a.test.trivial_asserts >= a.test.checks()
            && a.test.should_panic.is_none()
            && a.test.expected_exceptions.is_empty();
        let below_floor = settings
            .min_assertions_per_test
            .is_some_and(|min| a.test.checks() < min);
        // Equality assertions that compare an operand with itself
        // (`ast::self_comparison`). One finding a test: a test with nothing else that
        // can fail is the vacuous test below, which names them.
        let self_compared = a.test.equality_operands.reportable();
        let lifts = if mocks_only {
            &crate::findings::ASSERTS_ONLY_ON_MOCKS
        } else if trivial_only {
            &crate::findings::ASSERTS_ONLY_TRIVIAL
        } else if a.test.is_vacuous() {
            &crate::findings::VACUOUS_TEST_ADDED
        } else if below_floor {
            &crate::findings::ASSERTION_DENSITY_BELOW_FLOOR
        } else if !self_compared.is_empty() {
            &crate::findings::SELF_COMPARISON_ASSERTION_ADDED
        } else {
            continue;
        };
        // `allow-vacuous-test: <test> <reason>` lifts every finding on that one test.
        if let Some(record) = tokens::find_override(
            directives,
            GATE,
            lifts,
            tokens::ALLOW_VACUOUS_TEST,
            leaf_name(a.test),
        ) {
            out.overrides.push(record);
            continue;
        }
        // Every assertion is on a double's interactions: the test checks that the mock
        // was called, and nothing about what the code produced.
        if a.test.mock_asserts > 0
            && a.test.mock_asserts >= a.test.checks()
            && a.test.strong_asserts == 0
            && a.test.should_panic.is_none()
            && a.test.expected_exceptions.is_empty()
        {
            out.push(
                crate::config::Severity::Warning,
                &crate::findings::ASSERTS_ONLY_ON_MOCKS,
                Some(a.path),
                Some(a.test.line),
                format!(
                    "New test `{}` makes {} assertion(s), all on test-double interactions; it does not check what the code produces.",
                    a.test.name, a.test.mock_asserts
                ),
                "Assert on the result or the observable effect as well; interaction checks alone pass whatever the code returns.",
            );
            continue;
        }
        // Every assertion holds for nearly any value: `is not None`, `toBeDefined`,
        // `is_ok()`. The test runs the code and checks that something came back.
        // (`is not None` is a comparison, so the pack may count it as strong; the
        // trivial count decides.)
        if a.test.trivial_asserts > 0
            && a.test.trivial_asserts >= a.test.checks()
            && a.test.should_panic.is_none()
            && a.test.expected_exceptions.is_empty()
        {
            out.push(
                crate::config::Severity::Warning,
                &crate::findings::ASSERTS_ONLY_TRIVIAL,
                Some(a.path),
                Some(a.test.line),
                format!(
                    "New test `{}` makes {} assertion(s) that hold for nearly any value (not-null, defined, ok, truthy); it does not check what the code produced.",
                    a.test.name, a.test.trivial_asserts
                ),
                "Assert on the value or the effect; a not-null check passes any wrong answer.",
            );
            continue;
        }
        if a.test.is_vacuous() {
            let why = if a.test.total_asserts == 0 {
                "contains no assertion".to_string()
            } else if self_compared.is_empty() {
                format!(
                    "contains only tautological assertions ({} of {})",
                    a.test.tautologies, a.test.total_asserts
                )
            } else {
                format!(
                    "contains only tautological assertions ({} of {}), of which {} compare(s) an expression with itself (line {})",
                    a.test.tautologies,
                    a.test.total_asserts,
                    self_compared.len(),
                    lines_of(&self_compared)
                )
            };
            out.push(
                settings.severity(),
                &crate::findings::VACUOUS_TEST_ADDED,
                Some(a.path),
                Some(a.test.line),
                format!("New test `{}` {why}; it cannot fail.", a.test.name),
                "Assert the behavior under test. If the suite asserts through helpers or custom \
                 macros, declare them in `assert_helper_fns` / `extra_assert_macros`; a test that \
                 is meant not to assert (a smoke test) takes `allow-vacuous-test: <test> <reason>`.",
            );
            out.anchor_last(a.test.name.clone());
            continue;
        }
        if !self_compared.is_empty() {
            out.push(
                crate::config::Severity::Warning,
                &crate::findings::SELF_COMPARISON_ASSERTION_ADDED,
                Some(a.path),
                Some(self_compared[0].line),
                format!(
                    "New test `{}`: {} equality assertion(s) compare an expression with itself (line {}), so they hold whatever the code does. {SELF_COMPARISON_SCOPE}",
                    a.test.name,
                    self_compared.len(),
                    lines_of(&self_compared)
                ),
                &format!(
                    "Compare the value with what is expected of it. A deliberate reflexivity check takes `allow-vacuous-test: {} <reason>`.",
                    leaf_name(a.test)
                ),
            );
            out.anchor_last(a.test.name.clone());
            note_self_comparison_scope(&mut out);
        }
        if let Some(min) = settings.min_assertions_per_test {
            if a.test.checks() < min {
                out.push(
                    settings.severity(),
                    &crate::findings::ASSERTION_DENSITY_BELOW_FLOOR,
                    Some(a.path),
                    Some(a.test.line),
                    format!(
                        "New test `{}` contains {} effective assertion(s), failing minimum assertion density floor of {min}.",
                        a.test.name,
                        a.test.checks()
                    ),
                    "Add additional discriminating assertions to meet the configured assertion density floor.",
                );
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::TestFn;

    #[test]
    fn test_vacuous_tests_pure() {
        let settings = crate::config::AssertionGate::default();
        let t_empty = TestFn {
            name: "empty".to_string(),
            line: 10,
            total_asserts: 0,
            strong_asserts: 0,
            tautologies: 0,
            ignored: false,
            should_panic: None,
            ..Default::default()
        };
        let t_tautology = TestFn {
            name: "tauto".to_string(),
            line: 20,
            total_asserts: 1,
            strong_asserts: 1,
            tautologies: 1,
            ignored: false,
            should_panic: None,
            ..Default::default()
        };
        let t_real = TestFn {
            name: "real".to_string(),
            line: 30,
            total_asserts: 1,
            strong_asserts: 1,
            tautologies: 0,
            ignored: false,
            should_panic: None,
            ..Default::default()
        };

        let loc_empty = [Located {
            path: "tests/a.rs",
            file_survives: true,
            test: &t_empty,
        }];
        let out_empty = evaluate_vacuous_tests(&loc_empty, &settings, &[]).unwrap();
        assert_eq!(out_empty.violations.len(), 1);
        assert!(out_empty.violations[0]
            .message
            .contains("contains no assertion"));

        // `allow-vacuous-test` naming the test lifts it and records the override; naming
        // another test lifts nothing.
        let lift = |body: &str| {
            let d = tokens::parse_directives(body, tokens::OverrideSource::PrBody);
            evaluate_vacuous_tests(&loc_empty, &settings, &d).unwrap()
        };
        let lifted = lift(&format!(
            "allow-vacuous-test: {} smoke test\n",
            t_empty.name
        ));
        assert!(lifted.violations.is_empty());
        assert_eq!(lifted.overrides.len(), 1);
        let other = lift("allow-vacuous-test: some_other_test smoke test\n");
        assert_eq!(other.violations.len(), 1);
        assert!(other.overrides.is_empty());

        let loc_tauto = [Located {
            path: "tests/a.rs",
            file_survives: true,
            test: &t_tautology,
        }];
        let out_tauto = evaluate_vacuous_tests(&loc_tauto, &settings, &[]).unwrap();
        assert_eq!(out_tauto.violations.len(), 1);
        assert!(out_tauto.violations[0]
            .message
            .contains("contains only tautological assertions"));

        let loc_real = [Located {
            path: "tests/a.rs",
            file_survives: true,
            test: &t_real,
        }];
        let out_real = evaluate_vacuous_tests(&loc_real, &settings, &[]).unwrap();
        assert_eq!(out_real.violations.len(), 0);
    }
}
