//! How `assertion-reduction` reads a Rust test helper that lives in a `#[cfg(test)]`
//! module of the crate and not in a test-support path (#688): `src/test_support.rs`
//! declared by `#[cfg(test)] mod test_support;`, an inline `#[cfg(test)] mod`, a
//! `#[cfg(test)]` function. Such a helper that a test calls, the call resolved through the
//! crate's modules, is reported as `Test Helper Function Weakened` when it loses checks,
//! though no test changes. A helper that moves, gains checks, hands its checks to a
//! helper it calls, or that no test calls is not reported, and neither is a function of
//! the same name that the tests do not reach. Every shape is driven through the real
//! binary.

mod common;
use common::{Repo, Run, CONFIG_HEAD};

const WEAKENED: &str = "Test Helper Function Weakened";
const DECREASED: &str = "Assertion Count Decreased In Existing Test";

const CHECK_3: &str = "fn check(r: &R) {\n    assert_eq!(r.f0, 0);\n    assert_eq!(r.f1, 1);\n    assert_eq!(r.f2, 2);\n}\n";
const CHECK_2: &str = "fn check(r: &R) {\n    assert_eq!(r.f0, 0);\n    assert_eq!(r.f1, 1);\n}\n";
const CHECK_4: &str = "fn check(r: &R) {\n    assert_eq!(r.f0, 0);\n    assert_eq!(r.f1, 1);\n    assert_eq!(r.f2, 2);\n    assert_eq!(r.f0 + r.f2, 2);\n}\n";
const CALL: &str = "check(r);";
const SUPPORT: &str = "src/test_support.rs";
const LIB: &str = "src/lib.rs";

/// The code under test, and what every `src/lib.rs` starts with.
const MAKE: &str = "pub struct R {\n    pub f0: u32,\n    pub f1: u32,\n    pub f2: u32,\n}\n\npub fn make() -> R {\n    R {\n        f0: 0,\n        f1: 1,\n        f2: 2,\n    }\n}\n";

/// `text` indented one module deep.
fn indented(text: &str) -> String {
    text.lines()
        .map(|l| {
            if l.is_empty() {
                "\n".to_string()
            } else {
                format!("    {l}\n")
            }
        })
        .collect()
}

/// `src/lib.rs`: `top` declarations after the code under test, then a `tests` module
/// holding `imports`, the `helper` given and one test, `create`, that runs `calls` and
/// one assertion of its own.
fn lib(top: &str, imports: &str, helper: &str, calls: &str) -> String {
    format!(
        "{MAKE}\n{top}#[cfg(test)]\nmod tests {{\n    use super::*;\n{}\n{}    #[test]\n    fn create() {{\n        let r = &make();\n        {calls}\n        assert_eq!(r.f0 + r.f1, 1);\n    }}\n}}\n",
        indented(imports),
        indented(helper),
    )
}

const DECLARED: &str = "#[cfg(test)]\nmod test_support;\n\n";
const IMPORTED: &str = "use super::test_support::*;\n";

/// `src/lib.rs` with its test calling `check` of the sibling module `test_support`.
fn calling_lib() -> String {
    lib(DECLARED, IMPORTED, "", CALL)
}

/// A support module file holding `helper`, visible to the crate.
fn support(helper: &str) -> String {
    format!("use crate::R;\n\npub(crate) {helper}")
}

/// Commits `base` on `main`, then each of `steps` on a branch, and runs `check` on the
/// last step alone, with `pr_body` when one is given.
fn steps(base: &[(&str, &str)], steps: &[&[(&str, &str)]], pr_body: Option<&str>) -> Run {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", CONFIG_HEAD);
    for (path, content) in base {
        repo.write(path, content);
    }
    repo.commit("test: base");
    repo.git(&["checkout", "-q", "-B", "work"]);
    for (at, step) in steps.iter().enumerate() {
        if at + 1 == steps.len() {
            repo.git(&["branch", "-f", "before"]);
        }
        for (path, content) in *step {
            if content.is_empty() {
                repo.remove(path);
            } else {
                repo.write(path, content);
            }
        }
        repo.commit("refactor: change");
    }
    match pr_body {
        Some(body) => repo.check_with_pr(&["--base", "before"], body),
        None => repo.check(&["--base", "before"]),
    }
}

