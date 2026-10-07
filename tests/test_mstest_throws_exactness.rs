//! MSTest's `Assert.Throws<T>` (#626, #649). It exists from MSTest 3.8 only, and accepts
//! subclasses; xUnit's and NUnit's `Assert.Throws<T>` is exact. Whose `Assert` a file's is
//! comes from its `using` directives: (a) in an xUnit or NUnit file `Assert.Throws<T>` is
//! exact; (b) in an MSTest file it accepts subclasses, so `Assert.ThrowsExactly<T>` or
//! `Assert.ThrowsException<T>` replaced by it is a loss of exactness; (c) in a file whose
//! directives name no framework, the calls decide: `Assert.Throws<T>` accepts subclasses
//! beside an `Assert.ThrowsExactly`, and opposite an `Assert.ThrowsExactly` or an
//! `Assert.ThrowsException` on the other side of the change.

mod common;
use common::{Repo, Run, CONFIG_HEAD};

const WIDENED: &str = "Expected Exception Or Panic Widened";
const FILE: &str = "tests/SutTests.cs";

const MSTEST: &str = "using Microsoft.VisualStudio.TestTools.UnitTesting;";
const XUNIT: &str = "using Xunit;";
/// No directive names a test framework (they are global usings of the project).
const NO_FRAMEWORK: &str = "using System;";

fn file(using: &str, bodies: &[&str]) -> String {
    let methods: String = bodies
        .iter()
        .enumerate()
        .map(|(i, body)| {
            format!("    [TestMethod]\n    public void Rejects{i}() {{\n        {body}\n    }}\n\n")
        })
        .collect();
    format!("{using}\n\n[TestClass]\npublic class SutTests {{\n{methods}}}\n")
}

/// A change inside an MSTest file.
fn change(base: &[&str], head: &[&str]) -> Run {
    change_in((MSTEST, base), (MSTEST, head))
}

/// A change from `(using, bodies)` to `(using, bodies)`.
fn change_in(base: (&str, &[&str]), head: (&str, &[&str])) -> Run {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", CONFIG_HEAD);
    repo.write(FILE, &file(base.0, base.1));
    repo.commit("test: base");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write(FILE, &file(head.0, head.1));
    repo.commit("refactor: change");
    repo.check(&["--base", "main"])
}

/// The message of each expected-exception finding.
fn widened(run: &Run) -> Vec<String> {
    run.violations("assertion-reduction")
        .iter()
        .filter(|v| v["title"] == WIDENED)
        .map(|v| v["message"].as_str().unwrap_or("").to_string())
        .collect()
}

const EXACTLY: &str = "Assert.ThrowsExactly<ArgumentException>(() => sut.Run());";
const THROWS: &str = "Assert.Throws<ArgumentException>(() => sut.Run());";
const THROWS_NULL: &str = "Assert.Throws<ArgumentNullException>(() => sut.Run());";
const LEGACY: &str = "Assert.ThrowsException<ArgumentException>(() => sut.Run());";
const OTHER_EXACTLY: &str = "Assert.ThrowsExactly<FormatException>(() => sut.Parse());";

#[test]
fn throws_exactly_replaced_by_throws_is_a_loss_of_exactness() {
    let run = change(&[EXACTLY], &[THROWS]);
    let got = widened(&run);
    assert_eq!(got.len(), 1, "{}", run.stdout);
    assert!(
        got[0].contains("no longer checked as the exact type"),
        "{}",
        got[0]
    );

    // Beside a site the change does not touch: that one is not reported.
    let run = change(&[EXACTLY, THROWS_NULL], &[THROWS, THROWS_NULL]);
    assert_eq!(widened(&run).len(), 1, "{}", run.stdout);

    // Control: the reverse is a narrowing, and an unchanged site is unchanged. Nothing
    // at all is reported for it: the two spellings are the same assertion to the counts.
    let run = change(&[THROWS], &[EXACTLY]);
    assert_eq!(
        run.violations("assertion-reduction").len(),
        0,
        "{}",
        run.stdout
    );
    let run = change(&[LEGACY], &[EXACTLY]);
    assert_eq!(
        run.violations("assertion-reduction").len(),
        0,
        "{}",
        run.stdout
    );
    let run = change(&[EXACTLY], &[EXACTLY, "sut.Reset();"]);
    assert_eq!(widened(&run), Vec::<String>::new(), "{}", run.stdout);
}

