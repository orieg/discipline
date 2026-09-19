//! Golden-output / PHPT language pack.
//!
//! Parses `.phpt` test section files (`--TEST--`, `--FILE--`, `--EXPECT--`, `--EXPECTF--`,
//! `--EXPECTREGEX--`, `--SKIPIF--`, `--XFAIL--`) into universal [`ParsedFileFacts`].

use anyhow::Result;

use super::{AssertVocabulary, LanguagePack, ParsedFileFacts, TestFn};

/// Golden / PHPT language pack implementing [`LanguagePack`].
pub struct GoldenPack;

impl LanguagePack for GoldenPack {
    fn id(&self) -> &'static str {
        "golden"
    }

    fn name(&self) -> &'static str {
        "Golden / PHPT"
    }

    fn matches(&self, path: &str) -> bool {
        super::extension(path) == Some("phpt")
    }

    fn extract(&self, path: &str, src: &str, _vocab: &AssertVocabulary) -> Result<ParsedFileFacts> {
        let sections = parse_phpt_sections(src);

        let mut has_parse_errors = false;
        // A valid PHPT test must contain at least --TEST-- and --FILE--
        if !sections.iter().any(|(s, _)| s == "FILE") {
            has_parse_errors = true;
        }

        let name = sections
            .iter()
            .find(|(s, _)| s == "TEST")
            .map(|(_, c)| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path).to_string());

        // Expectations: --EXPECT--, --EXPECTF--, or --EXPECTREGEX--
        let expect_section = sections
            .iter()
            .find(|(s, _)| *s == "EXPECT" || *s == "EXPECTF" || *s == "EXPECTREGEX");

        let mut total_asserts = 0;
        let mut strong_asserts = 0;
        let mut tautologies = 0;

        if let Some((_kind, content)) = expect_section {
            let non_empty_lines = content.lines().filter(|l| !l.trim().is_empty()).count();
            if non_empty_lines > 0 {
                total_asserts = non_empty_lines;
                strong_asserts = non_empty_lines;
            } else {
                // Empty expectation is vacuous
                total_asserts = 1;
                tautologies = 1;
            }
        }

        let has_xfail = sections.iter().any(|(s, _)| s == "XFAIL");
        let has_skipif = sections.iter().any(|(s, c)| {
            if s != "SKIPIF" {
                return false;
            }
            // An unconditioned skip: e.g. "print 'skip'" without an "if"
            let text = c.to_ascii_lowercase();
            let prints_skip = text.contains("print \"skip\"")
                || text.contains("print 'skip'")
                || text.contains("echo \"skip\"")
                || text.contains("echo 'skip'")
                || text.contains("die('skip')")
                || text.contains("die(\"skip\")");
            prints_skip && !text.contains("if")
        });

        let ignored = has_xfail || has_skipif;
        let should_panic = has_xfail;

        let end_line = src.lines().count().max(1);
        let test = TestFn {
            name,
            line: 1,
            end_line,
            total_asserts,
            strong_asserts,
            tautologies,
            ignored,
            should_panic,
            ..Default::default()
        };

        Ok(ParsedFileFacts {
            tests: vec![test],
            unsafe_sites: Vec::new(),
            escape_hatches: Vec::new(),
            has_parse_errors,
            ..Default::default()
        })
    }
}

