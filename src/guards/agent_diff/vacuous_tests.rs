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
        let mocks_only = asserts_only_on_mocks(a.test);
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
        // Every assertion is on a double's interactions, or on a double's interactions
        // and a property nearly any value has: the test checks that the mock was called,
        // and nothing about what the code produced.
        if mocks_only {
            let message = if a.test.trivial_asserts == 0 {
                format!(
                    "New test `{}` makes {} assertion(s), all on test-double interactions; it does not check what the code produces.",
                    a.test.name, a.test.mock_asserts
                )
            } else {
                format!(
                    "New test `{}` asserts only on mocks and trivial properties: {} check(s) of test-double interactions and {} that hold for nearly any value (not-null, defined, ok, truthy); it does not check what the code produces.",
                    a.test.name, a.test.mock_asserts, a.test.trivial_asserts
                )
            };
            out.push(
                crate::config::Severity::Warning,
                &crate::findings::ASSERTS_ONLY_ON_MOCKS,
                Some(a.path),
                Some(a.test.line),
                message,
                "Assert on the result or the observable effect as well; interaction checks alone pass whatever the code returns.",
            );
            continue;
        }
        // Every assertion holds for nearly any value: `is not None`, `toBeDefined`,
        // `is_ok()`. The test runs the code and checks that something came back.
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

/// Whether every check of `test` is on a double's interactions, or on a double's
/// interactions and properties that hold for nearly any value (`is not None`,
/// `toBeDefined`, `assertNotNull`), with at least one interaction check.
///
/// A pack that counts an interaction check among its assertions (a called
/// `unittest.mock` assertion method, a Jest `toHaveBeenCalledWith`) says so in
/// `mock_checks`, and such a check explains its own assertion. A pack that does not (a
/// Java `verify(`, a mockall checkpoint) leaves every assertion to be explained as
/// trivial, or the test to have no strong assertion and no more checks than
/// interaction checks.
fn asserts_only_on_mocks(test: &crate::ast::TestFn) -> bool {
    if test.mock_asserts == 0 || test.should_panic.is_some() || !test.expected_exceptions.is_empty()
    {
        return false;
    }
    let explained = test
        .mock_checks
        .min(test.mock_asserts)
        .saturating_add(test.trivial_asserts);
    (test.mock_asserts >= test.checks() && test.strong_asserts == 0) || test.checks() <= explained
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

/// #865: a new test whose checks are all on a double's interactions or on properties
/// nearly any value has is `asserts-only-on-mocks`, in every pack; a check on the result
/// beside either keeps it quiet, and each weak kind alone keeps its own finding.
#[cfg(all(
    test,
    feature = "lang-python",
    feature = "lang-javascript",
    feature = "lang-java",
    feature = "lang-go",
    feature = "lang-rust"
))]
mod mocks_and_trivial_properties {
    use super::*;

    const MOCKS: &str = "vacuous-tests/asserts-only-on-mocks";
    const TRIVIAL: &str = "vacuous-tests/asserts-only-trivial-properties";
    const MIXED: &str = "only on mocks and trivial properties";

    /// `(code, whether the message names both weak kinds)` of each finding on the file.
    fn verdict(path: &str, src: &str) -> Vec<(String, bool)> {
        let facts = crate::ast::default_registry()
            .find_pack(path)
            .unwrap()
            .extract(path, src, &crate::ast::AssertVocabulary::default())
            .unwrap();
        assert_eq!(facts.tests.len(), 1, "{src}");
        let added = [Located {
            path,
            file_survives: true,
            test: &facts.tests[0],
        }];
        evaluate_vacuous_tests(&added, &crate::config::AssertionGate::default(), &[])
            .unwrap()
            .violations
            .iter()
            .map(|v| (v.code.clone(), v.message.contains(MIXED)))
            .collect()
    }

    fn mixed() -> Vec<(String, bool)> {
        vec![(MOCKS.to_string(), true)]
    }

    fn only(code: &str) -> Vec<(String, bool)> {
        vec![(code.to_string(), false)]
    }

