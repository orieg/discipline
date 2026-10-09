//! How `assertion-reduction` reads a Rust test helper that leaves the test's own file for
//! another module of the same crate (#683): the helper still stands for the checks it
//! holds when the call in the test resolves to it through the crate's `mod` declarations
//! and the `use` items or the path the call is written with. A helper that lost checks in
//! the move, a call that is gone, a helper of the same name in a module the call does not
//! reach, and a module the test does not import are each still a drop. Every shape is
//! driven through the real binary.

mod common;
use common::{Repo, Run, CONFIG_HEAD};

const DECREASED: &str = "Assertion Count Decreased In Existing Test";
const MOVED_NOTE: &str = "read as moved into helper";

const CHECK_3: &str = "fn check(r: &R) {\n    assert_eq!(r.f0, 0);\n    assert_eq!(r.f1, 1);\n    assert_eq!(r.f2, 2);\n}\n";
const CHECK_2: &str = "fn check(r: &R) {\n    assert_eq!(r.f0, 0);\n    assert_eq!(r.f1, 1);\n}\n";
const CHECK_0: &str = "fn check(r: &R) {\n    let _ = r;\n}\n";
const CALL: &str = "check(r);";

/// The code under test, and what every fixture file starts with.
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

/// A module file: `top` declarations after the code under test, then a `tests` module
/// holding `imports`, the `helper` given and one test that runs `calls` and one assertion
/// of its own.
fn module(top: &str, imports: &str, helper: &str, calls: &str) -> String {
    format!(
        "{MAKE}\n{top}#[cfg(test)]\nmod tests {{\n    use super::*;\n{}\n{}    #[test]\n    fn create() {{\n        let r = &make();\n        {calls}\n        assert_eq!(r.f0 + r.f1, 1);\n    }}\n}}\n",
        indented(imports),
        indented(helper),
    )
}

/// The base side of every case: the helper `check`, holding three assertions, in the
/// `tests` module of `src/lib.rs`, and the test calling it.
fn base() -> String {
    module("", "", CHECK_3, CALL)
}

/// A support module file holding `helper`, visible to its parent module.
fn support(helper: &str) -> String {
    format!("use super::R;\n\npub(super) {helper}")
}

const DECLARED: &str = "#[cfg(test)]\nmod test_support;\n\n";

/// Commits `base` on `main`, then `head` on a branch, and runs `check`.
fn change(base: &[(&str, &str)], head: &[(&str, &str)]) -> Run {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", CONFIG_HEAD);
    for (path, content) in base {
        repo.write(path, content);
    }
    repo.commit("test: base");
    repo.git(&["checkout", "-q", "-B", "work"]);
    for (path, content) in head {
        repo.write(path, content);
    }
    repo.commit("refactor: change");
    repo.check(&["--base", "main"])
}

