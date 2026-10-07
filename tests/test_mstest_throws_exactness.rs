//! MSTest's `Assert.Throws<T>` (#626). `Assert.ThrowsExactly<T>` exists from MSTest 3.8
//! only, and from that version `Assert.Throws<T>` accepts subclasses. The version is not
//! written in a test file, so the file's use of `Assert.ThrowsExactly` says it:
//! `Assert.Throws<T>` beside one, on either side of the change, accepts subclasses, and
//! `Assert.ThrowsExactly<T>` replaced by `Assert.Throws<T>` is a loss of exactness. In a
//! file that shows neither side using it, `Assert.Throws<T>` is exact as xUnit's is.

mod common;
use common::{Repo, Run, CONFIG_HEAD};

const WIDENED: &str = "Expected Exception Or Panic Widened";
const FILE: &str = "tests/SutTests.cs";

fn file(bodies: &[&str]) -> String {
    let methods: String = bodies
        .iter()
        .enumerate()
        .map(|(i, body)| {
            format!("    [TestMethod]\n    public void Rejects{i}() {{\n        {body}\n    }}\n\n")
        })
        .collect();
    format!("using Microsoft.VisualStudio.TestTools.UnitTesting;\n\n[TestClass]\npublic class SutTests {{\n{methods}}}\n")
}

fn change(base: &[&str], head: &[&str]) -> Run {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", CONFIG_HEAD);
    repo.write(FILE, &file(base));
    repo.commit("test: base");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write(FILE, &file(head));
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

/// In a file that uses `Assert.ThrowsExactly`, `Assert.Throws<T>` accepts subclasses: a
/// move to the parent class is a widening. Without that name on either side the same
/// move is between two exact classes, as it is for xUnit, and is not reported.
#[test]
fn throws_accepts_subclasses_only_beside_throws_exactly() {
    let run = change(&[THROWS_NULL, OTHER_EXACTLY], &[THROWS, OTHER_EXACTLY]);
    let got = widened(&run);
    assert_eq!(got.len(), 1, "{}", run.stdout);
    assert!(
        got[0].contains("from `ArgumentNullException` to `ArgumentException`"),
        "{}",
        got[0]
    );

    let run = change(&[THROWS_NULL], &[THROWS]);
    assert_eq!(widened(&run), Vec::<String>::new(), "{}", run.stdout);
}

/// The name appearing on one side only does not change what an untouched
/// `Assert.Throws<T>` is, and `Assert.ThrowsException<T>`, exact in every version, is
/// not a sign of the version: replaced by `Assert.Throws<T>` it stays unreported.
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
    let run = change(&[LEGACY], &[THROWS]);
    assert_eq!(
        widened(&run),
        Vec::<String>::new(),
        "legacy: {}",
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