    #[test]
    fn python() {
        let py = |body: &str| {
            verdict(
                "tests/test_a.py",
                &format!("def test_run():\n    m = mock.Mock()\n    r = run(m)\n{body}"),
            )
        };
        assert_eq!(
            py("    m.assert_called_once_with(1)\n    assert r is not None\n"),
            mixed()
        );
        assert_eq!(
            py("    m.assert_called_once_with(1)\n    assert isinstance(r, int)\n"),
            mixed()
        );
        // Controls: a check on the result beside either weak kind; each weak kind alone.
        assert!(py("    m.assert_called_once_with(1)\n    assert r == 2\n").is_empty());
        assert!(py("    assert r is not None\n    assert r == 2\n").is_empty());
        assert_eq!(py("    m.assert_called_once_with(1)\n"), only(MOCKS));
        assert_eq!(py("    assert r is not None\n"), only(TRIVIAL));
    }

    #[test]
    fn javascript_and_typescript() {
        for path in ["test/a.test.js", "test/a.test.ts"] {
            let js = |body: &str| {
                verdict(
                    path,
                    &format!(
                        "test('run', () => {{\n  const m = jest.fn();\n  const r = run(m);\n{body}}});\n"
                    ),
                )
            };
            assert_eq!(
                js("  expect(m).toHaveBeenCalledWith(1);\n  expect(r).toBeDefined();\n"),
                mixed(),
                "{path}"
            );
            assert_eq!(
                js("  expect(m).toHaveBeenCalled();\n  expect(r).toBeTruthy();\n"),
                mixed(),
                "{path}"
            );
            // Controls: a check on the result beside either weak kind; each weak kind alone.
            assert!(js("  expect(m).toHaveBeenCalledWith(1);\n  expect(r).toBe(2);\n").is_empty());
            assert!(js("  expect(r).toBeDefined();\n  expect(r).toBe(2);\n").is_empty());
            assert_eq!(js("  expect(m).toHaveBeenCalled();\n"), only(MOCKS));
            assert_eq!(js("  expect(m).toHaveBeenCalledWith(1);\n"), only(MOCKS));
            assert_eq!(js("  expect(r).toBeDefined();\n"), only(TRIVIAL));
        }
    }

    /// Packs that do not count an interaction check among a test's assertions.
    #[test]
    fn java_go_and_rust() {
        let java = |body: &str| {
            verdict(
                "src/test/java/ATest.java",
                &format!("class ATest {{\n  @Test\n  void run() {{\n    Repo m = mock(Repo.class);\n    Object r = new Svc(m).run();\n{body}  }}\n}}\n"),
            )
        };
        assert_eq!(
            java("    verify(m).find(1);\n    assertNotNull(r);\n"),
            mixed()
        );
        assert!(java("    verify(m).find(1);\n    assertEquals(2, r);\n").is_empty());
        assert!(java("    assertNotNull(r);\n    assertEquals(2, r);\n").is_empty());
        assert_eq!(java("    verify(m).find(1);\n"), only(MOCKS));
        assert_eq!(java("    assertNotNull(r);\n"), only(TRIVIAL));

        let go = |body: &str| {
            verdict(
                "a_test.go",
                &format!("package a\nimport \"testing\"\nfunc TestRun(t *testing.T) {{\n\tm := new(MockRepo)\n\tm.On(\"Find\", 1).Return(2)\n\tr := Run(m)\n{body}}}\n"),
            )
        };
        assert_eq!(
            go("\tm.AssertExpectations(t)\n\tassert.NotNil(t, r)\n"),
            mixed()
        );
        assert!(go("\tm.AssertExpectations(t)\n\tassert.Equal(t, 2, r)\n").is_empty());
        assert!(go("\tassert.NotNil(t, r)\n\tassert.Equal(t, 2, r)\n").is_empty());
        assert_eq!(go("\tassert.NotNil(t, r)\n"), only(TRIVIAL));

        let rust = |body: &str| {
            verdict(
                "tests/a.rs",
                &format!("#[test]\nfn t() {{\n    let mut m = MockRepo::new();\n    m.expect_find().returning(|_| 1);\n    let r = run(&m);\n{body}}}\n"),
            )
        };
        assert_eq!(
            rust("    m.checkpoint();\n    assert!(r.is_some());\n"),
            mixed()
        );
        assert!(rust("    m.checkpoint();\n    assert_eq!(r, Some(1));\n").is_empty());
        // A check that is neither trivial nor an equality is still a check on the result.
        assert!(rust(
            "    m.checkpoint();\n    assert!(r.is_some());\n    assert!(r.unwrap() > 3);\n"
        )
        .is_empty());
        assert_eq!(rust("    m.checkpoint();\n"), only(MOCKS));
        assert_eq!(rust("    assert!(r.is_some());\n"), only(TRIVIAL));
    }
}