/// `Assert.Throws<T>` accepts subclasses in an MSTest file, with `Assert.ThrowsExactly`
/// in the file or without (b), and in a file of no named framework that uses
/// `Assert.ThrowsExactly` (c): a move to the parent class is a widening. In an xUnit
/// file (a), and in a file that names no framework and never calls
/// `Assert.ThrowsExactly` (c), the same move is between two exact classes.
#[test]
fn throws_accepts_subclasses_in_an_mstest_file_and_beside_throws_exactly() {
    let run = change(&[THROWS_NULL, OTHER_EXACTLY], &[THROWS, OTHER_EXACTLY]);
    let got = widened(&run);
    assert_eq!(got.len(), 1, "{}", run.stdout);
    assert!(
        got[0].contains("from `ArgumentNullException` to `ArgumentException`"),
        "{}",
        got[0]
    );

    // (b) without `Assert.ThrowsExactly` anywhere in the file.
    let run = change(&[THROWS_NULL], &[THROWS]);
    let got = widened(&run);
    assert_eq!(got.len(), 1, "{}", run.stdout);
    assert!(
        got[0].contains("from `ArgumentNullException` to `ArgumentException`"),
        "{}",
        got[0]
    );

    // (c) the calls decide.
    let run = change_in(
        (NO_FRAMEWORK, &[THROWS_NULL, OTHER_EXACTLY]),
        (NO_FRAMEWORK, &[THROWS, OTHER_EXACTLY]),
    );
    assert_eq!(widened(&run).len(), 1, "{}", run.stdout);
    let run = change_in((NO_FRAMEWORK, &[THROWS_NULL]), (NO_FRAMEWORK, &[THROWS]));
    assert_eq!(widened(&run), Vec::<String>::new(), "{}", run.stdout);

    // (a) exact, also beside a method that calls `Assert.ThrowsExactly`.
    let run = change_in((XUNIT, &[THROWS_NULL]), (XUNIT, &[THROWS]));
    assert_eq!(widened(&run), Vec::<String>::new(), "{}", run.stdout);
    let run = change_in(
        (XUNIT, &[THROWS_NULL, OTHER_EXACTLY]),
        (XUNIT, &[THROWS, OTHER_EXACTLY]),
    );
    assert_eq!(widened(&run), Vec::<String>::new(), "{}", run.stdout);
}

/// The name appearing on one side only does not change what an untouched
/// `Assert.Throws<T>` is. `Assert.ThrowsException<T>` is exact in every version and a
/// name only MSTest has: replaced by `Assert.Throws<T>` it loses exactness in an MSTest
/// file (b) and in a file that names no framework (c), and keeps it in a file that has
/// become an xUnit file (a), where the change is a move to another framework.
#[test]
fn an_untouched_throws_is_the_same_on_both_sides() {
    let run = change(&[THROWS], &[THROWS, OTHER_EXACTLY]);
    assert_eq!(
        widened(&run),
        Vec::<String>::new(),
        "gained: {}",
        run.stdout
    );
    // The name gone from the head side: the untouched site is not reported.
    let run = change(&[THROWS, OTHER_EXACTLY], &[THROWS, "sut.Reset();"]);
    assert!(!run.stdout.contains("Rejects0"), "lost: {}", run.stdout);
    for (case, using) in [("b", MSTEST), ("c", NO_FRAMEWORK)] {
        let run = change_in((using, &[LEGACY]), (using, &[THROWS]));
        let got = widened(&run);
        assert_eq!(got.len(), 1, "legacy, case {case}: {}", run.stdout);
        assert!(
            got[0].contains("no longer checked as the exact type"),
            "legacy, case {case}: {}",
            got[0]
        );
    }
    let run = change_in((MSTEST, &[LEGACY]), (XUNIT, &[THROWS]));
    assert_eq!(
        widened(&run),
        Vec::<String>::new(),
        "legacy, case a: {}",
        run.stdout
    );
    let run = change(&[EXACTLY], &[LEGACY]);
    assert_eq!(
        widened(&run),
        Vec::<String>::new(),
        "to legacy: {}",
        run.stdout
    );
}