fn parse_phpt_sections(src: &str) -> Vec<(String, String)> {
    let mut sections = Vec::new();
    let mut current_section: Option<String> = None;
    let mut current_content = String::new();

    for line in src.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("--") && trimmed.ends_with("--") && trimmed.len() > 4 {
            let section_name = &trimmed[2..trimmed.len() - 2];
            if section_name
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            {
                if let Some(sec) = current_section.take() {
                    sections.push((sec, current_content));
                    current_content = String::new();
                }
                current_section = Some(section_name.to_string());
                continue;
            }
        }
        if current_section.is_some() {
            current_content.push_str(line);
            current_content.push('\n');
        }
    }

    if let Some(sec) = current_section {
        sections.push((sec, current_content));
    }

    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_phpt_sections_and_expectations() {
        let src = r#"--TEST--
Check for Judy BITSET set/unset/test methods
--SKIPIF--
<?php if (!extension_loaded("judy")) print "skip"; ?>
--FILE--
<?php
echo "line 1\n";
echo "line 2\n";
?>
--EXPECT--
line 1
line 2
"#;
        let pack = GoldenPack;
        assert!(pack.matches("tests/001.phpt"));
        assert!(!pack.matches("tests/001.rs"));

        let facts = pack
            .extract("tests/001.phpt", src, &AssertVocabulary::default())
            .expect("extract");
        assert!(!facts.has_parse_errors);
        assert_eq!(facts.tests.len(), 1);
        let t = &facts.tests[0];
        assert_eq!(t.name, "Check for Judy BITSET set/unset/test methods");
        assert_eq!(t.total_asserts, 2);
        assert_eq!(t.strong_asserts, 2);
        assert_eq!(t.tautologies, 0);
        assert!(!t.ignored);
        assert!(!t.is_vacuous());
    }

    #[test]
    fn empty_expectation_is_vacuous() {
        let src = r#"--TEST--
Empty Expectation Test
--FILE--
<?php echo 1; ?>
--EXPECT--
"#;
        let facts = GoldenPack
            .extract("tests/empty.phpt", src, &AssertVocabulary::default())
            .expect("extract");
        assert!(facts.tests[0].is_vacuous());
    }

    #[test]
    fn missing_file_section_marks_parse_error() {
        let src = r#"--TEST--
Incomplete Test
--EXPECT--
hello
"#;
        let facts = GoldenPack
            .extract("tests/bad.phpt", src, &AssertVocabulary::default())
            .expect("extract");
        assert!(facts.has_parse_errors);
    }

    #[test]
    fn xfail_and_unconditional_skip_mark_ignored() {
        let xfail_src = r#"--TEST--
Buggy Test
--XFAIL--
Expected bug #12345
--FILE--
<?php echo 1; ?>
--EXPECT--
1
"#;
        let facts = GoldenPack
            .extract("tests/xfail.phpt", xfail_src, &AssertVocabulary::default())
            .expect("extract");
        assert!(facts.tests[0].ignored);
        assert!(facts.tests[0].should_panic);

        let skip_src = r#"--TEST--
Unconditional Skip
--SKIPIF--
<?php print 'skip'; ?>
--FILE--
<?php echo 1; ?>
--EXPECT--
1
"#;
        let facts_skip = GoldenPack
            .extract("tests/skip.phpt", skip_src, &AssertVocabulary::default())
            .expect("extract");
        assert!(facts_skip.tests[0].ignored);
    }

    #[test]
    fn fixture_negative_control_clean_suite_passes() {
        let src = include_str!("../../tests/fixtures/golden/clean.phpt");
        let vocab = AssertVocabulary::default();
        let facts = GoldenPack
            .extract("tests/clean.phpt", src, &vocab)
            .expect("extract clean");
        assert!(!facts.has_parse_errors);
        assert_eq!(facts.tests.len(), 1);
        assert!(!facts.tests[0].is_vacuous());
        assert!(!facts.tests[0].ignored);
        assert_eq!(facts.tests[0].total_asserts, 1);
        assert_eq!(facts.tests[0].strong_asserts, 1);
    }

    #[test]
    fn fixture_vacuous_empty_expectation_detected() {
        let src = include_str!("../../tests/fixtures/golden/vacuous.phpt");
        let vocab = AssertVocabulary::default();
        let facts = GoldenPack
            .extract("tests/vacuous.phpt", src, &vocab)
            .expect("extract vacuous");
        assert_eq!(facts.tests.len(), 1);
        assert!(facts.tests[0].is_vacuous());
    }

    #[test]
    fn fixture_implicit_assertions_detected() {
        let src = include_str!("../../tests/fixtures/golden/implicit_asserts.phpt");
        let vocab = AssertVocabulary::default();
        let facts = GoldenPack
            .extract("tests/implicit.phpt", src, &vocab)
            .expect("extract implicit");
        assert_eq!(facts.tests.len(), 1);
        assert!(!facts.tests[0].is_vacuous());
        assert_eq!(facts.tests[0].total_asserts, 1);
        assert_eq!(facts.tests[0].strong_asserts, 1);
    }

    #[test]
    fn fixture_skips_and_xfail_detected() {
        let vocab = AssertVocabulary::default();
        let skip_src = include_str!("../../tests/fixtures/golden/skips.phpt");
        let skip_facts = GoldenPack
            .extract("tests/skip.phpt", skip_src, &vocab)
            .expect("extract skips");
        assert!(skip_facts.tests[0].ignored);

        let xfail_src = include_str!("../../tests/fixtures/golden/xfail.phpt");
        let xfail_facts = GoldenPack
            .extract("tests/xfail.phpt", xfail_src, &vocab)
            .expect("extract xfail");
        assert!(xfail_facts.tests[0].ignored);
        assert!(xfail_facts.tests[0].should_panic);
    }

    #[test]
    fn fixture_comments_and_strings_not_counted_as_expectations() {
        let src = include_str!("../../tests/fixtures/golden/comments_and_strings.phpt");
        let vocab = AssertVocabulary::default();
        let facts = GoldenPack
            .extract("tests/comments.phpt", src, &vocab)
            .expect("extract comments");
        assert_eq!(facts.tests.len(), 1);
        assert_eq!(facts.tests[0].total_asserts, 1);
        assert_eq!(facts.tests[0].strong_asserts, 1);
        assert!(!facts.tests[0].is_vacuous());
    }

    #[test]
    fn fixture_syntax_error_missing_file_section() {
        let src = include_str!("../../tests/fixtures/golden/syntax_error.phpt");
        let vocab = AssertVocabulary::default();
        let facts = GoldenPack
            .extract("tests/broken.phpt", src, &vocab)
            .expect("extract broken");
        assert!(facts.has_parse_errors);
    }
}
