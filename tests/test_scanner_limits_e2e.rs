//! A grammar scanner with a fixed-size serialized state can abort the process or mis-read
//! a file on a construct past that size (#636). tree-sitter-ruby 0.23.1 stores a
//! here-document word's length in one byte, so a word of 256 bytes round-trips as length 0
//! and the scanner's `assert(size == length)` aborts; without assertions it reads an empty
//! word and the here-document swallows the rest of the file. tree-sitter-python 0.25.0 writes
//! its indentation stack two bytes per level into a 1024-byte buffer and tests the space once
//! but writes twice, so a deep enough stack writes past the buffer (and the parser library
//! aborts on its own `length <= 1024`). A change must not be able to do either through one
//! file, so the file is refused before the parser sees it, by name, the way an over-budget
//! parse is. The controls are the same change at the limit, which is parsed.

mod common;
#[cfg(any(feature = "lang-ruby", feature = "lang-python"))]
use common::{Repo, Run};

/// The gates that need every changed file's facts and stop on one they cannot get.
#[cfg(any(feature = "lang-ruby", feature = "lang-python"))]
const NEED_EVERY_FILE: &[&str] = &[
    "assertion-reduction",
    "vacuous-tests",
    "ignored-tests",
    "unsafe-safety-comment",
    "deletion-rationale",
];

/// A Ruby file whose here-document word is `word_len` bytes, followed by a method a gate
/// would read.
#[cfg(feature = "lang-ruby")]
fn ruby_with_heredoc_word(word_len: usize) -> String {
    let w = "A".repeat(word_len);
    format!("def hidden\n  assert_equal 1, 2\nend\nx = <<{w}\nbody\n{w}\n")
}

#[cfg(feature = "lang-ruby")]
fn setup(repo: &Repo, word_len: usize) -> Run {
    repo.commit_base("test/a_test.rb", "x = 1\n", "feat: add a test");
    repo.write("test/a_test.rb", &ruby_with_heredoc_word(word_len));
    repo.git(&["add", "-A"]);
    repo.check_with_pr(&[], "no-issue: a test change")
}

#[cfg(feature = "lang-ruby")]
#[test]
fn a_change_with_an_over_long_here_document_word_is_refused_by_name() {
    let repo = Repo::new();
    let run = setup(&repo, 256);
    // The run did not abort and did not pass: it is a fail-closed "could not check".
    assert_eq!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    let (reason, gate) = run.could_not_check();
    assert_eq!(reason, "gate");
    assert!(
        NEED_EVERY_FILE.contains(&gate.as_deref().unwrap_or("")),
        "{gate:?}"
    );
    let detail = run.json()["could_not_check"]["detail"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        detail.contains("could not parse `test/a_test.rb`")
            && detail.contains("here-document word of 256 bytes")
            && detail.contains("255"),
        "{detail}"
    );
    // No gate reported an outcome, so nothing reads as a pass.
    assert_eq!(run.json()["outcomes"].as_array().unwrap().len(), 0);
}

/// The fuzz seed for this defect (`fuzz/corpus/language_packs/ruby_heredoc_word_over_limit`),
/// read as the fuzz target reads it: the first byte selects the Ruby pack, the rest is the
/// source. The pack refuses it rather than aborting.
#[cfg(feature = "lang-ruby")]
#[test]
fn the_fuzz_seed_is_refused_not_aborted() {
    use discipline::ast::{default_registry, AssertVocabulary};
    let path = format!(
        "{}/fuzz/corpus/language_packs/ruby_heredoc_word_over_limit",
        env!("CARGO_MANIFEST_DIR")
    );
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    assert_eq!(bytes[0], 13, "the first byte must select the Ruby pack");
    let src = String::from_utf8_lossy(&bytes[1..]).into_owned();
    let reg = default_registry();
    let pack = reg.find_pack("src/m.rb").expect("the Ruby pack");
    let err = pack
        .extract("src/m.rb", &src, &AssertVocabulary::default())
        .unwrap_err()
        .to_string();
    assert!(err.contains("here-document word of 256 bytes"), "{err}");
}

