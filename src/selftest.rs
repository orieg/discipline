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
                "fn a(p:*const u8)->u8{\n// SAFETY: valid\nunsafe { *p }\n}",
                &v,
            )?;
            let bad = analyze(
                "fn a(p:*const u8)->u8{\nlet _ = \"// SAFETY: x\";\nunsafe{ *p }\n}",
                &v,
            )?;
            Ok(ok.unsafe_sites[0].documented && !bad.unsafe_sites[0].documented)
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
        Ok(hit("Phase 2 (1 week)") && !hit("Phase 2 follows Phase 1"))
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
        "config: unknown keys and planned gates are rejected",
        || {
            let head = "[meta]\nversion = 1\nname = \"t\"\n";
            let typo =
                DisciplineConfig::from_toml_str(&format!("{head}[gates.pii]\nlan_ipz = false\n"));
            let planned =
                DisciplineConfig::from_toml_str(&format!("{head}[gates.miri]\nenabled = true\n"));
            let fine =
                DisciplineConfig::from_toml_str(&format!("{head}[gates.pii]\nlan_ips = false\n"));
            Ok(typo.is_err() && planned.is_err() && fine.is_ok())
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
            Ok(is_exempt_time_estimate(exempt_span, 13, 21, "19 years")
                && !is_exempt_time_estimate(real_est, 14, 21, "3 weeks"))
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
            };
            let h = TestFn {
                name: "test_check".to_string(),
                line: 1,
                total_asserts: 1,
                strong_asserts: 1,
                tautologies: 0,
                ignored: false,
                should_panic: false,
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
            Ok(unexcused.violations.len() == 1
                && unexcused.overrides.is_empty()
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
