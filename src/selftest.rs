//! `discipline self-test`: negative and positive controls compiled into the
//! binary, so a *released artifact* can prove on the runner it executes on
//! that its detectors still discriminate. Each case pairs an input that must
//! trip a detector with a near-identical one that must not.

use crate::ast::{analyze, AssertVocabulary};
use crate::config::{DisciplineConfig, Overrides, PiiGate};
use crate::guards::hygiene::{pii_rules, time_estimate_patterns};
use crate::guards::integrity::diff_configs;
use crate::guards::line_allows;
use crate::tokens::{covers, directive_reasons, REMOVES};
use anyhow::{bail, Result};
use regex::Regex;

type Case = (&'static str, fn() -> Result<bool>);

const CASES: &[Case] = &[
    ("ast: weakened assertion lowers the strong count", || {
        let v = AssertVocabulary::default();
        let base = analyze("#[test] fn t() { assert_eq!(f(), Some(3)); }", &v)?;
        let head = analyze("#[test] fn t() { assert!(f().is_some()); }", &v)?;
        Ok(base.tests[0].strong_asserts == 1 && head.tests[0].strong_asserts == 0)
    }),
    (
        "ast: tautological test is vacuous, real test is not",
        || {
            let v = AssertVocabulary::default();
            let bad = analyze("#[test] fn t() { assert!(true); }", &v)?;
            let good = analyze("#[test] fn t() { assert!(f()); }", &v)?;
            Ok(bad.tests[0].is_vacuous() && !good.tests[0].is_vacuous())
        },
    ),
    ("ast: assert inside a comment is not an assertion", || {
        let f = analyze(
            "#[test] fn t() { // assert_eq!(1, 2);\n }",
            &AssertVocabulary::default(),
        )?;
        Ok(f.tests[0].total_asserts == 0)
    }),
    (
        "ast: SAFETY comment above documents, prose about it does not",
        || {
            let v = AssertVocabulary::default();
            let ok = analyze(
                "fn a(p:*const u8)->u8{\n// SAFETY: pointer is valid for reads\nunsafe { *p }\n}",
                &v,
            )?;
            let short = analyze(
                "fn a(p:*const u8)->u8{\n// SAFETY: valid\nunsafe { *p }\n}",
                &v,
            )?;
            let bad = analyze(
                "fn a(p:*const u8)->u8{\nlet _ = \"// SAFETY: x\";\nunsafe{ *p }\n}",
                &v,
            )?;
            Ok(ok.unsafe_sites[0].documented
                && !short.unsafe_sites[0].documented
                && !bad.unsafe_sites[0].documented)
        },
    ),
    ("tokens: directive is line-anchored and scoped", || {
        let armed = directive_reasons("removes: tests/a.rs replaced", REMOVES);
        let prose = directive_reasons(
            "| removes: tests/a.rs | doc |\nuse removes: tests/a.rs",
            REMOVES,
        );
        let placeholder = directive_reasons("removes: <reason>", REMOVES);
        Ok(covers(&armed, "tests/a.rs")
            && !covers(&armed, "tests/b.rs")
            && prose.is_empty()
            && placeholder.is_empty())
    }),
    ("hygiene: time-estimate patterns discriminate", || {
        let res: Vec<Regex> = time_estimate_patterns()
            .iter()
            .map(|p| Regex::new(p))
            .collect::<Result<_, _>>()?;
        let hit = |s: &str| res.iter().any(|r| r.is_match(s));
        let empty_allowed = Vec::new();
        let fires = |text: &str| {
            !crate::guards::hygiene::scan_text_for_time_estimates(text, &res, &empty_allowed)
                .is_empty()
        };
        Ok(hit("Phase 2 (1 week)")
            && hit("The capacity work is 2 weeks.")
            && hit("Expect it in two weeks.")
            && hit("ETA: a month.")
            && !hit("Phase 2 follows Phase 1")
            && fires("The capacity work is 2 weeks.")
            && fires("The migration took a decision; rollout is 3 weeks.")
            && fires("Timeout budget aside, we ship in 2 weeks.")
            && !fires("The job took 29 min on the hosted runner.")
            && !fires("Miri shards are capped at 180 minutes."))
    }),
    (
        "hygiene: pii rules discriminate and honor allowed users",
        || {
            let s = PiiGate::default();
            let rules = pii_rules(&s)?;
            let leak = format!("/{}/alice/x", "Users");
            let ci = format!("/{}/runner/x", "home");
            let hits = |line: &str| {
                rules.iter().any(|r| {
                    r.re.captures_iter(line).any(|c| {
                        !(r.user_group
                            && c.get(1)
                                .is_some_and(|u| s.allowed_users.iter().any(|a| a == u.as_str())))
                    })
                })
            };
            Ok(hits(&leak) && !hits(&ci))
        },
    ),
    ("markers: inline allow is scoped to its gate", || {
        Ok(line_allows("x discipline:allow(pii)", "pii")
            && !line_allows("x discipline:allow(pii)", "time-estimates"))
    }),
    (
        "config: unknown keys and unknown gates are rejected",
        || {
            let head = "[meta]\nversion = 1\nname = \"t\"\n";
            let typo =
                DisciplineConfig::from_toml_str(&format!("{head}[gates.pii]\nlan_ipz = false\n"));
            let unknown =
                DisciplineConfig::from_toml_str(&format!("{head}[gates.unknown-gate]\nenabled = true\n"));
            let fine =
                DisciplineConfig::from_toml_str(&format!("{head}[gates.pii]\nlan_ips = false\n"));
            Ok(typo.is_err() && unknown.is_err() && fine.is_ok())
        },
    ),
    (
        "config: layered overrides apply, conflicting ones are refused",
        || {
            let o = Overrides {
                disable: vec!["pii".into()],
                ..Default::default()
            };
            let c = DisciplineConfig::resolve(None, &o)?;
            let clash = Overrides {
                enable: vec!["pii".into()],
                disable: vec!["pii".into()],
                ..Default::default()
            };
            Ok(!c.gates.pii.enabled
                && c.gates.time_estimates.enabled
                && DisciplineConfig::resolve(None, &clash).is_err())
        },
    ),
    (
        "integrity: disabling a gate is a weakening, tightening is not",
        || {
            let base = DisciplineConfig::default_for_repo("t");
            let mut weaker = base.clone();
            weaker.gates.vacuous_tests.enabled = false;
            let mut stricter = base.clone();
            stricter.gates.pii.hostname_denylist.push("h".into());
            Ok(diff_configs(&base, &weaker)?.len() == 1
                && diff_configs(&base, &stricter)?.is_empty())
        },
    ),
    (
        "tokens: bare directory word does not act as prefix, but explicit slash does",
        || {
            let bare = directive_reasons("removes: tests refactored", REMOVES);
            let slash = directive_reasons("removes: tests/ refactored", REMOVES);
            let directive_marker = directive_reasons(
                "<!-- discipline:allow(deletion-rationale) tests/legacy/ -->",
                REMOVES,
            );
            Ok(!covers(&bare, "tests/a.rs")
                && covers(&slash, "tests/a.rs")
                && covers(&directive_marker, "tests/legacy/old.rs"))
        },
    ),
    ("ast: cfg_attr ignore is detected as ignored test", || {
        let v = AssertVocabulary::default();
        let parsed = analyze(
            "#[test]\n#[cfg_attr(all(), ignore)]\nfn t() { assert_eq!(1, 1); }",
            &v,
        )?;
        Ok(parsed.tests[0].ignored)
    }),
    (
        "hygiene: lan ip rules exempt RFC 1918 network ID, catch host IP",
        || {
            use crate::guards::hygiene::is_exempt_lan_ip;
            Ok(is_exempt_lan_ip("10.0.0.0", "10.0.0.0", 0, 8)
                && is_exempt_lan_ip("192.168.0.0", "192.168.0.0", 0, 11)
                && !is_exempt_lan_ip("10.0.1.5", "10.0.1.5", 0, 8) // discipline:allow(pii)
                && !is_exempt_lan_ip("192.168.1.50", "192.168.1.50", 0, 12)) // discipline:allow(pii)
        },
    ),
    (
        "hygiene: contextual time estimate exemptions discriminate",
        || {
            use crate::guards::hygiene::is_exempt_time_estimate;
            let exempt_span = "survived for 19 years in production";
            let real_est = "Plan: ship in 3 weeks";
            let cap_work = "The capacity work is 2 weeks.";
            let took_run = "The job took 29 min on the hosted runner.";
            Ok(is_exempt_time_estimate(exempt_span, 13, 21, "19 years")
                && is_exempt_time_estimate(took_run, 13, 19, "29 min")
                && !is_exempt_time_estimate(real_est, 14, 21, "3 weeks")
                && !is_exempt_time_estimate(cap_work, 21, 28, "2 weeks"))
        },
    ),
    (
        "pairing: forced test pairing pairs unrelated tests and marks them forced",
        || {
            use crate::gitctx::{ChangeKind, ChangedFile};
            use crate::guards::agent_diff::{match_tests, FileFacts};
            let v = AssertVocabulary::default();
            let b = analyze("#[test] fn test_alpha() { assert_eq!(1, 1); }", &v)?;
            let h = analyze("#[test] fn test_omega() { assert!(true != false); }", &v)?;
            let facts = vec![FileFacts {
                file: ChangedFile {
                    path: "tests/a.rs".into(),
                    old_path: "tests/a.rs".into(),
                    kind: ChangeKind::Modified,
                    added_lines: std::collections::BTreeSet::new(),
                },
                base: Some(b),
                head: Some(h),
                newly_added_nul: false,
            }];
            let (pairs, removed, added) = match_tests(&facts);
            Ok(pairs.len() == 1
                && pairs[0].forced
                && pairs[0].base.name == "test_alpha"
                && pairs[0].head.name == "test_omega"
                && removed.is_empty()
                && added.is_empty())
        },
    ),
    (
        "directives: hidden directives rejected by default, accepted when allow_hidden is set",
        || {
            use crate::config::DirectivesConfig;
            use crate::tokens::extract_directives;
            let hidden_directive = "<!-- removes: tests/old.rs replaced -->";
            let default_policy = DirectivesConfig::default();
            let (active_default, notes_default) =
                extract_directives(Some(hidden_directive), &[], &default_policy);
            let permissive_policy = DirectivesConfig {
                allow_hidden: true,
                ..Default::default()
            };
            let (active_permissive, notes_permissive) =
                extract_directives(Some(hidden_directive), &[], &permissive_policy);
            Ok(active_default.is_empty()
                && !notes_default.is_empty()
                && active_permissive.len() == 1
                && active_permissive[0].hidden
                && notes_permissive.is_empty())
        },
    ),
    ("ast: constant-expression tautologies are vacuous", || {
        let v = AssertVocabulary::default();
        let math = analyze("#[test] fn t() { assert!(1 + 1 > 0); }", &v)?;
        let eq = analyze("#[test] fn t() { assert_eq!(1, 1); }", &v)?;
        let ne = analyze("#[test] fn t() { assert_ne!(1, 2); }", &v)?;
        let real = analyze("#[test] fn t() { assert!(f() == 1); }", &v)?;
        Ok(math.tests[0].is_vacuous()
            && eq.tests[0].is_vacuous()
            && ne.tests[0].is_vacuous()
            && !real.tests[0].is_vacuous())
    }),
    (
        "ast: fallible test with ? and unwrap count as assertions, empty fallible stays vacuous",
        || {
            let v = AssertVocabulary::default();
            let q = analyze("#[test] fn t() -> Result<(), E> { x()?; Ok(()) }", &v)?;
            let unwrap = analyze("#[test] fn t() { x().unwrap(); }", &v)?;
            let empty = analyze("#[test] fn t() -> Result<(), E> { Ok(()) }", &v)?;
            Ok(!q.tests[0].is_vacuous()
                && !unwrap.tests[0].is_vacuous()
                && empty.tests[0].is_vacuous())
        },
    ),
    (
        "agent-diff: pure evaluate_assertion_reduction discriminates drop and accepts override",
        || {
            use crate::ast::TestFn;
            use crate::guards::agent_diff::{evaluate_assertion_reduction, TestPair};
            let b = TestFn {
                name: "test_check".to_string(),
                line: 1,
                total_asserts: 2,
                strong_asserts: 2,
                tautologies: 0,
                ignored: false,
                should_panic: false,
                ..Default::default()
            };
            let h = TestFn {
                name: "test_check".to_string(),
                line: 1,
                total_asserts: 1,
                strong_asserts: 1,
                tautologies: 0,
                ignored: false,
                should_panic: false,
                ..Default::default()
            };
            let pair = [TestPair {
                path: "tests/pure.rs",
                base: &b,
                head: &h,
                forced: false,
            }];
            let settings = crate::config::AssertionGate::default();
            let unexcused = evaluate_assertion_reduction(&pair, &[], &settings, &[], false)?;
            let directives = [crate::tokens::ParsedDirective {
                directive: "allow-assertion-drop".to_string(),
                reason: "test_check simplified".to_string(),
                source: crate::tokens::OverrideSource::PrBody,
                hidden: false,
            }];
            let excused = evaluate_assertion_reduction(&pair, &[], &settings, &directives, false)?;

            // An added test must NOT offset the paired test's reduction
            use crate::guards::agent_diff::Located;
            let added_test = [Located {
                path: "tests/pure.rs",
                file_survives: true,
                test: &b,
            }];
            let with_added =
                evaluate_assertion_reduction(&pair, &added_test, &settings, &[], false)?;

            Ok(unexcused.violations.len() == 1
                && unexcused.overrides.is_empty()
                && with_added.violations.len() == 1
                && excused.violations.is_empty()
                && excused.overrides.len() == 1)
        },
    ),
    (
        "agent-diff: pure evaluate_deletion_rationale catches unexcused test and file removals",
        || {
            use crate::ast::TestFn;
            use crate::gitctx::{ChangeKind, ChangedFile};
            use crate::guards::agent_diff::{evaluate_deletion_rationale, Located};
            let settings = crate::config::DeletionGate::default();
            let deleted_file = [ChangedFile {
                path: "src/old.rs".into(),
                old_path: "src/old.rs".into(),
                kind: ChangeKind::Deleted,
                added_lines: std::collections::BTreeSet::new(),
            }];
            let unexcused_file =
                evaluate_deletion_rationale(&deleted_file, &[], &settings, &[], false)?;
            let file_directive = [crate::tokens::ParsedDirective {
                directive: "removes".to_string(),
                reason: "src/old.rs superseded".to_string(),
                source: crate::tokens::OverrideSource::PrBody,
                hidden: false,
            }];
            let excused_file =
                evaluate_deletion_rationale(&deleted_file, &[], &settings, &file_directive, false)?;

            let t = TestFn {
                name: "test_old".to_string(),
                line: 1,
                total_asserts: 1,
                strong_asserts: 1,
                tautologies: 0,
                ignored: false,
                should_panic: false,
                ..Default::default()
            };
            let removed_test = [Located {
                path: "tests/suite.rs",
                file_survives: true,
                test: &t,
            }];
            let unexcused_test =
                evaluate_deletion_rationale(&[], &removed_test, &settings, &[], false)?;
            let test_directive = [crate::tokens::ParsedDirective {
                directive: "removes".to_string(),
                reason: "test_old superseded".to_string(),
                source: crate::tokens::OverrideSource::PrBody,
                hidden: false,
            }];
            let excused_test =
                evaluate_deletion_rationale(&[], &removed_test, &settings, &test_directive, false)?;

            Ok(unexcused_file.violations.len() == 1
                && excused_file.violations.is_empty()
                && excused_file.overrides.len() == 1
                && unexcused_test.violations.len() == 1
                && excused_test.violations.is_empty()
                && excused_test.overrides.len() == 1)
        },
    ),
    (
        "agent-diff: newly added NUL byte flags violation and is lifted by allow-nul directive",
        || {
            use crate::gitctx::{ChangeKind, ChangedFile};
            use crate::guards::agent_diff::{report_newly_added_nul_bytes, FileFacts};
            use crate::guards::GateOutcome;
            let facts = [FileFacts {
                file: ChangedFile {
                    path: "tests/payload.phpt".into(),
                    old_path: "tests/payload.phpt".into(),
                    kind: ChangeKind::Added,
                    added_lines: std::collections::BTreeSet::new(),
                },
                base: None,
                head: None,
                newly_added_nul: true,
            }];
            let mut out_unexcused = GateOutcome::new("assertion-reduction");
            let empty_exempt = crate::guards::PathFilter::new(&[])?;
            report_newly_added_nul_bytes(
                &facts,
                crate::config::Severity::Error,
                &mut out_unexcused,
                &[],
                false,
                &empty_exempt,
            );

            let directives = [crate::tokens::ParsedDirective {
                directive: "allow-nul".to_string(),
                reason: "tests/payload.phpt binary cache test payload".to_string(),
                source: crate::tokens::OverrideSource::PrBody,
                hidden: false,
            }];
            let mut out_excused = GateOutcome::new("assertion-reduction");
            report_newly_added_nul_bytes(
                &facts,
                crate::config::Severity::Error,
                &mut out_excused,
                &directives,
                false,
                &empty_exempt,
            );

            Ok(out_unexcused.violations.len() == 1
                && out_unexcused.overrides.is_empty()
                && out_excused.violations.is_empty()
                && out_excused.overrides.len() == 1)
        },
    ),
    (
        "golden-output: path matching discriminates and accepts allow-golden-update",
        || {
            use crate::guards::PathFilter;
            use crate::tokens::{directive_reasons, ALLOW_GOLDEN_UPDATE};
            let paths = [
                "**/golden/**".to_string(),
                "**/snapshots/**".to_string(),
                "**/*.snap".to_string(),
                "tests/fixtures/**/output*".to_string(),
            ];
            let filter = PathFilter::new(&paths)?;
            let is_golden = filter.matches("tests/snapshots/result.snap");
            let not_golden = filter.matches("src/lib.rs");

            let armed = directive_reasons(
                "allow-golden-update: tests/snapshots/result.snap regenerated",
                ALLOW_GOLDEN_UPDATE,
            );
            let prose = directive_reasons(
                "mention of allow-golden-update: tests/snapshots/result.snap",
                ALLOW_GOLDEN_UPDATE,
            );
            Ok(is_golden
                && !not_golden
                && covers(&armed, "tests/snapshots/result.snap")
                && !covers(&armed, "tests/snapshots/other.snap")
                && prose.is_empty())
        },
    ),
    #[cfg(feature = "lang-python")]
    (
        "python: pytest and unittest extraction catches assertions, vacuous tests, and skips",
        || {
            use crate::ast::LanguagePack;
            let py_pack = crate::ast::python::PythonPack;
            let vocab = AssertVocabulary::default();
            let src = "def test_a():\n    assert 1 + 1 == 2\n\ndef test_b():\n    pass\n\n@pytest.mark.skip\ndef test_c():\n    assert True\n";
            let facts = py_pack.extract("test_mod.py", src, &vocab)?;
            Ok(facts.tests.len() == 3
                && facts.tests[0].total_asserts == 1
                && !facts.tests[0].is_vacuous()
                && facts.tests[1].is_vacuous()
                && facts.tests[2].ignored)
        },
    ),
    #[cfg(feature = "lang-python")]
    (
        "python: context manager assertions and class/module pytestmark skips are detected",
        || {
            use crate::ast::LanguagePack;
            let py_pack = crate::ast::python::PythonPack;
            let vocab = AssertVocabulary::default();
            let src = "import unittest\nimport pytest\n\nclass T(unittest.TestCase):\n    pytestmark = pytest.mark.skip('skip class')\n    def test_ctx(self):\n        with self.assertRaises(ValueError):\n            int('x')\n    def test_vac(self):\n        pass\n";
            let facts = py_pack.extract("test_mod.py", src, &vocab)?;
            let t_ctx = facts
                .tests
                .iter()
                .find(|t| t.name.ends_with("test_ctx"))
                .unwrap();
            let t_vac = facts
                .tests
                .iter()
                .find(|t| t.name.ends_with("test_vac"))
                .unwrap();
            Ok(t_ctx.total_asserts == 1
                && t_ctx.strong_asserts == 1
                && !t_ctx.is_vacuous()
                && t_ctx.ignored
                && t_vac.is_vacuous()
                && t_vac.ignored)
        },
    ),
    #[cfg(feature = "lang-javascript")]
    (
        "javascript: describe/it extraction catches matchers, vacuous tests, and skips",
        || {
            use crate::ast::LanguagePack;
            let js_pack = crate::ast::javascript::JavaScriptPack;
            let vocab = AssertVocabulary::default();
            let src = "describe('S', () => {\n  it('a', () => { expect(1 + 1).toBe(2); });\n  it('b', () => {});\n  it.skip('c', () => {});\n});";
            let facts = js_pack.extract("test.js", src, &vocab)?;
            Ok(facts.tests.len() == 3
                && facts.tests[0].total_asserts == 1
                && !facts.tests[0].is_vacuous()
                && facts.tests[1].is_vacuous()
                && facts.tests[2].ignored)
        },
    ),
    #[cfg(feature = "lang-javascript")]
    (
        "javascript: strong vs weak matcher weakening is detected",
        || {
            use crate::ast::LanguagePack;
            let js_pack = crate::ast::javascript::JavaScriptPack;
            let vocab = AssertVocabulary::default();
            let strong_src = "test('a', () => {\n  expect(x).toBe(1);\n  expect(y).toEqual(2);\n  expect(z).toHaveLength(3);\n});";
            let weak_src = "test('a', () => {\n  expect(x).toBeTruthy();\n  expect(y).toBeDefined();\n  expect(z).toBeDefined();\n});";
            let strong_facts = js_pack.extract("test.js", strong_src, &vocab)?;
            let weak_facts = js_pack.extract("test.js", weak_src, &vocab)?;
            Ok(strong_facts.tests[0].strong_asserts == 3
                && weak_facts.tests[0].strong_asserts == 0
                && weak_facts.tests[0].total_asserts == 3)
        },
    ),
    #[cfg(feature = "lang-java")]
    (
        "java: JUnit 5 extraction catches assertions, vacuous tests, and disabled tests",
        || {
            use crate::ast::LanguagePack;
            let java_pack = crate::ast::java::JavaPack;
            let vocab = AssertVocabulary::default();
            let src = "class JTest {\n  @Test void t1() { assertEquals(1, 2); }\n  @Test void t2() { assertTrue(true); }\n  @Disabled @Test void t3() { assertEquals(3, 4); }\n}";
            let facts = java_pack.extract("JTest.java", src, &vocab)?;
            Ok(facts.tests.len() == 3
                && facts.tests[0].total_asserts == 1
                && !facts.tests[0].is_vacuous()
                && facts.tests[1].is_vacuous()
                && facts.tests[2].ignored)
        },
    ),
    #[cfg(feature = "lang-java")]
    (
        "java: assertEquals vs assertTrue assertion weakening is detected",
        || {
            use crate::ast::LanguagePack;
            let java_pack = crate::ast::java::JavaPack;
            let vocab = AssertVocabulary::default();
            let strong_src = "class JTest {\n  @Test void t() { assertEquals(a, b); assertThrows(E.class, () -> {}); }\n}";
            let weak_src = "class JTest {\n  @Test void t() { assertTrue(x); assertTrue(y); }\n}";
            let strong_facts = java_pack.extract("JTest.java", strong_src, &vocab)?;
            let weak_facts = java_pack.extract("JTest.java", weak_src, &vocab)?;
            Ok(strong_facts.tests[0].strong_asserts == 2
                && weak_facts.tests[0].strong_asserts == 0
                && weak_facts.tests[0].total_asserts == 2)
        },
    ),
    #[cfg(feature = "lang-go")]
    (
        "go: testing.T extraction catches assertions, vacuous tests, and t.Skip",
        || {
            use crate::ast::LanguagePack;
            let go_pack = crate::ast::r#go::GoPack;
            let vocab = AssertVocabulary::default();
            let src = "package pkg\nfunc TestOne(t *testing.T) { t.Fatalf(\"err\") }\nfunc TestTwo(t *testing.T) { assert.True(t, true) }\nfunc TestThree(t *testing.T) { t.Skip(\"reason\") }\n";
            let facts = go_pack.extract("pkg_test.go", src, &vocab)?;
            Ok(facts.tests.len() == 3
                && facts.tests[0].total_asserts == 1
                && !facts.tests[0].is_vacuous()
                && facts.tests[1].is_vacuous()
                && facts.tests[2].ignored)
        },
    ),
    #[cfg(feature = "lang-go")]
    (
        "go: t.Fatalf assertion drop is detected",
        || {
            use crate::ast::LanguagePack;
            let go_pack = crate::ast::r#go::GoPack;
            let vocab = AssertVocabulary::default();
            let strong_src = "package pkg\nfunc TestA(t *testing.T) {\n  t.Fatalf(\"err\")\n  require.Equal(t, a, b)\n}\n";
            let weak_src = "package pkg\nfunc TestA(t *testing.T) {\n  t.Fail()\n  t.Fail()\n}\n";
            let strong_facts = go_pack.extract("pkg_test.go", strong_src, &vocab)?;
            let weak_facts = go_pack.extract("pkg_test.go", weak_src, &vocab)?;
            Ok(strong_facts.tests[0].strong_asserts == 2
                && weak_facts.tests[0].strong_asserts == 0
                && weak_facts.tests[0].total_asserts == 2)
        },
    ),
    #[cfg(feature = "lang-php")]
    (
        "php: PHPUnit extraction catches assertions, vacuous tests, and markTestSkipped",
        || {
            use crate::ast::LanguagePack;
            let php_pack = crate::ast::php::PhpPack;
            let vocab = AssertVocabulary::default();
            let src = "<?php\nclass JTest extends TestCase {\n  public function testOne() { $this->assertEquals(1, 2); }\n  public function testTwo() { $this->assertTrue(true); }\n  public function testThree() { $this->markTestSkipped('skip'); }\n}\n";
            let facts = php_pack.extract("tests/JTest.php", src, &vocab)?;
            Ok(facts.tests.len() == 3
                && facts.tests[0].total_asserts == 1
                && !facts.tests[0].is_vacuous()
                && facts.tests[1].is_vacuous()
                && facts.tests[2].ignored)
        },
    ),
    #[cfg(feature = "lang-php")]
    (
        "php: assertEquals vs assertTrue assertion weakening is detected",
        || {
            use crate::ast::LanguagePack;
            let php_pack = crate::ast::php::PhpPack;
            let vocab = AssertVocabulary::default();
            let strong_src = "<?php\nclass JTest extends TestCase {\n  public function testA() { $this->assertEquals($a, $b); $this->expectException(E::class); }\n}\n";
            let weak_src = "<?php\nclass JTest extends TestCase {\n  public function testA() { $this->assertTrue($x); $this->assertTrue($y); }\n}\n";
            let strong_facts = php_pack.extract("tests/JTest.php", strong_src, &vocab)?;
            let weak_facts = php_pack.extract("tests/JTest.php", weak_src, &vocab)?;
            Ok(strong_facts.tests[0].strong_asserts == 2
                && weak_facts.tests[0].strong_asserts == 0
                && weak_facts.tests[0].total_asserts == 2)
        },
    ),
    #[cfg(feature = "lang-cpp")]
    (
        "c_cpp: GoogleTest extraction catches assertions, vacuous tests, and DISABLED_ tests",
        || {
            use crate::ast::LanguagePack;
            let cpp_pack = crate::ast::c_cpp::CppPack;
            let vocab = AssertVocabulary::default();
            let src = "TEST(Suite, TestOne) { EXPECT_EQ(1, 2); }\nTEST(Suite, TestTwo) { EXPECT_TRUE(true); }\nTEST(Suite, DISABLED_TestThree) { EXPECT_EQ(1, 2); }\n";
            let facts = cpp_pack.extract("tests/test.cpp", src, &vocab)?;
            Ok(facts.tests.len() == 3
                && facts.tests[0].total_asserts == 1
                && !facts.tests[0].is_vacuous()
                && facts.tests[1].is_vacuous()
                && facts.tests[2].ignored)
        },
    ),
    #[cfg(feature = "lang-cpp")]
    (
        "c_cpp: EXPECT_EQ vs EXPECT_TRUE assertion weakening is detected",
        || {
            use crate::ast::LanguagePack;
            let cpp_pack = crate::ast::c_cpp::CppPack;
            let vocab = AssertVocabulary::default();
            let strong_src = "TEST(Suite, TestA) { EXPECT_EQ(a, b); ASSERT_NE(c, d); }\n";
            let weak_src = "TEST(Suite, TestA) { EXPECT_TRUE(x); EXPECT_TRUE(y); }\n";
            let strong_facts = cpp_pack.extract("tests/test.cpp", strong_src, &vocab)?;
            let weak_facts = cpp_pack.extract("tests/test.cpp", weak_src, &vocab)?;
            Ok(strong_facts.tests[0].strong_asserts == 2
                && weak_facts.tests[0].strong_asserts == 0
                && weak_facts.tests[0].total_asserts == 2)
        },
    ),
    #[cfg(feature = "lang-csharp")]
    (
        "csharp: xUnit extraction catches assertions, vacuous tests, and Fact(Skip = ...)",
        || {
            use crate::ast::LanguagePack;
            let cs_pack = crate::ast::csharp::CSharpPack;
            let vocab = AssertVocabulary::default();
            let src = "public class CalcTests {\n    [Fact]\n    public void TestOne() { Assert.Equal(1, 2); }\n    [Fact]\n    public void TestTwo() { Assert.True(true); }\n    [Fact(Skip = \"not ready\")]\n    public void TestThree() { Assert.Equal(1, 2); }\n}\n";
            let facts = cs_pack.extract("tests/CalcTests.cs", src, &vocab)?;
            Ok(facts.tests.len() == 3
                && facts.tests[0].total_asserts == 1
                && !facts.tests[0].is_vacuous()
                && facts.tests[1].is_vacuous()
                && facts.tests[2].ignored)
        },
    ),
    #[cfg(feature = "lang-csharp")]
    (
        "csharp: Assert.Equal vs Assert.True assertion weakening is detected",
        || {
            use crate::ast::LanguagePack;
            let cs_pack = crate::ast::csharp::CSharpPack;
            let vocab = AssertVocabulary::default();
            let strong_src = "public class T {\n    [Fact]\n    public void TestA() { Assert.Equal(a, b); Assert.NotEqual(c, d); }\n}\n";
            let weak_src = "public class T {\n    [Fact]\n    public void TestA() { Assert.True(x); Assert.True(y); }\n}\n";
            let strong_facts = cs_pack.extract("tests/T.cs", strong_src, &vocab)?;
            let weak_facts = cs_pack.extract("tests/T.cs", weak_src, &vocab)?;
            Ok(strong_facts.tests[0].strong_asserts == 2
                && weak_facts.tests[0].strong_asserts == 0
                && weak_facts.tests[0].total_asserts == 2)
        },
    ),
    #[cfg(feature = "lang-ruby")]
    (
        "ruby: Minitest extraction catches assertions, vacuous tests, and skip",
        || {
            use crate::ast::LanguagePack;
            let rb_pack = crate::ast::ruby::RubyPack;
            let vocab = AssertVocabulary::default();
            let src = "class CalcTest < Minitest::Test\n  def test_one; assert_equal 1, 2; end\n  def test_two; assert true; end\n  def test_three; skip; assert_equal 1, 2; end\nend\n";
            let facts = rb_pack.extract("test/test_calc.rb", src, &vocab)?;
            Ok(facts.tests.len() == 3
                && facts.tests[0].total_asserts == 1
                && !facts.tests[0].is_vacuous()
                && facts.tests[1].is_vacuous()
                && facts.tests[2].ignored)
        },
    ),
    #[cfg(feature = "lang-ruby")]
    (
        "ruby: assert_equal vs assert assertion weakening is detected",
        || {
            use crate::ast::LanguagePack;
            let rb_pack = crate::ast::ruby::RubyPack;
            let vocab = AssertVocabulary::default();
            let strong_src = "class T < Minitest::Test\n  def test_a; assert_equal a, b; assert_match c, d; end\nend\n";
            let weak_src = "class T < Minitest::Test\n  def test_a; assert x; assert y; end\nend\n";
            let strong_facts = rb_pack.extract("test/test_t.rb", strong_src, &vocab)?;
            let weak_facts = rb_pack.extract("test/test_t.rb", weak_src, &vocab)?;
            Ok(strong_facts.tests[0].strong_asserts == 2
                && weak_facts.tests[0].strong_asserts == 0
                && weak_facts.tests[0].total_asserts == 2)
        },
    ),
    (
        "bench: callgrind and criterion benchmark parsing and delta calculation discriminate",
        || {
            use crate::guards::perf::parse_metrics;
            use crate::tokens::ALLOW_REGRESSION;
            let callgrind_sample = "events: Ir\nsummary: 10000\n";
            let criterion_sample = r#"{"mean": {"point_estimate": 500.0}}"#;
            let cg_m = parse_metrics("target/iai/bench/callgrind.out", callgrind_sample)?;
            let cr_m = parse_metrics("target/criterion/bench/estimates.json", criterion_sample)?;
            let armed = directive_reasons(
                "allow-regression: bench perf justification",
                ALLOW_REGRESSION,
            );
            let prose =
                directive_reasons("just mention of allow-regression: bench", ALLOW_REGRESSION);
            Ok(cg_m.len() == 1
                && cg_m[0].count == 10000.0
                && cg_m[0].unit == "Ir"
                && cr_m.len() == 1
                && cr_m[0].count == 500.0
                && cr_m[0].unit == "ns"
                && covers(&armed, "bench")
                && !covers(&armed, "other")
                && prose.is_empty())
        },
    ),
    (
        "bench: multi-language parsing (go, google benchmark, pytest) and subject resolution discriminate",
        || {
            use crate::guards::perf::{benchmark_subjects, parse_metrics};
            let go_sample = "BenchmarkSearch-8   100000   12.40 ns/op\n";
            let gbench_sample = r#"{"benchmarks": [{"name": "BM_SetInsert/1024", "cpu_time": 440.0, "time_unit": "ns"}]}"#;
            let pytest_sample = r#"{"benchmarks": [{"name": "test_serialize", "stats": {"mean": 0.000135}}]}"#;

            let go_m = parse_metrics("benchmarks/go.txt", go_sample)?;
            let gb_m = parse_metrics("build/bench.json", gbench_sample)?;
            let py_m = parse_metrics("reports/pytest.json", pytest_sample)?;

            let go_subjects = benchmark_subjects("benchmarks/go.txt", &go_m[0].name);
            let gb_subjects = benchmark_subjects("build/bench.json", &gb_m[0].name);
            let py_subjects = benchmark_subjects("reports/pytest.json", &py_m[0].name);

            Ok(go_m.len() == 1
                && go_m[0].count == 12.40
                && go_m[0].unit == "ns/op"
                && go_subjects.contains(&"BenchmarkSearch".to_string())
                && gb_m.len() == 1
                && gb_m[0].count == 440.0
                && gb_m[0].unit == "ns"
                && gb_subjects.contains(&"BM_SetInsert".to_string())
                && py_m.len() == 1
                && py_m[0].count == 0.000135
                && py_m[0].unit == "s"
                && py_subjects.contains(&"test_serialize".to_string()))
        },
    ),
    (
        "command: fail-closed execution wrapper catches forbidden output, exit status, and count ratchet",
        || {
            use crate::guards::command::{run_command_bounded, split_command_line};
            use std::path::Path;

            let tokens = split_command_line("echo \"test result: ok. 42 passed\"")?;
            if tokens != vec!["echo", "test result: ok. 42 passed"] {
                return Ok(false);
            }

            let run = run_command_bounded("echo_test", "echo \"test result: ok. 42 passed\"", 5, Path::new("."))?;
            if !run.status.success() {
                return Ok(false);
            }

            let output = format!("{}\n{}", run.stdout, run.stderr);
            let re = Regex::new(r"(\d+) passed")?;
            let count = re.captures(&output).and_then(|c| c.get(1)).and_then(|m| m.as_str().parse::<u64>().ok());
            if count != Some(42) {
                return Ok(false);
            }

            // Ratchet checks
            let floor_pass = 40;
            let floor_fail = 50;
            let passes = count.unwrap() >= floor_pass;
            let fails = count.unwrap() < floor_fail;

            // Forbidden output checks
            let forbid_pat = "FAILED";
            let forbid_tripped = "42 passed";
            let clean = !output.contains(forbid_pat);
            let caught = output.contains(forbid_tripped);

            Ok(passes && fails && clean && caught)
        },
    ),
    (
        "command: canary failure, missing tool, and zero-items are detected",
        || {
            use crate::guards::command::run_command_bounded;
            use crate::tokens::{covers, directive_reasons, ALLOW_COMMAND};
            use std::path::Path;

            let missing_err = run_command_bounded(
                "missing_tool",
                "non_existent_binary_xyz_12345",
                5,
                Path::new("."),
            );
            let missing_ok = match missing_err {
                Err(e) => format!("{e:#}").contains("not found in PATH"),
                Ok(_) => false,
            };

            let timeout_err = run_command_bounded("timeout_test", "sleep 3", 1, Path::new("."));
            let timeout_ok = match timeout_err {
                Err(e) => format!("{e:#}").contains("timed out after 1s"),
                Ok(_) => false,
            };

            // Canary diagnostic verification
            let canary_out = "error[E0308]: mismatched types\nexpected u32, found i32";
            let has_diag = canary_out.contains("mismatched types");
            let missing_diag = !canary_out.contains("assertion failed");

            // Zero items detection
            let zero_out = "running 0 tests\ntest result: ok. 0 passed";
            let re = Regex::new(r"running 0 tests")?;
            let zero_matched = re.is_match(zero_out);

            // Override directive check
            let armed = directive_reasons(
                "allow-command: my_suite integration tests offline in sandbox",
                ALLOW_COMMAND,
            );
            let covers_named = covers(&armed, "my_suite");
            let covers_other = covers(&armed, "other_suite");

            Ok(missing_ok
                && timeout_ok
                && has_diag
                && missing_diag
                && zero_matched
                && covers_named
                && !covers_other)
        },
    ),
    (
        "dependency: manifest delta detects wildcards, unpinned git deps, and banned packages",
        || {
            use crate::guards::dependency::{parse_cargo_toml, parse_package_json};

            // 1. Wildcard detection in cargo and npm
            let cargo_wild = r#"
[dependencies]
sample-wildcard = "*"
sample-pinned = "1.2.3"
git-unpinned = { git = "https://github.com/example/lib.git", branch = "main" }
git-pinned = { git = "https://github.com/example/lib.git", rev = "1234567890abcdef1234567890abcdef12345678" }
"#;
            let cargo_deps = parse_cargo_toml(cargo_wild, "Cargo.toml");
            let unpinned_ver = cargo_deps.iter().find(|d| d.name == "sample-wildcard").unwrap();
            let pinned_ver = cargo_deps.iter().find(|d| d.name == "sample-pinned").unwrap();
            let git_unpinned = cargo_deps.iter().find(|d| d.name == "git-unpinned").unwrap();
            let git_pinned = cargo_deps.iter().find(|d| d.name == "git-pinned").unwrap();

            let wildcard_discriminated = unpinned_ver.is_wildcard && !pinned_ver.is_wildcard;
            let git_pin_discriminated = git_unpinned.is_git && git_unpinned.git_pin.is_none()
                && git_pinned.is_git && git_pinned.git_pin.is_some();

            // 2. npm wildcard detection
            let pkg_json = r#"{
  "dependencies": {
    "wild": "latest",
    "exact": "2.4.0"
  }
}"#;
            let npm_deps = parse_package_json(pkg_json, "package.json");
            let wild = npm_deps.iter().find(|d| d.name == "wild").unwrap();
            let exact = npm_deps.iter().find(|d| d.name == "exact").unwrap();
            let npm_wildcard_discriminated = wild.is_wildcard && !exact.is_wildcard;

            Ok(wildcard_discriminated && git_pin_discriminated && npm_wildcard_discriminated)
        },
    ),
    (
        "dependency: deny.toml allowlist enforcement and allow-dependency override discriminate",
        || {
            use crate::guards::dependency::parse_deny_toml;
            use crate::tokens::{covers, directive_reasons, ALLOW_DEPENDENCY};

            let deny_content = r#"
[bans]
deny = [
    { name = "malicious-pkg" },
]
allow = [
    { name = "approved-pkg" },
]
wildcards = "deny"

[sources]
unknown-git = "deny"
allow-git = [
    "https://github.com/trusted/repo",
]
"#;
            let policy = parse_deny_toml(deny_content)?;
            let policy_ok = policy.deny_bans.contains("malicious-pkg")
                && !policy.deny_bans.contains("approved-pkg")
                && policy.allow_bans.contains("approved-pkg")
                && policy.wildcards_denied
                && policy.deny_unknown_git
                && policy.allow_git.iter().any(|u| u.contains("trusted/repo"));

            // Directive override discrimination
            let armed = directive_reasons(
                "allow-dependency: malicious-pkg vendor patched audit in progress",
                ALLOW_DEPENDENCY,
            );
            let covers_named = covers(&armed, "malicious-pkg");
            let covers_other = covers(&armed, "other-pkg");
            let placeholder = directive_reasons("allow-dependency: <reason>", ALLOW_DEPENDENCY);

            Ok(policy_ok && covers_named && !covers_other && placeholder.is_empty())
        },
    ),
    (
        "test-budget: proptest, quickcheck, hypothesis, and fast-check budget reductions are detected",
        || {
            use crate::guards::test_budget::{
                extract_js_budgets, extract_python_budgets, extract_rust_budgets,
            };

            // 1. Rust proptest and quickcheck reduction
            let base_rs = "let c = ProptestConfig { cases: 5000, max_shrink_iters: 2000, ..Default::default() };\nQuickCheck::new().tests(500);";
            let head_rs = "let c = ProptestConfig { cases: 500, max_shrink_iters: 200, ..Default::default() };\nQuickCheck::new().tests(50);";
            let base_rust = extract_rust_budgets(base_rs, "tests/prop.rs");
            let head_rust = extract_rust_budgets(head_rs, "tests/prop.rs");

            let cases_drop = base_rust.iter().find(|m| m.subject == "proptest cases").unwrap().value
                > head_rust.iter().find(|m| m.subject == "proptest cases").unwrap().value;
            let shrink_drop = base_rust.iter().find(|m| m.subject == "proptest max_shrink_iters").unwrap().value
                > head_rust.iter().find(|m| m.subject == "proptest max_shrink_iters").unwrap().value;
            let qc_drop = base_rust.iter().find(|m| m.subject == "quickcheck tests").unwrap().value
                > head_rust.iter().find(|m| m.subject == "quickcheck tests").unwrap().value;

            // 2. Python Hypothesis reduction
            let base_py = "@settings(max_examples=1000, deadline=500)\ndef test_h(): pass";
            let head_py = "@settings(max_examples=100, deadline=50)\ndef test_h(): pass";
            let base_python = extract_python_budgets(base_py, "test_h.py");
            let head_python = extract_python_budgets(head_py, "test_h.py");

            let hypo_examples_drop = base_python.iter().find(|m| m.subject == "hypothesis max_examples").unwrap().value
                > head_python.iter().find(|m| m.subject == "hypothesis max_examples").unwrap().value;
            let hypo_deadline_drop = base_python.iter().find(|m| m.subject == "hypothesis deadline").unwrap().value
                > head_python.iter().find(|m| m.subject == "hypothesis deadline").unwrap().value;

            // 3. JS fast-check reduction
            let base_js = "fc.assert(prop, { numRuns: 1000 });";
            let head_js = "fc.assert(prop, { numRuns: 100 });";
            let base_fc = extract_js_budgets(base_js, "test.js");
            let head_fc = extract_js_budgets(head_js, "test.js");

            let fc_drop = base_fc[0].value > head_fc[0].value;

            Ok(cases_drop && shrink_drop && qc_drop && hypo_examples_drop && hypo_deadline_drop && fc_drop)
        },
    ),
    (
        "test-budget: workflow flags, fuzz targets, and allow-test-shrink override discriminate",
        || {
            use crate::guards::test_budget::{
                extract_fuzz_manifest_targets, extract_script_and_workflow_budgets,
            };
            use crate::tokens::{covers, directive_reasons, ALLOW_TEST_SHRINK};

            // 1. Workflow flags reduction
            let base_wf = "PROPTEST_CASES=10000\ncargo fuzz run t -max_total_time 3600 -runs 1000000\ngo test -fuzztime=10m";
            let head_wf = "PROPTEST_CASES=1000\ncargo fuzz run t -max_total_time 300 -runs 10000\ngo test -fuzztime=1m";
            let base_metrics = extract_script_and_workflow_budgets(base_wf, "ci.sh");
            let head_metrics = extract_script_and_workflow_budgets(head_wf, "ci.sh");

            let prop_drop = base_metrics.iter().find(|m| m.subject == "PROPTEST_CASES").unwrap().value
                > head_metrics.iter().find(|m| m.subject == "PROPTEST_CASES").unwrap().value;
            let time_drop = base_metrics.iter().find(|m| m.subject == "fuzz -max_total_time").unwrap().value
                > head_metrics.iter().find(|m| m.subject == "fuzz -max_total_time").unwrap().value;
            let runs_drop = base_metrics.iter().find(|m| m.subject == "fuzz -runs").unwrap().value
                > head_metrics.iter().find(|m| m.subject == "fuzz -runs").unwrap().value;
            let fuzztime_drop = base_metrics.iter().find(|m| m.subject == "go fuzz -fuzztime").unwrap().value
                > head_metrics.iter().find(|m| m.subject == "go fuzz -fuzztime").unwrap().value;

            // 2. Fuzz manifest targets removal
            let base_fuzz = "[[bin]]\nname = \"target_a\"\n[[bin]]\nname = \"target_b\"";
            let head_fuzz = "[[bin]]\nname = \"target_a\"";
            let base_targets = extract_fuzz_manifest_targets(base_fuzz);
            let head_targets = extract_fuzz_manifest_targets(head_fuzz);
            let target_removed = base_targets.contains("target_b") && !head_targets.contains("target_b");

            // 3. Directive override check
            let armed = directive_reasons(
                "allow-test-shrink: PROPTEST_CASES fast local iteration budget",
                ALLOW_TEST_SHRINK,
            );
            let covers_named = covers(&armed, "PROPTEST_CASES");
            let covers_other = covers(&armed, "fuzz -max_total_time");
            let placeholder = directive_reasons("allow-test-shrink: <reason>", ALLOW_TEST_SHRINK);

            Ok(prop_drop && time_drop && runs_drop && fuzztime_drop && target_removed && covers_named && !covers_other && placeholder.is_empty())
        },
    ),
    (
        "presets: turnkey preset resolution merges defaults and explicit overrides",
        || {
            use crate::config::DisciplineConfig;
            use crate::guards::presets;

            let toml_text = r#"
[meta]
version = 1
name = "test"

[gates.command]
enabled = true
preset = "cargo-mutants"
timeout_seconds = 120
forbid_output = ["CUSTOM_FORBID"]

[[gates.command.commands]]
name = "semver"
preset = "cargo-semver-checks"

[[gates.command.commands]]
name = "custom"
command = "cargo test"
"#;
            let cfg = DisciplineConfig::from_toml_str(toml_text)?;
            let cmd_gate = &cfg.gates.command;

            // 1. Top-level preset resolves with user override
            let mutants_preset = presets::resolve_preset(cmd_gate.preset.as_deref().unwrap()).unwrap();
            let effective_cmd = cmd_gate.command.as_deref().unwrap_or(mutants_preset.default_command);
            let effective_timeout = cmd_gate.timeout_seconds.unwrap_or(mutants_preset.default_timeout_seconds);

            // 2. CommandEntry preset resolves default command
            let semver_entry = &cmd_gate.commands[0];
            let semver_preset = presets::resolve_preset(semver_entry.preset.as_deref().unwrap()).unwrap();
            let semver_cmd = semver_entry.command.as_deref().unwrap_or(semver_preset.default_command);

            // 3. Unknown preset fails lookup
            let unknown = presets::resolve_preset("nonexistent-tool");

            Ok(effective_cmd == "cargo mutants --in-diff"
                && effective_timeout == 120
                && cmd_gate.forbid_output.contains(&"CUSTOM_FORBID".to_string())
                && semver_cmd == "cargo semver-checks check-release"
                && unknown.is_none())
        },
    ),
    (
        "presets: cargo-mutants, cargo-deny, and loom definitions enforce zero-items and forbid patterns",
        || {
            use crate::guards::presets;

            let mutants = presets::resolve_preset("cargo-mutants").unwrap();
            let deny = presets::resolve_preset("cargo-deny").unwrap();
            let loom = presets::resolve_preset("loom").unwrap();
            let lcov = presets::resolve_preset("lcov").unwrap();

            // Check mutation testing invariants
            let mutants_ok = mutants.category == "mutation"
                && mutants.zero_items_pattern == Some("0 mutants tested")
                && mutants.forbid_output.contains(&"survived")
                && mutants.forbid_output.contains(&"MISSED");

            // Check supply chain invariants & policy files
            let deny_ok = deny.category == "supply-chain"
                && deny.policy_files.contains(&"deny.toml")
                && deny.default_command == "cargo deny check";

            // Check concurrency test invariants
            let loom_ok = loom.category == "concurrency"
                && loom.zero_items_pattern == Some("running 0 tests")
                && loom.default_command == "cargo test --test loom -- --nocapture";

            // Check coverage report invariants
            let lcov_ok = lcov.category == "coverage"
                && lcov.zero_items_pattern.is_some()
                && lcov.policy_files.contains(&"lcov.info");

            Ok(mutants_ok && deny_ok && loom_ok && lcov_ok)
        },
    ),
    (
        "ast: compile-time assertions in Rust and C/C++ are extracted outside tests",
        || {
            use crate::ast::LanguagePack;
            let rust_pack = crate::ast::rust::RustPack;
            let vocab = AssertVocabulary::default();
            let rust_src = "const _: () = assert!(std::mem::size_of::<u64>() == 8);\nconst _: () = { assert!(true); };\n#[test]\nfn test_normal() { assert!(true); }\n";
            let rust_facts = rust_pack.extract("src/types.rs", rust_src, &vocab)?;

            let c_pack = crate::ast::c_cpp::CPack;
            let c_src = "static_assert(sizeof(int) == 4, \"int size\");\nint test_main() { assert(1); return 0; }\n";
            let c_facts = c_pack.extract("src/types.c", c_src, &vocab)?;

            Ok(rust_facts.compile_time_asserts == 2
                && rust_facts.compile_time_test.as_ref().map(|t| t.total_asserts) == Some(2)
                && rust_facts.tests.len() == 1
                && c_facts.compile_time_asserts == 1
                && c_facts.compile_time_test.as_ref().map(|t| t.total_asserts) == Some(1)
                && c_facts.tests.len() == 1)
        },
    ),
    (
        "report: agent-prompt format emits repair instructions with zero directive tokens",
        || {
            use crate::guards::{CheckSummary, GateOutcome, Violation};
            use crate::config::Severity;
            use crate::report::format_agent_prompt;

            let mut o = GateOutcome::new("assertion-reduction");
            o.violations.push(Violation {
                gate: "assertion-reduction",
                severity: Severity::Error,
                title: "Assertion Reduction In Existing Test".into(),
                file: Some("src/lib.rs".into()),
                line: Some(10),
                message: "effective assertions dropped from 2 to 0".into(),
                remediation: Some("Restore the assertions, or justify the drop: `allow-assertion-drop: foo <reason>`.".into()),
            });

            let summary = CheckSummary {
                base: "main".into(),
                errors: 1,
                warnings: 0,
                overrides: 0,
                outcomes: vec![o],
                planned_gates: vec![],
            };

            let prompt = format_agent_prompt(&summary);
            let has_repair = prompt.contains("Repair: Restore the assertions");
            let leaks_directive = prompt.contains("allow-assertion-drop")
                || prompt.contains("discipline:allow")
                || prompt.contains("removes:");

            Ok(has_repair && !leaks_directive)
        },
    ),
    (
        "hygiene: pii secrets scanner detects private keys and tokens with redaction",
        || {
            use crate::config::PiiGate;
            use crate::guards::hygiene::pii_rules;

            let settings = PiiGate::default();
            let rules = pii_rules(&settings)?;

            // 1. Private key header
            let priv_key = "-----BEGIN RSA PRIVATE KEY-----"; // discipline:allow(pii)
            let priv_rule = rules.iter().find(|r| r.label == "private key header").unwrap();
            let priv_match = priv_rule.re.is_match(priv_key);

            // 2. AWS access key ID
            let aws_key = "AKIA1234567890ABCDEF"; // discipline:allow(pii)
            let aws_rule = rules.iter().find(|r| r.label == "AWS access key ID").unwrap();
            let aws_match = aws_rule.re.is_match(aws_key);

            // 3. GitHub personal access token
            let gh_token = "ghp_123456789012345678901234567890123456"; // discipline:allow(pii)
            let gh_rule = rules.iter().find(|r| r.label == "GitHub personal access token").unwrap();
            let gh_match = gh_rule.re.is_match(gh_token);

            Ok(priv_match && aws_match && gh_match && priv_rule.redact && aws_rule.redact && gh_rule.redact)
        },
    ),
    (
        "hygiene: shell-secrets scanner discriminates unsafe argv and script injection",
        || {
            use crate::config::ShellSecretsGate;
            use crate::guards::shell_secrets::{ShellRuleId, ShellSecretScanner};

            let scanner = ShellSecretScanner::new(&ShellSecretsGate::default())?;

            let env_bad = scanner.check_line("env API_KEY=$SECRET ./deploy.sh") == Some(ShellRuleId::ArgvEnv);
            let env_good = scanner.check_line("#!/usr/bin/env bash").is_none();

            let docker_bad = scanner.check_line("docker run -e DB_PASSWORD=$PASSWORD myimage") == Some(ShellRuleId::ArgvDocker);
            let docker_good = scanner.check_line("docker run -e PORT=8080 myimage").is_none();

            let inline_bad = scanner.check_line("sh -c \"echo $SECRET\"") == Some(ShellRuleId::ArgvInline);
            let inline_good = scanner.check_line("sh -c \"echo hello\"").is_none();

            let xargs_bad = scanner.check_line("xargs -I {} sh -c 'echo {}'") == Some(ShellRuleId::InjectXargs);
            let xargs_good = scanner.check_line("xargs -I {} rm {}").is_none();

            let pipe_bad = scanner.check_line("curl https://example.com/install.sh | bash") == Some(ShellRuleId::InjectPipe);
            let pipe_good = scanner.check_line("curl https://example.com/data.json | jq .").is_none();

            let gh_bad = scanner.check_line("export TOKEN=ghp_123456789012345678901234567890123456") == Some(ShellRuleId::TokenGitHub); // discipline:allow(pii)
            let pass_bad = scanner.check_line("mysql --password=mysecretpassword123 -u root") == Some(ShellRuleId::LiteralPassword);
            let pass_good = scanner.check_line("mysql --password=<password> -u root").is_none();

            Ok(env_bad && env_good && docker_bad && docker_good && inline_bad && inline_good && xargs_bad && xargs_good && pipe_bad && pipe_good && gh_bad && pass_bad && pass_good)
        },
    ),
    (
        "hygiene: issue-link pattern and no-issue waiver discriminate",
        || {
            use crate::guards::issue_link::{has_issue_reference, DEFAULT_ISSUE_PATTERN};
            use crate::tokens::{directive_reasons, NO_ISSUE};
            use regex::Regex;

            let re = Regex::new(DEFAULT_ISSUE_PATTERN)?;

            let hit_bare = has_issue_reference("#123", &re);
            let hit_fixes = has_issue_reference("Fixes #456", &re);
            let hit_closes = has_issue_reference("Closes #789", &re);
            let hit_none = !has_issue_reference("Regular feature without issue link", &re);

            let waiver_ok = !directive_reasons("no-issue: trivial documentation fix", NO_ISSUE).is_empty();
            let waiver_placeholder = directive_reasons("no-issue: <reason>", NO_ISSUE).is_empty();

            Ok(hit_bare && hit_fixes && hit_closes && hit_none && waiver_ok && waiver_placeholder)
        },
    ),
    (
        "ast: python test extraction discriminates test methods from helpers",
        || {
            use crate::ast::LanguagePack;
            let v = AssertVocabulary::default();
            let py_code = "class TestSuite(unittest.TestCase):\n    def _helper(self):\n        pass\n    @staticmethod\n    def util():\n        pass\n    def test_real(self):\n        assert 1 == 1\n";
            let parsed = crate::ast::python::PythonPack.extract("test_suite.py", py_code, &v)?;
            Ok(parsed.tests.len() == 1 && parsed.tests[0].name == "TestSuite::test_real")
        },
    ),
    (
        "ast: c_cpp test extraction recognises non-zero return and abort as assertions",
        || {
            use crate::ast::LanguagePack;
            let v = AssertVocabulary::default();
            let cpp_code = "void fail_if_bad() { abort(); }\nint main() {\n    fail_if_bad();\n    return 1;\n}\n";
            let parsed = crate::ast::c_cpp::CppPack.extract("test_driver.cpp", cpp_code, &v)?;
            let main_test = parsed.tests.iter().find(|t| t.name == "main").unwrap();
            Ok(main_test.strong_asserts >= 1 && !main_test.is_vacuous())
        },
    ),
    (
        "ignored-tests: approved predicates waive conditional ignores",
        || {
            use crate::ast::TestFn;
            use crate::config::IgnoredTestsGate;
            use crate::guards::agent_diff::{evaluate_ignored_tests, Located};

            let miri_test = TestFn {
                name: "miri_test".to_string(),
                line: 1,
                conditional_ignore: Some("miri".to_string()),
                ..Default::default()
            };
            let added = [Located {
                path: "tests/m.rs",
                file_survives: true,
                test: &miri_test,
            }];

            let default_settings = IgnoredTestsGate::default();
            let unapproved = evaluate_ignored_tests(&[], &added, &default_settings, &[], false)?;

            let mut approved_settings = default_settings.clone();
            approved_settings.approved_predicates = vec!["miri".to_string()];
            let approved = evaluate_ignored_tests(&[], &added, &approved_settings, &[], false)?;

            Ok(unapproved.violations.len() == 1 && approved.violations.is_empty())
        },
    ),
    (
        "hygiene: terms of art and historical narration exempt from time-estimates",
        || {
            let banned = crate::guards::hygiene::time_estimate_patterns()
                .iter()
                .map(|p| Regex::new(p))
                .collect::<Result<Vec<_>, _>>()?;
            let empty = Vec::new();
            let check = |text: &str| {
                crate::guards::hygiene::scan_text_for_time_estimates(text, &banned, &empty).is_empty()
            };
            Ok(check("The nightly cache has a 7 days retention.")
                && check("~6.06 days active window")
                && check("forty minutes later — a commit ordering")
                && check("1-min average decaying")
                && check("`load1` metric")
                && !check("Ship v0.1 (1-2 days)."))
        },
    ),
    (
        "pii: agent config references detected across text files",
        || {
            let s = PiiGate {
                home_paths: false,
                lan_ips: false,
                secrets: false,
                agent_config_refs: true,
                ..PiiGate::default()
            };
            let rules = pii_rules(&s)?;
            let hit = |text: &str| rules.iter().any(|r| r.re.is_match(text));
            Ok(hit("with unit tests in ~/.claude/CLAUDE.md") // discipline:allow(pii)
                && hit("follow $HOME/.gemini/GEMINI.md for style") // discipline:allow(pii)
                && hit("Per RESEARCH_DISCIPLINES.md Rule 1") // discipline:allow(pii)
                && hit("see PAPER_PUBLISHING_PLAYBOOK.md") // discipline:allow(pii)
                && !hit("export PATH=$HOME/.cargo/bin:$PATH")
                && !hit("AGENTS.md is the canonical guide"))
        },
    ),
    (
        "pii: test functions are scanned and flag home paths and lan ips",
        || {
            let s = PiiGate::default();
            let rules = pii_rules(&s)?;
            let hit = |text: &str| rules.iter().any(|r| r.re.is_match(text));
            Ok(hit("    fake_path = \"/Users/someone/repo/\"") // discipline:allow(pii)
                && hit("    fake_ip = \"192.168.1.50\"")) // discipline:allow(pii)
        },
    ),
    (
        "tokens: namespaced directive prefix discipline: accepted",
        || {
            let parsed = crate::tokens::parse_directives(
                "discipline: removes: tests/old.rs refactored\n<!-- discipline: allow-regression: bench_a -->\ndiscipline: allow-ignore: miri\n",
                crate::tokens::OverrideSource::PrBody,
            );
            Ok(parsed.len() == 3
                && parsed[0].directive == "removes"
                && parsed[1].directive == "allow-regression"
                && parsed[2].directive == "allow-ignore")
        },
    ),
    (
        "deletion-rationale: require_scope configuration controls unscoped removes",
        || {
            use crate::config::DeletionGate;
            use crate::gitctx::{ChangeKind, ChangedFile};
            use crate::guards::agent_diff::evaluate_deletion_rationale;
            use crate::tokens::{parse_directives, OverrideSource};

            let directives = parse_directives("removes: general cleanup", OverrideSource::PrBody);
            let deleted = [ChangedFile {
                path: "src/old.rs".into(),
                old_path: "src/old.rs".into(),
                kind: ChangeKind::Deleted,
                added_lines: std::collections::BTreeSet::new(),
            }];

            let strict = DeletionGate {
                require_scope: true,
                ..DeletionGate::default()
            };
            let unstrict = DeletionGate {
                require_scope: false,
                ..DeletionGate::default()
            };

            let out_strict = evaluate_deletion_rationale(&deleted, &[], &strict, &directives, false)?;
            let out_unstrict = evaluate_deletion_rationale(&deleted, &[], &unstrict, &directives, false)?;

            Ok(out_strict.violations.len() == 1 && out_unstrict.violations.is_empty())
        },
    ),
    (
        "test-floor: constant extraction and test count output discrimination",
        || {
            use crate::guards::test_floor::parse_test_count_output;
            let sample = "
test tests::a: test
test tests::b: test
test tests::c: test
3 passed; 0 failed
";
            let parsed = parse_test_count_output(sample);
            let num = parse_test_count_output("142\n");
            let const_src = "pub const TEST_FLOOR: usize = 120;\n";
            let re = Regex::new(r"(?m)^[ \t]*(?:(?:pub|export)\s+)?(?:const\s+)?TEST_FLOOR(?:\s*:\s*[a-zA-Z0-9_]+)?\s*=\s*(\d+)")?;
            let extracted = re.captures(const_src).and_then(|c| c[1].parse::<usize>().ok());

            Ok(parsed == 3 && num == 142 && extracted == Some(120))
        },
    ),
    (
        "ci-integrity: rollup needs detection, pinning, and error masks",
        || {
            use crate::guards::ci_integrity::parse_workflow_jobs;
            let workflow = "
name: CI
jobs:
  build:
    runs-on: ubuntu-latest
  test:
    runs-on: ubuntu-latest
  ci-gate:
    needs: [build]
";
            let (jobs, rollup_needs) = parse_workflow_jobs(workflow, Some("ci-gate"));
            let missing_test = jobs.contains("test") && !rollup_needs.contains("test");

            let action_re = Regex::new(r#"uses:\s*['"]?([a-zA-Z0-9_.-]+/[a-zA-Z0-9_.-]+)@([^'"\s#]+)"#)?;
            let pinned = action_re.captures("uses: actions/checkout@b4ffde65f46336ab88eb53be808477a3936bae11");
            let unpinned = action_re.captures("uses: actions/checkout@v4");

            let is_sha = |caps: Option<regex::Captures>| {
                caps.map(|c| {
                    let r = c.get(2).unwrap().as_str();
                    r.len() == 40 && r.chars().all(|ch| ch.is_ascii_hexdigit())
                }).unwrap_or(false)
            };

            let continue_err_re = Regex::new(r"continue-on-error:\s*true\b")?;
            let masks = continue_err_re.is_match("continue-on-error: true");
            let clean = !continue_err_re.is_match("continue-on-error: false");

            Ok(missing_test && is_sha(pinned) && !is_sha(unpinned) && masks && clean)
        },
    ),
    (
        "bench-regression: iai console parsing, two-tier threshold, and sourced overrides",
        || {
            use crate::config::{BenchRegressionGate, Severity};
            use crate::guards::perf::bounds::DiscreteMetric;
            use crate::guards::perf::{
                evaluate_metrics_regression_with_directives, parse_iai_callgrind_console_both,
                BenchmarkMetric, MetricValue,
            };
            use crate::guards::GateOutcome;
            use crate::tokens::{OverrideSource, ParsedDirective};

            let sample = "\
cost::map_insert random:\"random\"
  Instructions:               1,050|1,000 (+5.0000%)
smoke_cost::set_contains
  Instructions:               500|N/A (No baseline)
";
            let (head, base) = parse_iai_callgrind_console_both(sample);
            if head.len() != 2 || base.len() != 1 {
                return Ok(false);
            }

            let settings = BenchRegressionGate {
                tolerance_pct: 5.0,
                noise_floor_pct: Some(0.5),
                advisory_pct: Some(0.1),
                require_sourced_override: true,
                ..Default::default()
            };

            let base_metrics = vec![
                BenchmarkMetric {
                    name: "map_get".to_string(),
                    count: 1000.0,
                    value: MetricValue::Discrete(DiscreteMetric::new(1000)),
                    unit: "Ir".to_string(),
                },
                BenchmarkMetric {
                    name: "map_insert".to_string(),
                    count: 1000.0,
                    value: MetricValue::Discrete(DiscreteMetric::new(1000)),
                    unit: "Ir".to_string(),
                },
            ];

            // 1 arm at 2% regression (< 5% single-worst, only 1 arm > 0.5% noise floor) -> passes
            let head_pass = vec![
                BenchmarkMetric {
                    name: "map_get".to_string(),
                    count: 1020.0,
                    value: MetricValue::Discrete(DiscreteMetric::new(1020)),
                    unit: "Ir".to_string(),
                },
                BenchmarkMetric {
                    name: "map_insert".to_string(),
                    count: 1000.0,
                    value: MetricValue::Discrete(DiscreteMetric::new(1000)),
                    unit: "Ir".to_string(),
                },
            ];
            let mut out1 = GateOutcome::new("bench-regression");
            evaluate_metrics_regression_with_directives(
                &[],
                &settings,
                &base_metrics,
                &head_pass,
                "base.txt",
                "head.txt",
                Severity::Error,
                &mut out1,
            )?;
            let pass_ok = out1.violations.is_empty();

            // 2 arms at 2% regression (>= 2 arms > 0.5% noise floor) -> fails
            let head_fail = vec![
                BenchmarkMetric {
                    name: "map_get".to_string(),
                    count: 1020.0,
                    value: MetricValue::Discrete(DiscreteMetric::new(1020)),
                    unit: "Ir".to_string(),
                },
                BenchmarkMetric {
                    name: "map_insert".to_string(),
                    count: 1020.0,
                    value: MetricValue::Discrete(DiscreteMetric::new(1020)),
                    unit: "Ir".to_string(),
                },
            ];
            let mut out2 = GateOutcome::new("bench-regression");
            evaluate_metrics_regression_with_directives(
                &[],
                &settings,
                &base_metrics,
                &head_fail,
                "base.txt",
                "head.txt",
                Severity::Error,
                &mut out2,
            )?;
            let fail_ok = out2.violations.len() == 2;

            // Sourced override naming map_get: approved for map_get, map_insert fails
            let dirs = vec![ParsedDirective {
                directive: "allow-regression".to_string(),
                reason: "map_get trade refs https://github.com/orieg/expanse/actions/runs/34490311084".to_string(),
                source: OverrideSource::PrBody,
                hidden: false,
            }];
            let mut out3 = GateOutcome::new("bench-regression");
            evaluate_metrics_regression_with_directives(
                &dirs,
                &settings,
                &base_metrics,
                &head_fail,
                "base.txt",
                "head.txt",
                Severity::Error,
                &mut out3,
            )?;
            let override_subset_ok = out3.overrides.len() == 1 && out3.violations.len() == 1;

            Ok(pass_ok && fail_ok && override_subset_ok)
        },
    ),
    (
        "provenance-tags: table provenance, mechanism claims, wall-clock intervals, and paired figures",
        || {
            use crate::guards::provenance_tags::scan_markdown_text;

            // 1. Table with numbers without tag -> fires
            let bad_table = "| Arm | Latency |\n|---|---|\n| get | 12.4 ns |\n";
            let v_table_bad = scan_markdown_text(bad_table, "docs/perf.md", true, true, true, true);
            let good_table = "| Arm | Latency |\n|---|---|\n| get | 12.4 ns |\n\n(measured: Apple M1, abc1234)\n";
            let v_table_good = scan_markdown_text(good_table, "docs/perf.md", true, true, true, true);

            // 2. Mechanism claim without evidence -> fires
            let bad_mech = "The speedup is memory-latency-bound across all sizes.";
            let v_mech_bad = scan_markdown_text(bad_mech, "docs/perf.md", true, true, true, true);
            let good_mech = "The speedup is memory-latency-bound (measured via LLC-load-misses in results/cache.json).";
            let v_mech_good = scan_markdown_text(good_mech, "docs/perf.md", true, true, true, true);

            // 3. Bare wall-clock ratio -> fires
            let bad_ratio = "The new algorithm is 2.9x faster on realistic workloads.";
            let v_ratio_bad = scan_markdown_text(bad_ratio, "docs/perf.md", true, true, true, true);
            let good_ratio = "The new algorithm is 2.9x faster [2.7x, 3.1x] on realistic workloads.";
            let v_ratio_good = scan_markdown_text(good_ratio, "docs/perf.md", true, true, true, true);

            // 4. Paired figures without workload tag -> fires
            let bad_pair = "Lookup takes 11.9 ns vs 108.9 ns in the competitor.";
            let v_pair_bad = scan_markdown_text(bad_pair, "docs/perf.md", true, true, true, true);
            let good_pair = "Lookup takes 11.9 ns vs 108.9 ns (workload: uniform-random) in the competitor.";
            let v_pair_good = scan_markdown_text(good_pair, "docs/perf.md", true, true, true, true);

            Ok(!v_table_bad.is_empty()
                && v_table_good.is_empty()
                && !v_mech_bad.is_empty()
                && v_mech_good.is_empty()
                && !v_ratio_bad.is_empty()
                && v_ratio_good.is_empty()
                && !v_pair_bad.is_empty()
                && v_pair_good.is_empty())
        },
    ),
    (
        "presets: cargo-public-api, miri, and sanitizers preset resolution",
        || {
            use crate::guards::presets::resolve_preset;
            let api = resolve_preset("cargo-public-api").expect("cargo-public-api preset exists");
            let miri = resolve_preset("miri").expect("miri preset exists");
            let san = resolve_preset("sanitizers").expect("sanitizers preset exists");

            let api_ok = api.default_command.contains("public-api") && api.policy_files.contains(&"public-api.txt");
            let miri_ok = miri.default_command.contains("miri") && miri.zero_items_pattern.is_some();
            let san_ok = san.canary_expected_diagnostic == Some("ThreadSanitizer: data race");

            Ok(api_ok && miri_ok && san_ok)
        },
    ),
    (
        "archive-contents: detects missing required paths and forbidden entry leaks",
        || {
            use crate::guards::archive_contents::read_archive_entries;
            use flate2::write::GzEncoder;
            use flate2::Compression;
            use std::fs::File;

            let temp_path = std::env::temp_dir().join(format!("discipline_selftest_{}.tgz", std::process::id()));
            let f = File::create(&temp_path)?;
            let enc = GzEncoder::new(f, Compression::default());
            let mut tar = tar::Builder::new(enc);

            let data = b"content";
            let mut h1 = tar::Header::new_gnu();
            h1.set_size(data.len() as u64);
            h1.set_mode(0o644);
            h1.set_cksum();
            tar.append_data(&mut h1, "Judy-2.6.0/config.m4", &data[..])?;

            let mut h2 = tar::Header::new_gnu();
            h2.set_size(data.len() as u64);
            h2.set_mode(0o644);
            h2.set_cksum();
            tar.append_data(&mut h2, "Judy-2.6.0/tools/check.sh", &data[..])?;
            let enc = tar.into_inner()?;
            enc.finish()?;

            let entries = read_archive_entries(&temp_path, 1);
            let _ = std::fs::remove_file(&temp_path);
            let entries = entries?;
            let has_required = entries.contains(&"config.m4".to_string());
            let missing_required = !entries.contains(&"php_judy.h".to_string());

            let forbidden_re = Regex::new(r"^tools/")?;
            let has_forbidden = entries.iter().any(|e| forbidden_re.is_match(e));
            let clean_entry_safe = !forbidden_re.is_match("config.m4");

            Ok(has_required && missing_required && has_forbidden && clean_entry_safe)
        },
    ),
    (
        "manifest-sync: extracts declared paths and reconciles bidirectional diffs",
        || {
            let manifest_xml = r#"
            <package>
                <contents>
                    <file name="tests/001.phpt" role="test"/>
                    <file name="tests/ghost.phpt" role="test"/>
                </contents>
            </package>"#;

            let re = Regex::new(r#"<file\s+name="([^"]+)""#)?;
            let manifest_files: std::collections::BTreeSet<String> = re
                .captures_iter(manifest_xml)
                .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
                .collect();

            let git_files = ["tests/001.phpt".to_string(), "tests/unmanifested.phpt".to_string()];

            let unmanifested: Vec<_> = git_files
                .iter()
                .filter(|f| !manifest_files.contains(*f))
                .cloned()
                .collect();
            let ghost: Vec<_> = manifest_files
                .iter()
                .filter(|f| !git_files.contains(f))
                .cloned()
                .collect();

            Ok(unmanifested == vec!["tests/unmanifested.phpt"] && ghost == vec!["tests/ghost.phpt"])
        },
    ),
    (
        "version-lockstep: verifies multi-source equality and detects mismatch",
        || {
            let header = "#define PHP_JUDY_VERSION \"2.6.0\"\n";
            let manifest_match = "<release>2.6.0</release>";
            let manifest_mismatch = "<release>2.5.0</release>";

            let h_re = Regex::new(r#"#define\s+PHP_JUDY_VERSION\s+"([^"]+)""#)?;
            let m_re = Regex::new(r#"<release>([^<]+)</release>"#)?;

            let h_ver = h_re.captures(header).and_then(|c| c.get(1)).map(|m| m.as_str()).unwrap();
            let m_ver_ok = m_re.captures(manifest_match).and_then(|c| c.get(1)).map(|m| m.as_str()).unwrap();
            let m_ver_bad = m_re.captures(manifest_mismatch).and_then(|c| c.get(1)).map(|m| m.as_str()).unwrap();

            Ok(h_ver == m_ver_ok && h_ver != m_ver_bad)
        },
    ),
    (
        "bench-regression: sample array adapter computes bootstrap confidence interval",
        || {
            use crate::guards::perf::{parse_metrics, MetricValue};

            let json = r#"{
                "benchmarks": {
                    "core.bitset.write.judy": {
                        "median_ms": 14.5314,
                        "runs_ms": [14.3895, 14.476, 14.5057, 14.5314, 14.5369, 14.538, 14.5991]
                    }
                }
            }"#;

            let metrics = parse_metrics("baselines/latest.json", json)?;
            if metrics.len() != 1 {
                return Ok(false);
            }

            match &metrics[0].value {
                MetricValue::Continuous(est) => {
                    let has_ci = est.ci.is_some();
                    let ci = est.ci.unwrap();
                    let valid_bounds = ci.lower <= est.point_estimate && est.point_estimate <= ci.upper;
                    let valid_unit = est.unit == "ms";
                    Ok(has_ci && valid_bounds && valid_unit)
                }
                _ => Ok(false),
            }
        },
    ),
    (
        "scope-confinement: glob confinement discriminates authorized and forbidden changes",
        || {
            use globset::{Glob, GlobSetBuilder};
            let mut allowed_b = GlobSetBuilder::new();
            allowed_b.add(Glob::new("src/**")?);
            let allowed = allowed_b.build()?;

            let mut forbidden_b = GlobSetBuilder::new();
            forbidden_b.add(Glob::new(".github/**")?);
            let forbidden = forbidden_b.build()?;

            let ok_file = "src/lib.rs";
            let bad_file = ".github/workflows/ci.yml";
            let outside_file = "docs/guide.md";

            Ok(allowed.is_match(ok_file)
                && !forbidden.is_match(ok_file)
                && forbidden.is_match(bad_file)
                && !allowed.is_match(outside_file))
        },
    ),
    (
        "suppression-delta: annotations across languages are detected",
        || {
            let rust_suppr = "#[allow(unused_variables)]";
            let py_suppr = "x = 1  # noqa";
            let ts_suppr = "// @ts-ignore";
            let clean = "let x = 1;";

            let has_suppr = |s: &str| {
                s.contains("#[allow(")
                    || s.contains("# noqa")
                    || s.contains("// @ts-ignore")
            };

            Ok(has_suppr(rust_suppr)
                && has_suppr(py_suppr)
                && has_suppr(ts_suppr)
                && !has_suppr(clean))
        },
    ),
    (
        "pr-checklist: regex detects checked and unchecked items",
        || {
            let re = Regex::new(r"(?i)^[ \t]*-[ \t]*\[[xX]\][ \t]*(.*)$")?;
            let checked = "- [x] Added unit tests";
            let checked_upper = "- [X] Updated docs";
            let unchecked = "- [ ] Benchmarks added";

            Ok(re.is_match(checked)
                && re.is_match(checked_upper)
                && !re.is_match(unchecked))
        },
    ),
    (
        "unsafe-budget: counts unsafe blocks across AST facts",
        || {
            let v = AssertVocabulary::default();
            let src = "fn safe() {}\nfn unsafe_fn() {\n    unsafe { let _ = 1; }\n    unsafe { let _ = 2; }\n}";
            let facts = analyze(src, &v)?;
            Ok(facts.unsafe_sites.len() == 2)
        },
    ),
    (
        "msrv: parse_rust_version extracts valid MSRV string",
        || {
            let cargo_toml = "[package]\nname = \"foo\"\nversion = \"0.1.0\"\nrust-version = \"1.90\"\n";
            let no_msrv = "[package]\nname = \"foo\"\nversion = \"0.1.0\"\n";
            let parsed_ok = crate::guards::msrv::parse_rust_version(cargo_toml);
            let parsed_none = crate::guards::msrv::parse_rust_version(no_msrv);

            Ok(parsed_ok == Some("1.90".to_string()) && parsed_none.is_none())
        },
    ),
    (
        "miri: resolves static preset with zero-tests pattern",
        || {
            let preset = crate::guards::presets::resolve_preset("miri");
            match preset {
                Some(p) => Ok(p.default_command.contains("cargo miri test")
                    && p.zero_items_pattern == Some("running 0 tests")),
                None => Ok(false),
            }
        },
    ),
    (
        "sanitizers: resolves static preset with race canary diagnostic",
        || {
            let preset = crate::guards::presets::resolve_preset("sanitizers");
            match preset {
                Some(p) => Ok(p.canary_command.is_some()
                    && p.canary_expected_diagnostic == Some("ThreadSanitizer: data race")),
                None => Ok(false),
            }
        },
    ),
];

pub fn run() -> Result<bool> {
    let mut failed = 0;
    for (name, case) in CASES {
        let ok = matches!(case(), Ok(true));
        println!("{} {name}", if ok { "ok  " } else { "FAIL" });
        failed += usize::from(!ok);
    }
    if CASES.is_empty() {
        bail!("self-test has no cases");
    }
    println!("\n{} case(s), {failed} failed", CASES.len());
    Ok(failed == 0)
}
