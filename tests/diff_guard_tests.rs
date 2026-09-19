mod common;
use common::{Repo, GOOD_LIB, GOOD_TEST};

#[test]
fn safety_comment_requires_at_least_four_descriptive_words() {
    let repo = Repo::new();
    // 1. Placeholder / short safety comment (< 4 words) fails
    repo.write(
        "src/lib.rs",
        &format!("{GOOD_LIB}\npub fn short_safety(p: *const u8) -> u8 {{\n    // SAFETY: valid\n    unsafe {{ *p }}\n}}\n"),
    );
    repo.commit("feat: add unsafe with short safety comment");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1, "short safety comment must fail");
    assert_eq!(
        run.titles("unsafe-safety-comment"),
        vec!["Unsafe Without SAFETY Comment"],
        "{}",
        run.stdout
    );

    // 2. Descriptive safety comment (>= 4 words) passes
    repo.write(
        "src/lib.rs",
        &format!("{GOOD_LIB}\npub fn descriptive_safety(p: *const u8) -> u8 {{\n    // SAFETY: caller guarantees pointer is non-null and properly aligned.\n    unsafe {{ *p }}\n}}\n"),
    );
    repo.commit("feat: use descriptive safety comment");
    let run = repo.check(&[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(
        run.titles("unsafe-safety-comment").is_empty(),
        "{}",
        run.stdout
    );
}

#[test]
fn test_suppression_rejects_commented_out_test() {
    let repo = Repo::new();
    // Comment out #[test] with // #[test]
    repo.write(
        "tests/a.rs",
        &GOOD_TEST.replace("#[test]\nfn orders", "// #[test]\nfn orders"),
    );
    repo.commit("test: suppress orders test by commenting out attribute");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1, "commented-out test must fail");
    assert_eq!(
        run.titles("ignored-tests"),
        vec!["Test Newly Skipped"],
        "{}",
        run.stdout
    );

    // Directive allows suppression
    repo.write(
        "body.md",
        "Summary\n\nallow-ignore: orders temporarily disabled pending fix\n",
    );
    let run = repo.check(&["--pr-body-file", "body.md"]);
    assert!(run.titles("ignored-tests").is_empty(), "{}", run.stdout);
}

#[test]
fn test_suppression_rejects_conditional_exclusion_cfg_not_ci() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        &GOOD_TEST.replace("#[test]\nfn orders", "#[test]\n#[cfg(not(ci))]\nfn orders"),
    );
    repo.commit("test: exclude from ci via cfg");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1, "cfg(not(ci)) test suppression must fail");
    assert_eq!(
        run.titles("ignored-tests"),
        vec!["Test Newly Skipped"],
        "{}",
        run.stdout
    );
}

#[test]
fn test_suppression_rejects_conditional_exclusion_cfg_not_test() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        &GOOD_TEST.replace(
            "#[test]\nfn orders",
            "#[test]\n#[cfg(not(test))]\nfn orders",
        ),
    );
    repo.commit("test: exclude from test via cfg");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1, "cfg(not(test)) test suppression must fail");
    assert_eq!(
        run.titles("ignored-tests"),
        vec!["Test Newly Skipped"],
        "{}",
        run.stdout
    );
}

#[test]
fn vacuous_assertions_in_new_test_rejected() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        &format!("{GOOD_TEST}\n#[test]\nfn vacuous_true() {{\n    assert!(true);\n}}\n"),
    );
    repo.commit("test: add vacuous assert!(true)");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    assert_eq!(
        run.titles("vacuous-tests"),
        vec!["Vacuous Test Added"],
        "{}",
        run.stdout
    );

    // Same with assert_eq!(x, x)
    repo.write(
        "tests/a.rs",
        &format!("{GOOD_TEST}\n#[test]\nfn vacuous_identity() {{\n    let x = 42;\n    assert_eq!(x, x);\n}}\n"),
    );
    repo.commit("test: add vacuous assert_eq!(x, x)");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    assert_eq!(
        run.titles("vacuous-tests"),
        vec!["Vacuous Test Added"],
        "{}",
        run.stdout
    );

    // Same with assert_eq!(1, 1)
    repo.write(
        "tests/a.rs",
        &format!("{GOOD_TEST}\n#[test]\nfn vacuous_consts() {{\n    assert_eq!(1, 1);\n}}\n"),
    );
    repo.commit("test: add vacuous assert_eq!(1, 1)");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    assert_eq!(
        run.titles("vacuous-tests"),
        vec!["Vacuous Test Added"],
        "{}",
        run.stdout
    );
}