/// `src/lib.rs` from [`base`] to `lib`, beside the files `added`.
fn moved(lib: &str, added: &[(&str, &str)]) -> Run {
    let base = base();
    let mut head = vec![("src/lib.rs", lib)];
    head.extend_from_slice(added);
    change(&[("src/lib.rs", &base)], &head)
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

fn notes(run: &Run) -> String {
    run.outcome("assertion-reduction")["notes"]
        .as_array()
        .map(|notes| {
            notes
                .iter()
                .filter_map(|n| n.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn drop_in_lib() -> Vec<(String, String)> {
    vec![(DECREASED.to_string(), "src/lib.rs".to_string())]
}

/// Asserts that `run` reports nothing and notes the move.
fn assert_credited(case: &str, run: &Run) {
    assert_eq!(reported(run), Vec::new(), "{case}: {}", run.stdout);
    assert!(
        notes(run).contains(MOVED_NOTE),
        "{case}: no note of the move: {}",
        notes(run)
    );
}

/// Asserts that `run` reports the test of `src/lib.rs` and reads nothing as moved.
fn assert_dropped(case: &str, run: &Run) {
    assert_eq!(reported(run), drop_in_lib(), "{case}: {}", run.stdout);
    assert!(
        !notes(run).contains(MOVED_NOTE),
        "{case}: a move was read: {}",
        notes(run)
    );
}

/// The helper moves unchanged to a sibling module declared `#[cfg(test)] mod
/// test_support;`, and the test still calls it: nothing is lost, however the test's
/// module reaches the helper.
#[test]
fn a_helper_moved_unchanged_to_a_sibling_module_still_stands_for_its_checks() {
    let sibling = support(CHECK_3);
    let reached: [(&str, &str, &str); 6] = [
        ("a glob import", "use super::test_support::*;\n", CALL),
        ("a named import", "use super::test_support::check;\n", CALL),
        (
            "a glob import from the crate root",
            "use crate::test_support::*;\n",
            CALL,
        ),
        (
            "a path through the parent's names",
            "",
            "test_support::check(r);",
        ),
        (
            "a path from the crate root",
            "",
            "crate::test_support::check(r);",
        ),
        (
            "an import under another name",
            "use super::test_support::check as verify;\n",
            "verify(r);",
        ),
    ];
    for (case, imports, calls) in reached {
        let lib = module(DECLARED, imports, "", calls);
        let run = moved(&lib, &[("src/test_support.rs", &sibling)]);
        assert_credited(case, &run);
    }
}

/// The shape of the change that showed the gap: tests of a module file split into
/// submodules, the helper in a `test_support` submodule beside them, reached by a glob
/// import written from the crate root. The helper checks through `unwrap`, which counts
/// in test code only: the `#[cfg(test)]` on the `mod` declaration is what makes it so.
#[test]
fn a_helper_moved_to_a_submodule_of_a_nested_module_still_stands_for_its_checks() {
    let unwraps = "fn parsed(text: &str) -> u32 {\n    let first = text.split(',').next().unwrap();\n    first.trim().parse::<u32>().unwrap()\n}\n";
    let test = |imports: &str, helper: &str| {
        format!(
            "pub fn double(x: u32) -> u32 {{\n    x * 2\n}}\n\n#[cfg(test)]\nmod tests {{\n    use super::*;\n{}\n{}    #[test]\n    fn doubles() {{\n        assert_eq!(double(parsed(\"2, 3\")), 4);\n        assert_eq!(double(parsed(\"4\")), 8);\n    }}\n}}\n",
            indented(imports),
            indented(helper),
        )
    };
    let lib = "pub mod calc;\n";
    let base_calc = test("", unwraps);
    let head_calc = format!(
        "#[cfg(test)]\nmod test_support;\n\n{}",
        test("use crate::calc::test_support::*;\n", "")
    );
    let run = change(
        &[("src/lib.rs", lib), ("src/calc.rs", &base_calc)],
        &[
            ("src/lib.rs", lib),
            ("src/calc.rs", &head_calc),
            ("src/calc/test_support.rs", &format!("pub(super) {unwraps}")),
        ],
    );
    assert_eq!(reported(&run), Vec::new(), "{}", run.stdout);
    assert!(notes(&run).contains(MOVED_NOTE), "{}", notes(&run));

    // The tests leave the file too, for a submodule beside the helper's.
    let split = |helper: &str| {
        change(
            &[("src/lib.rs", lib), ("src/calc.rs", &base_calc)],
            &[
                ("src/lib.rs", lib),
                (
                    "src/calc.rs",
                    "mod ops;\n#[cfg(test)]\nmod test_support;\n\npub use ops::double;\n",
                ),
                (
                    "src/calc/ops.rs",
                    &test("use crate::calc::test_support::*;\n", ""),
                ),
                ("src/calc/test_support.rs", &format!("pub(super) {helper}")),
            ],
        )
    };
    let run = split(unwraps);
    assert_eq!(reported(&run), Vec::new(), "{}", run.stdout);
    assert!(notes(&run).contains(MOVED_NOTE), "{}", notes(&run));
    // Control: the helper keeps one of its two `unwrap`s.
    let weaker = unwraps.replace(".next().unwrap()", ".next().unwrap_or(\"0\")");
    let run = split(&weaker);
    assert_eq!(
        reported(&run),
        vec![(DECREASED.to_string(), "src/calc/ops.rs".to_string())],
        "{}",
        run.stdout
    );
}

/// The helper loses an assertion, or all of them, in the move: the test checks less and
/// is reported, with what the moved helper still holds counted in.
#[test]
fn a_helper_that_lost_checks_in_the_move_is_still_a_drop() {
    for (case, helper) in [("one of three lost", CHECK_2), ("all lost", CHECK_0)] {
        let lib = module(DECLARED, "use super::test_support::*;\n", "", CALL);
        let run = moved(&lib, &[("src/test_support.rs", &support(helper))]);
        assert_eq!(reported(&run), drop_in_lib(), "{case}: {}", run.stdout);
    }
}

/// The helper moves unchanged and the test stops calling it: the helper's checks no
/// longer run in the test.
#[test]
fn a_moved_helper_the_test_no_longer_calls_is_a_drop() {
    let lib = module(DECLARED, "use super::test_support::*;\n", "", "let _ = r;");
    let run = moved(&lib, &[("src/test_support.rs", &support(CHECK_3))]);
    assert_dropped("the call is gone", &run);
}

/// The call resolves to a `check` that holds nothing; a `check` holding all three
/// assertions sits in a module the call does not go through, in a file that sorts before
/// the imported one. The name alone names both: only the imported one is the helper the
/// test runs.
#[test]
fn a_helper_of_the_same_name_in_another_module_does_not_stand_in() {
    let lib = module(
        "#[cfg(test)]\nmod aaa_unrelated;\n#[cfg(test)]\nmod test_support;\n\n",
        "use super::test_support::*;\n",
        "",
        CALL,
    );
    let run = moved(
        &lib,
        &[
            ("src/aaa_unrelated.rs", &support(CHECK_3)),
            ("src/test_support.rs", &support(CHECK_0)),
        ],
    );
    assert_dropped("the imported helper holds nothing", &run);

    // The same two helpers with the import naming the one that holds the checks.
    let lib = module(
        "#[cfg(test)]\nmod aaa_unrelated;\n#[cfg(test)]\nmod test_support;\n\n",
        "use super::aaa_unrelated::*;\n",
        "",
        CALL,
    );
    let run = moved(
        &lib,
        &[
            ("src/aaa_unrelated.rs", &support(CHECK_3)),
            ("src/test_support.rs", &support(CHECK_0)),
        ],
    );
    assert_credited("the imported helper holds the checks", &run);

    // The other `check` is new in a test-support file, where helpers are paired by name:
    // the call is the imported helper's, and is not counted a second time there.
    let lib = module(DECLARED, "use super::test_support::*;\n", "", CALL);
    let run = moved(
        &lib,
        &[
            ("src/test_support.rs", &support(CHECK_2)),
            ("tests/common/mod.rs", &format!("pub {CHECK_3}")),
        ],
    );
    assert_eq!(
        reported(&run),
        drop_in_lib(),
        "a same-named helper in a test-support file: {}",
        run.stdout
    );
}

/// The helper moves to a module the crate declares and the test's module does not
/// import, or to a file no `mod` declaration names: the call does not resolve to it.
#[test]
fn a_helper_in_a_module_the_test_does_not_import_is_a_drop() {
    let lib = module(DECLARED, "", "", CALL);
    let run = moved(&lib, &[("src/test_support.rs", &support(CHECK_3))]);
    assert_dropped("declared and not imported", &run);

    let lib = module("", "use super::test_support::*;\n", "", CALL);
    let run = moved(&lib, &[("src/test_support.rs", &support(CHECK_3))]);
    assert_dropped("imported and not declared", &run);
}

/// What the resolution does not follow is left as a drop: a module whose file is named
/// by a `#[path]` attribute, two glob imports that both hold the name, and a name the
/// test binds itself.
#[test]
fn a_call_that_cannot_be_resolved_to_one_helper_is_a_drop() {
    // The module's file is the one `#[path]` names, which holds nothing; a file with the
    // module's own name holds the three assertions and is not part of the crate.
    let lib = module(
        "#[cfg(test)]\n#[path = \"support_impl.rs\"]\nmod test_support;\n\n",
        "use super::test_support::*;\n",
        "",
        CALL,
    );
    let run = moved(
        &lib,
        &[
            ("src/support_impl.rs", &support(CHECK_0)),
            ("src/test_support.rs", &support(CHECK_3)),
        ],
    );
    assert_dropped("a #[path] module", &run);

    let lib = module(
        "#[cfg(test)]\nmod other_support;\n#[cfg(test)]\nmod test_support;\n\n",
        "use super::other_support::*;\nuse super::test_support::*;\n",
        "",
        CALL,
    );
    let run = moved(
        &lib,
        &[
            ("src/other_support.rs", &support(CHECK_3)),
            ("src/test_support.rs", &support(CHECK_3)),
        ],
    );
    assert_dropped("two glob imports hold the name", &run);

    let lib = module(
        DECLARED,
        "use super::test_support::*;\n",
        "",
        "let check = |_: &R| ();\n        check(r);",
    );
    let run = moved(&lib, &[("src/test_support.rs", &support(CHECK_3))]);
    assert_dropped("a closure of the test has the name", &run);

    // A call no execution reaches is not one the test makes.
    let lib = module(
        DECLARED,
        "use super::test_support::*;\n",
        "",
        "if r.f0 == 0 {\n            return;\n            #[allow(unreachable_code)]\n            check(r);\n        }",
    );
    let run = moved(&lib, &[("src/test_support.rs", &support(CHECK_3))]);
    assert_dropped("the call is unreachable", &run);
}

/// A helper of the test's own file with the same name is the one the pack counted for
/// the call, whatever path the call is written with: the helper of the other module is
/// not counted beside it.
#[test]
fn a_helper_of_the_tests_own_file_with_the_name_keeps_the_call() {
    let lib = module(DECLARED, "", CHECK_0, "test_support::check(r);");
    let run = moved(&lib, &[("src/test_support.rs", &support(CHECK_3))]);
    assert_dropped("a same-file helper has the name", &run);
}

/// A helper that moves to a test-support path (a `testutil` directory) is paired by
/// name with the helpers of the change, as before: the note names the helper alone.
#[test]
fn a_helper_moved_to_a_test_support_path_is_left_to_the_pairing_of_helpers() {
    let lib = module(
        "#[cfg(test)]\nmod testutil;\n\n",
        "use super::testutil::*;\n",
        "",
        CALL,
    );
    let run = moved(&lib, &[("src/testutil/mod.rs", &support(CHECK_3))]);
    assert_eq!(reported(&run), Vec::new(), "{}", run.stdout);
    assert!(
        notes(&run).contains("read as moved into helper `check` (3 check(s))"),
        "{}",
        notes(&run)
    );
}

/// A staged check reads the head side from the index: the moved helper is found there.
#[test]
fn a_staged_move_is_read_from_the_index() {
    let stage = |support_text: &str| {
        let repo = Repo::new();
        repo.git(&["checkout", "-q", "main"]);
        repo.write("discipline.toml", CONFIG_HEAD);
        repo.write("src/lib.rs", &base());
        repo.commit("test: base");
        repo.git(&["checkout", "-q", "-B", "work"]);
        let lib = module(DECLARED, "use super::test_support::*;\n", "", CALL);
        repo.write("src/lib.rs", &lib);
        repo.write("src/test_support.rs", support_text);
        repo.git(&["add", "-A"]);
        repo.check(&["--staged"])
    };
    assert_credited("staged whole", &stage(&support(CHECK_3)));
    let run = stage(&support(CHECK_2));
    assert_eq!(reported(&run), drop_in_lib(), "{}", run.stdout);
}

/// A function of a module that is built outside tests too is the code under test, not a
/// test helper: its assertions do not stand for the ones a test dropped.
#[test]
fn a_function_of_a_module_built_outside_tests_does_not_stand_in() {
    let lib = module(
        "mod test_support;\n\n",
        "use super::test_support::*;\n",
        "",
        CALL,
    );
    let run = moved(&lib, &[("src/test_support.rs", &support(CHECK_3))]);
    assert_dropped("no #[cfg(test)] on the declaration", &run);

    // The file gates itself: `#![cfg(test)]` at its top.
    let gated = format!("#![cfg(test)]\n{}", support(CHECK_3));
    let run = moved(&lib, &[("src/test_support.rs", &gated)]);
    assert_credited("#![cfg(test)] in the file", &run);
}

/// A helper the test already called in the sibling module stands for nothing new: the
/// assertions the test drops beside the call are a drop. Calling it once more stands for
/// its checks once more.
#[test]
fn a_sibling_helper_the_test_already_called_stands_for_no_dropped_assertion() {
    let with = |own: usize, calls: usize| {
        let body: String = (0..own)
            .map(|i| format!("assert_eq!(r.f{i}, {i});\n        "))
            .chain((0..calls).map(|_| format!("{CALL}\n        ")))
            .collect();
        module(DECLARED, "use super::test_support::*;\n", "", body.trim())
    };
    let sibling = support(CHECK_3);
    let run = change(
        &[
            ("src/lib.rs", &with(3, 1)),
            ("src/test_support.rs", &sibling),
        ],
        &[
            ("src/lib.rs", &with(0, 1)),
            ("src/test_support.rs", &sibling),
        ],
    );
    assert_dropped("called once on both sides", &run);

    let run = change(
        &[
            ("src/lib.rs", &with(3, 1)),
            ("src/test_support.rs", &sibling),
        ],
        &[
            ("src/lib.rs", &with(0, 2)),
            ("src/test_support.rs", &sibling),
        ],
    );
    assert_credited("called once more", &run);

    // The call to the sibling helper is replaced by one to a new helper of a test-support
    // file, and the test's own assertions go: the new helper stands for the call that
    // left, not for the assertions as well.
    let replaced = module(DECLARED, "use super::test_support::*;\n", "", "verify(r);");
    let verify = format!("pub {}", CHECK_3.replace("fn check", "fn verify"));
    let run = change(
        &[
            ("src/lib.rs", &with(3, 1)),
            ("src/test_support.rs", &sibling),
        ],
        &[
            ("src/lib.rs", &replaced),
            ("src/test_support.rs", &sibling),
            ("tests/common/mod.rs", &verify),
        ],
    );
    assert_eq!(
        reported(&run),
        drop_in_lib(),
        "the call replaced by another helper's: {}",
        run.stdout
    );

    // The base test already called the helper through a module that is not followed
    // (`#[path]`); on the head side the same call resolves. It is not a new call.
    let by_path = with(3, 1).replace(
        DECLARED,
        "#[cfg(test)]\n#[path = \"test_support.rs\"]\nmod test_support;\n\n",
    );
    let run = change(
        &[("src/lib.rs", &by_path), ("src/test_support.rs", &sibling)],
        &[
            ("src/lib.rs", &with(0, 1)),
            ("src/test_support.rs", &sibling),
        ],
    );
    assert_dropped("resolved on the head side only", &run);
}

/// #688: a helper moved out of a test-support path into a non-test-support path
/// stands for nothing and is reported as deleted.
#[test]
fn a_helper_moved_to_a_non_test_support_path_is_reported_as_deleted() {
    let helper = "def check(r):\n    assert r.a == 1\n    assert r.b == 2\n";
    let test = "def test_it():\n    check(r)\n";
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", CONFIG_HEAD);
    repo.write("tests/helpers.py", helper);
    repo.write("tests/test_app.py", test);
    repo.commit("test: base");

    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.git(&["rm", "-q", "tests/helpers.py"]);
    repo.write("src/app_helpers.py", helper);
    repo.commit("refactor: move helper to non-test-support file");

    let run = repo.check(&["--base", "main"]);
    let violations = reported(&run);
    assert_eq!(
        violations,
        vec![(
            "Test Helper Function Weakened".to_string(),
            "tests/helpers.py".to_string()
        )],
        "{}",
        run.stdout
    );
}