#[cfg(feature = "lang-ruby")]
#[test]
fn the_control_at_the_limit_is_checked_not_refused() {
    let repo = Repo::new();
    let run = setup(&repo, 255);
    // The word at the limit parses: the gates run and report outcomes.
    assert!(
        run.json()["could_not_check"].is_null(),
        "{}",
        run.json()["could_not_check"]
    );
    assert!(!run.json()["outcomes"].as_array().unwrap().is_empty());
}

// ---- Python indentation nesting -------------------------------------------

/// A Python file with `levels` distinct indentation prefixes (`levels - 1` nested blocks),
/// the deepest line opening an f-string, plus a function a gate would read.
#[cfg(feature = "lang-python")]
fn python_with_nesting(levels: usize) -> String {
    let mut s = String::from("def hidden():\n    assert 1 == 2\n");
    for i in 0..levels.saturating_sub(1) {
        s.push_str(&" ".repeat(i));
        s.push_str("if x:\n");
    }
    s.push_str(&" ".repeat(levels.saturating_sub(1)));
    s.push_str("y = f\"{a}\"\n");
    s
}

#[cfg(feature = "lang-python")]
#[test]
fn a_change_with_too_many_indentation_prefixes_is_refused_by_name() {
    let repo = Repo::new();
    repo.commit_base("test/a_test.py", "x = 1\n", "feat: add a test");
    // The previously crashing shape: far past the limit, with an open f-string.
    repo.write("test/a_test.py", &python_with_nesting(511));
    repo.git(&["add", "-A"]);
    let run = repo.check_with_pr(&[], "no-issue: a test change");
    assert_eq!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    let (reason, gate) = run.could_not_check();
    assert_eq!(reason, "gate");
    assert!(
        NEED_EVERY_FILE.contains(&gate.as_deref().unwrap_or("")),
        "{gate:?}"
    );
    let detail = run.json()["could_not_check"]["detail"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        detail.contains("could not parse `test/a_test.py`")
            && detail.contains("more than 200 distinct indentation prefixes"),
        "{detail}"
    );
    assert_eq!(run.json()["outcomes"].as_array().unwrap().len(), 0);
}

/// The fuzz seed (`fuzz/corpus/language_packs/python_indent_nesting_over_limit`): the first
/// byte selects the Python pack, the rest is the once-crashing source. The pack refuses it
/// rather than aborting.
#[cfg(feature = "lang-python")]
#[test]
fn the_python_fuzz_seed_is_refused_not_aborted() {
    use discipline::ast::{default_registry, AssertVocabulary};
    let path = format!(
        "{}/fuzz/corpus/language_packs/python_indent_nesting_over_limit",
        env!("CARGO_MANIFEST_DIR")
    );
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    assert_eq!(bytes[0], 1, "the first byte must select the Python pack");
    let src = String::from_utf8_lossy(&bytes[1..]).into_owned();
    let reg = default_registry();
    let pack = reg.find_pack("src/m.py").expect("the Python pack");
    let err = pack
        .extract("src/m.py", &src, &AssertVocabulary::default())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("more than 200 distinct indentation prefixes"),
        "{err}"
    );
}

/// A deeply but legitimately indented Python file (at the limit) is checked, not refused.
#[cfg(feature = "lang-python")]
#[test]
fn the_python_control_at_the_limit_is_checked_not_refused() {
    let repo = Repo::new();
    repo.commit_base("test/a_test.py", "x = 1\n", "feat: add a test");
    repo.write("test/a_test.py", &python_with_nesting(200));
    repo.git(&["add", "-A"]);
    let run = repo.check_with_pr(&[], "no-issue: a test change");
    assert!(
        run.json()["could_not_check"].is_null(),
        "{}",
        run.json()["could_not_check"]
    );
    assert!(!run.json()["outcomes"].as_array().unwrap().is_empty());
}