/// One change from `base` to `head`; a file whose head text is empty is deleted.
fn change(base: &[(&str, &str)], head: &[(&str, &str)]) -> Run {
    steps(base, &[head], None)
}

/// What `assertion-reduction` reported: `(title, file)` of each violation.
fn reported(run: &Run) -> Vec<(String, String)> {
    run.violations("assertion-reduction")
        .iter()
        .map(|v| {
            (
                v["title"].as_str().unwrap().to_string(),
                v["file"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

fn messages(run: &Run) -> String {
    run.violations("assertion-reduction")
        .iter()
        .map(|v| v["message"].as_str().unwrap_or("").to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn weakened_in(path: &str) -> Vec<(String, String)> {
    vec![(WEAKENED.to_string(), path.to_string())]
}

/// Asserts that `run` reports nothing.
fn assert_quiet(case: &str, run: &Run) {
    assert_eq!(reported(run), Vec::new(), "{case}: {}", run.stdout);
}

/// A helper in a sibling `#[cfg(test)]` module loses an assertion and no test
/// changes: the helper is reported, with the tests that call it.
#[test]
fn a_sibling_helper_that_loses_a_check_is_reported() {
    let root = calling_lib();
    let run = change(
        &[(LIB, &root), (SUPPORT, &support(CHECK_3))],
        &[(SUPPORT, &support(CHECK_2))],
    );
    assert_eq!(reported(&run), weakened_in(SUPPORT), "{}", run.stdout);
    let message = messages(&run);
    assert!(
        message.contains("Helper `check`: effective assertions dropped from 3 to 2")
            && message.contains("called by `tests::create`"),
        "{message}"
    );
    assert_eq!(run.code, 1, "{}", run.stdout);

    // The module is test-only by the attribute on an inline module of the helper's file,
    // by an inner attribute of the file, and by the attribute on the function itself.
    let inline = |helper: &str| {
        format!(
            "#[cfg(test)]\npub(crate) mod checks {{\n    use crate::R;\n\n{}}}\n",
            indented(&format!("pub(crate) {helper}"))
        )
    };
    let inner = |helper: &str| format!("#![cfg(test)]\n{}", support(helper));
    let on_fn = |helper: &str| format!("use crate::R;\n\n#[cfg(test)]\npub(crate) {helper}");
    let shapes: [(&str, &str, &str, String, String); 3] = [
        (
            "an inline module",
            "mod test_support;\n\n",
            "use super::test_support::checks::*;\n",
            inline(CHECK_3),
            inline(CHECK_2),
        ),
        (
            "an inner attribute",
            "mod test_support;\n\n",
            IMPORTED,
            inner(CHECK_3),
            inner(CHECK_2),
        ),
        (
            "an attribute on the function",
            "mod test_support;\n\n",
            IMPORTED,
            on_fn(CHECK_3),
            on_fn(CHECK_2),
        ),
    ];
    for (case, top, imports, base, head) in shapes {
        let root = lib(top, imports, "", CALL);
        let run = change(&[(LIB, &root), (SUPPORT, &base)], &[(SUPPORT, &head)]);
        assert_eq!(
            reported(&run),
            weakened_in(SUPPORT),
            "{case}: {}",
            run.stdout
        );
    }

    // A check that only test code holds: `unwrap` replaced by a default.
    let parsed = "fn check(r: &R) {\n    let n: u32 = \"1\".parse().unwrap();\n    assert_eq!(r.f1, n);\n}\n";
    let lenient = parsed.replace(".unwrap()", ".unwrap_or_default()");
    let root = calling_lib();
    let run = change(
        &[(LIB, &root), (SUPPORT, &support(parsed))],
        &[(SUPPORT, &support(&lenient))],
    );
    assert_eq!(reported(&run), weakened_in(SUPPORT), "{}", run.stdout);

    // The test stops calling the helper in the change that weakens it: a test of the
    // base side called it.
    let run = change(
        &[(LIB, &root), (SUPPORT, &support(CHECK_3))],
        &[
            (LIB, &lib(DECLARED, IMPORTED, "", "let _ = r;")),
            (SUPPORT, &support(CHECK_2)),
        ],
    );
    assert_eq!(reported(&run), weakened_in(SUPPORT), "{}", run.stdout);

    // The helper is deleted and the test stops calling it.
    let run = change(
        &[(LIB, &root), (SUPPORT, &support(CHECK_3))],
        &[(LIB, &lib("", "", "", "let _ = r;")), (SUPPORT, "")],
    );
    assert_eq!(reported(&run), weakened_in(SUPPORT), "{}", run.stdout);
    assert!(
        messages(&run).contains("Helper `check`: deleted or checks removed"),
        "{}",
        messages(&run)
    );
}

/// The test reaches the sibling helper through a helper of its own file, or through
/// another helper of the sibling module: it is still a helper a test calls.
#[test]
fn a_sibling_helper_reached_through_another_helper_is_reported() {
    let through_own = lib(
        DECLARED,
        IMPORTED,
        "fn verify(r: &R) {\n    check(r);\n    assert_eq!(r.f2, 2);\n}\n",
        "verify(r);",
    );
    let run = change(
        &[(LIB, &through_own), (SUPPORT, &support(CHECK_3))],
        &[(SUPPORT, &support(CHECK_2))],
    );
    assert_eq!(reported(&run), weakened_in(SUPPORT), "{}", run.stdout);

    // `check` calls `deep`, which loses the assertion: `deep` is reported, and `check`,
    // whose own body is as it was, is not reported for it.
    let two = |deep: &str| {
        format!(
            "use crate::R;\n\npub(crate) fn check(r: &R) {{\n    assert_eq!(r.f0, 0);\n    deep(r);\n}}\n\n{deep}"
        )
    };
    let deep_2 = "fn deep(r: &R) {\n    assert_eq!(r.f1, 1);\n    assert_eq!(r.f2, 2);\n}\n";
    let deep_1 = "fn deep(r: &R) {\n    assert_eq!(r.f1, 1);\n}\n";
    let root = calling_lib();
    let run = change(
        &[(LIB, &root), (SUPPORT, &two(deep_2))],
        &[(SUPPORT, &two(deep_1))],
    );
    assert_eq!(reported(&run), weakened_in(SUPPORT), "{}", run.stdout);
    assert!(
        messages(&run).contains("Helper `deep`: effective assertions dropped from 2 to 1"),
        "{}",
        messages(&run)
    );
}

/// The helper's own body is as it was and what it calls lost the assertion: a method
/// of its file, which is not judged as a helper of the crate. The helper is reported.
#[test]
fn a_sibling_helper_that_loses_a_check_through_a_method_is_reported() {
    let through = |method: &str| {
        format!(
            "use crate::R;\n\nstruct Checks;\n\nimpl Checks {{\n    fn verify(r: &R) {{\n{method}    }}\n}}\n\npub(crate) fn check(r: &R) {{\n    assert_eq!(r.f0, 0);\n    Checks::verify(r);\n}}\n"
        )
    };
    let two = "        assert_eq!(r.f1, 1);\n        assert_eq!(r.f2, 2);\n";
    let one = "        assert_eq!(r.f1, 1);\n";
    let root = calling_lib();
    let run = change(
        &[(LIB, &root), (SUPPORT, &through(two))],
        &[(SUPPORT, &through(one))],
    );
    assert_eq!(reported(&run), weakened_in(SUPPORT), "{}", run.stdout);
    assert!(
        messages(&run).contains("Helper `check`: effective assertions dropped from 3 to 2"),
        "{}",
        messages(&run)
    );
}

/// The two-step path: one change moves the helper from the test's module into a
/// sibling module, which is no finding; the next weakens it there, and is reported.
#[test]
fn a_helper_moved_and_then_weakened_is_reported_at_the_second_change() {
    let inline = lib("", "", CHECK_3, CALL);
    let moved = calling_lib();
    let first: &[(&str, &str)] = &[(LIB, &moved), (SUPPORT, &support(CHECK_3))];
    let run = steps(&[(LIB, &inline)], &[first], None);
    assert_quiet("the move", &run);

    let second: &[(&str, &str)] = &[(SUPPORT, &support(CHECK_2))];
    let run = steps(&[(LIB, &inline)], &[first, second], None);
    assert_eq!(reported(&run), weakened_in(SUPPORT), "{}", run.stdout);
}

/// A helper that moves unchanged is not reported: to another sibling module the
/// tests import, behind a re-export the untouched tests reach it through, and beside a
/// weaker function of the same name in a module the tests do not go through.
#[test]
fn a_helper_moved_unchanged_is_not_reported() {
    let base_lib = calling_lib();
    let base: &[(&str, &str)] = &[(LIB, &base_lib), (SUPPORT, &support(CHECK_3))];

    let other = lib(
        "#[cfg(test)]\nmod checks;\n\n",
        "use super::checks::*;\n",
        "",
        CALL,
    );
    let run = change(
        base,
        &[
            (LIB, &other),
            (SUPPORT, ""),
            ("src/checks.rs", &support(CHECK_3)),
        ],
    );
    assert_quiet("to another sibling module", &run);

    // The tests are untouched: the module re-exports the helper from a module below it.
    let reexport = "mod deep;\n\npub(crate) use deep::check;\n";
    let run = change(
        base,
        &[
            (SUPPORT, reexport),
            ("src/test_support/deep.rs", &support(CHECK_3)),
        ],
    );
    assert_quiet("behind a re-export", &run);

    // A weaker `check` is new in a module that sorts first and that the tests do not go
    // through: the helper is paired with the one the tests' calls resolve to.
    let run = change(
        base,
        &[
            (
                LIB,
                &lib(
                    "#[cfg(test)]\nmod aaa_unrelated;\n#[cfg(test)]\nmod test_support;\n\n",
                    IMPORTED,
                    "",
                    CALL,
                ),
            ),
            ("src/aaa_unrelated.rs", &support(CHECK_2)),
            (SUPPORT, reexport),
            ("src/test_support/deep.rs", &support(CHECK_3)),
        ],
    );
    assert_quiet("beside a weaker function of the same name", &run);

    // The module it moves to is declared with `#[path]`, which is not followed: the
    // helper is paired with the function of its name that is new in a changed file. Its
    // old file stays, holding another function, so the move is not a renamed file.
    let emptied = "use crate::R;\n\npub(crate) fn other(r: &R) {\n    let _ = r;\n}\n";
    let by_path = lib(
        "#[cfg(test)]\n#[path = \"checks.rs\"]\nmod helpers;\n\n",
        "use super::helpers::*;\n",
        "",
        CALL,
    );
    let run = change(
        base,
        &[
            (
                LIB,
                &by_path.replace(
                    "mod helpers;",
                    "mod helpers;\n#[cfg(test)]\nmod test_support;",
                ),
            ),
            (SUPPORT, emptied),
            ("src/checks.rs", &support(CHECK_3)),
        ],
    );
    assert_quiet("to a module that is not followed", &run);

    // Control: the helper loses an assertion on the way.
    let run = change(
        base,
        &[
            (SUPPORT, reexport),
            ("src/test_support/deep.rs", &support(CHECK_2)),
        ],
    );
    assert_eq!(
        reported(&run),
        weakened_in("src/test_support/deep.rs"),
        "{}",
        run.stdout
    );
}

/// A helper whose lost assertion moved into a helper it now calls has lost nothing:
/// the second helper is in its own file, or in another module its call resolves to.
#[test]
fn checks_moved_into_a_helper_the_helper_calls_are_not_lost() {
    let root = calling_lib();
    let base: &[(&str, &str)] = &[(LIB, &root), (SUPPORT, &support(CHECK_3))];
    let same_file = format!(
        "{}\nfn last(r: &R) {{\n    assert_eq!(r.f2, 2);\n}}\n",
        support("fn check(r: &R) {\n    assert_eq!(r.f0, 0);\n    assert_eq!(r.f1, 1);\n    last(r);\n}\n")
    );
    let run = change(base, &[(SUPPORT, &same_file)]);
    assert_quiet("into a helper of its file", &run);

    let calls_deep = "mod deep;\n\nuse crate::R;\n\npub(crate) fn check(r: &R) {\n    assert_eq!(r.f0, 0);\n    assert_eq!(r.f1, 1);\n    deep::last(r);\n}\n";
    let deep = |body: &str| format!("use crate::R;\n\npub(crate) fn last(r: &R) {{\n{body}}}\n");
    let run = change(
        base,
        &[
            (SUPPORT, calls_deep),
            (
                "src/test_support/deep.rs",
                &deep("    assert_eq!(r.f2, 2);\n"),
            ),
        ],
    );
    assert_quiet("into a helper of another module", &run);

    // Control: the second helper holds nothing.
    let run = change(
        base,
        &[
            (SUPPORT, calls_deep),
            ("src/test_support/deep.rs", &deep("    let _ = r;\n")),
        ],
    );
    assert_eq!(reported(&run), weakened_in(SUPPORT), "{}", run.stdout);
}

/// A helper that gains an assertion is not reported.
#[test]
fn a_strengthened_helper_is_not_reported() {
    let root = calling_lib();
    let run = change(
        &[(LIB, &root), (SUPPORT, &support(CHECK_3))],
        &[(SUPPORT, &support(CHECK_4))],
    );
    assert_quiet("one assertion more", &run);
}

/// A test-only function that no test calls is not a helper: losing an assertion
/// there weakens no test. The control beside it is the same function with a caller.
#[test]
fn a_test_only_function_no_test_calls_is_not_a_helper() {
    let uncalled = lib(DECLARED, IMPORTED, "", "let _ = r;");
    let run = change(
        &[(LIB, &uncalled), (SUPPORT, &support(CHECK_3))],
        &[(SUPPORT, &support(CHECK_2))],
    );
    assert_quiet("no test calls it", &run);

    // A function of the code under test, built outside tests too, is not a helper here
    // whatever calls it.
    let production = lib("mod test_support;\n\n", IMPORTED, "", CALL);
    let run = change(
        &[(LIB, &production), (SUPPORT, &support(CHECK_3))],
        &[(SUPPORT, &support(CHECK_2))],
    );
    assert_quiet("built outside tests too", &run);
}

/// A function of the same name in a module the tests do not reach loses an
/// assertion, and the helper the tests call is unchanged: nothing is reported. The name
/// alone names both.
#[test]
fn a_same_named_function_the_tests_do_not_reach_is_not_reported() {
    let root = lib(
        "#[cfg(test)]\nmod test_support;\n#[cfg(test)]\nmod unrelated;\n\n",
        IMPORTED,
        "",
        CALL,
    );
    let run = change(
        &[
            (LIB, &root),
            (SUPPORT, &support(CHECK_3)),
            ("src/unrelated.rs", &support(CHECK_3)),
        ],
        &[("src/unrelated.rs", &support(CHECK_2))],
    );
    assert_quiet("the other function of that name", &run);

    // Control: the same change to the helper the tests do call.
    let run = change(
        &[
            (LIB, &root),
            (SUPPORT, &support(CHECK_3)),
            ("src/unrelated.rs", &support(CHECK_3)),
        ],
        &[(SUPPORT, &support(CHECK_2))],
    );
    assert_eq!(reported(&run), weakened_in(SUPPORT), "{}", run.stdout);
}

/// `allow-assertion-drop:` naming the helper, or its file, lifts the finding and is
/// recorded; one naming something else lifts nothing.
#[test]
fn the_lifting_directive_lifts_a_weakened_sibling_helper() {
    let root = calling_lib();
    let base: &[(&str, &str)] = &[(LIB, &root), (SUPPORT, &support(CHECK_3))];
    let head: &[(&str, &str)] = &[(SUPPORT, &support(CHECK_2))];
    for subject in ["check", SUPPORT] {
        let body = format!("allow-assertion-drop: {subject} the field is checked by the caller\n");
        let run = steps(base, &[head], Some(&body));
        assert_quiet(subject, &run);
        let overrides = run.outcome("assertion-reduction")["overrides"].clone();
        assert_eq!(
            overrides.as_array().map(Vec::len),
            Some(1),
            "{subject}: {}",
            run.stdout
        );
    }
    let run = steps(
        base,
        &[head],
        Some("allow-assertion-drop: create the field is checked by the caller\n"),
    );
    assert_eq!(reported(&run), weakened_in(SUPPORT), "{}", run.stdout);
}

/// A helper in the tests' own module that loses an assertion is shown by the tests that
/// call it, which drop with it: one finding on the test, as before, and none on the
/// helper.
#[test]
fn a_helper_of_the_tests_own_file_is_shown_by_its_tests() {
    let run = change(
        &[(LIB, &lib("", "", CHECK_3, CALL))],
        &[(LIB, &lib("", "", CHECK_2, CALL))],
    );
    assert_eq!(
        reported(&run),
        vec![(DECREASED.to_string(), LIB.to_string())],
        "{}",
        run.stdout
    );
}

/// What a function of the code under test holds is not the helper's: a helper that
/// calls a function of its file built outside tests too is not reported when that
/// function loses an `expect`, or leaves the file.
#[test]
fn what_the_code_under_test_holds_is_not_the_helpers() {
    let file = |parse: &str| {
        format!(
            "{parse}\n#[cfg(test)]\npub(crate) mod checks {{\n    use crate::R;\n\n    pub(crate) fn check(r: &R) {{\n        let n = super::parse(\"1\");\n        assert_eq!(r.f1, n);\n    }}\n}}\n"
        )
    };
    let strict = "pub fn parse(text: &str) -> u32 {\n    let first = text.split(',').next().expect(\"one field\");\n    first.trim().parse().expect(\"a number\")\n}\n";
    let lenient = strict.replace(".expect(\"a number\")", ".unwrap_or_default()");
    let root = lib(
        "mod test_support;\n\n",
        "use super::test_support::checks::*;\n",
        "",
        CALL,
    );
    let run = change(
        &[(LIB, &root), (SUPPORT, &file(strict))],
        &[(SUPPORT, &file(&lenient))],
    );
    assert_quiet("the function under test loses an expect", &run);

    let elsewhere = "pub use crate::parsing::parse;\n";
    let run = change(
        &[(LIB, &root), (SUPPORT, &file(strict))],
        &[
            (
                LIB,
                &root.replace("mod test_support;", "mod parsing;\nmod test_support;"),
            ),
            (SUPPORT, &file(elsewhere)),
            ("src/parsing.rs", strict),
        ],
    );
    assert_quiet("the function under test leaves the file", &run);
}

/// A helper of a test-support file is judged as before, by the pairing of helpers by
/// name, and once: a `#[cfg(test)]` on it does not make it a second finding.
#[test]
fn a_helper_of_a_test_support_file_is_reported_once() {
    let api = "mod common;\n\n#[test]\nfn create() {\n    let r = &common::make();\n    common::check(r);\n    assert_eq!(r.f0 + r.f1, 1);\n}\n";
    let common = |helper: &str| format!("{MAKE}\n#[cfg(test)]\npub {helper}");
    let run = change(
        &[
            ("tests/api.rs", api),
            ("tests/common/mod.rs", &common(CHECK_3)),
        ],
        &[("tests/common/mod.rs", &common(CHECK_2))],
    );
    assert_eq!(
        reported(&run),
        weakened_in("tests/common/mod.rs"),
        "{}",
        run.stdout
    );
}

/// A call that is not resolved to one function names no helper, as before: a module
/// whose file a `#[path]` attribute names is not followed, so the function is not known
/// to be called and its weakening is not reported.
#[test]
fn a_helper_reached_only_through_an_unresolved_call_is_not_reported() {
    let root = lib(
        "#[cfg(test)]\n#[path = \"test_support.rs\"]\nmod helpers;\n\n",
        "use super::helpers::*;\n",
        "",
        CALL,
    );
    let run = change(
        &[(LIB, &root), (SUPPORT, &support(CHECK_3))],
        &[(SUPPORT, &support(CHECK_2))],
    );
    assert_quiet("a module declared with #[path]", &run);
}
