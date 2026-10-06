//! `assertion-reduction`: the case-counting decisions and the property-test bodies of
//! issue 596, driven through the real binary: one base commit on `main`, one change on
//! `work`. Each behaviour has the shape that must be reported and the neighbouring shape
//! that must not.

mod common;
use common::{Repo, Run};

const GATE: &str = "assertion-reduction";
const CASES_REDUCED: &str = "assertion-reduction/test-cases-reduced";

/// Commits `base` at `path` on `main`, `head` on `work`, and checks the change.
fn judge(path: &str, base: &str, head: &str) -> Run {
    assert_ne!(base, head, "the change must change the file");
    let repo = Repo::new();
    repo.commit_base(path, base, "test: base");
    repo.write(path, head);
    repo.commit("test: change");
    repo.check(&[])
}

/// The codes of the findings the gate reported.
fn codes(run: &Run) -> Vec<String> {
    run.violations(GATE)
        .iter()
        .map(|v| v["code"].as_str().unwrap().to_string())
        .collect()
}

/// The gate's notes, as one string.
fn notes(run: &Run) -> String {
    run.outcome(GATE)["notes"].to_string()
}

/// The run reports exactly one finding of the gate, a case reduction whose message
/// contains `fragment`.
#[track_caller]
fn assert_reduced(run: &Run, fragment: &str) {
    let violations = run.violations(GATE);
    assert_eq!(
        violations.len(),
        1,
        "expected one finding: {violations:#?}\nnotes: {}\n{}",
        notes(run),
        run.stderr
    );
    assert_eq!(violations[0]["code"], CASES_REDUCED, "{violations:#?}");
    let message = violations[0]["message"].as_str().unwrap();
    assert!(
        message.contains(fragment),
        "expected `{fragment}` in: {message}"
    );
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
}

/// The run reports exactly one finding of the gate, with this code, on this line.
#[track_caller]
fn assert_one(run: &Run, code: &str, line: Option<u64>) {
    let violations = run.violations(GATE);
    assert_eq!(
        codes(run),
        [format!("{GATE}/{code}")],
        "{violations:#?}\n{}",
        run.stderr
    );
    if let Some(line) = line {
        assert_eq!(violations[0]["line"], line, "{violations:#?}");
    }
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
}

/// The run reports nothing from the gate and passes.
#[track_caller]
fn assert_clean(run: &Run) {
    let violations = run.violations(GATE);
    assert!(
        violations.is_empty(),
        "expected no finding: {violations:#?}"
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
}

// ---------------------------------------------------------------------------
// Decision 1: rows moved to another test of the file are matched by content
// ---------------------------------------------------------------------------

/// A file with one parametrized test, and room for a second one: `@A@` is the first
/// test's rows, `@B@` the second's.
struct Table {
    path: &'static str,
    first: &'static str,
    second: &'static str,
    tail: &'static str,
    /// How one integer is written as a row, and what stands between two rows.
    row: (&'static str, &'static str, &'static str),
}

impl Table {
    fn rows(&self, values: &[i32]) -> String {
        let (open, close, between) = self.row;
        values
            .iter()
            .map(|v| format!("{open}{v}{close}"))
            .collect::<Vec<_>>()
            .join(between)
    }

    fn file(&self, first: &[i32], second: Option<&[i32]>) -> String {
        let mut out = self.first.replace("@A@", &self.rows(first));
        if let Some(second) = second {
            out.push_str(&self.second.replace("@B@", &self.rows(second)));
        }
        out.push_str(self.tail);
        out
    }
}

const PYTHON: Table = Table {
    path: "tests/test_calc.py",
    first: "\
import pytest

@pytest.mark.parametrize(\"x\", [@A@])
def test_x(x):
    assert x > 0
",
    second: "
@pytest.mark.parametrize(\"x\", [@B@])
def test_other(x):
    assert x > 0
",
    tail: "",
    row: ("", "", ", "),
};

const JAVASCRIPT: Table = Table {
    path: "tests/calc.test.js",
    first: "\
test.each([@A@])(\"x %i\", (x) => {
  expect(x).toBeGreaterThan(0);
});
",
    second: "
test.each([@B@])(\"other %i\", (x) => {
  expect(x).toBeGreaterThan(0);
});
",
    tail: "",
    row: ("[", "]", ", "),
};

const TYPESCRIPT: Table = Table {
    path: "tests/calc.test.ts",
    first: "\
test.each([@A@])(\"x %i\", (x: number) => {
  expect(x).toBeGreaterThan(0);
});
",
    second: "
test.each([@B@])(\"other %i\", (x: number) => {
  expect(x).toBeGreaterThan(0);
});
",
    tail: "",
    row: ("[", "]", ", "),
};

const GO: Table = Table {
    path: "calc_test.go",
    first: "\
package calc

import \"testing\"

func TestX(t *testing.T) {
	for _, c := range []struct{ n int }{@A@} {
		if c.n <= 0 {
			t.Fatalf(\"bad %v\", c)
		}
	}
}
",
    second: "
func TestOther(t *testing.T) {
	for _, c := range []struct{ n int }{@B@} {
		if c.n <= 0 {
			t.Fatalf(\"bad %v\", c)
		}
	}
}
",
    tail: "",
    row: ("{", "}", ", "),
};

const JAVA: Table = Table {
    path: "src/test/java/CalcTest.java",
    first: "\
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.ValueSource;
import static org.junit.jupiter.api.Assertions.assertTrue;

class CalcTest {
    @ParameterizedTest
    @ValueSource(ints = {@A@})
    void positive(int x) {
        assertTrue(x > 0);
    }
",
    second: "
    @ParameterizedTest
    @ValueSource(ints = {@B@})
    void other(int x) {
        assertTrue(x > 0);
    }
",
    tail: "}\n",
    row: ("", "", ", "),
};

const KOTLIN: Table = Table {
    path: "src/test/kotlin/CalcTest.kt",
    first: "\
import org.junit.jupiter.params.ParameterizedTest
import org.junit.jupiter.params.provider.ValueSource
import kotlin.test.assertTrue

class CalcTest {
    @ParameterizedTest
    @ValueSource(ints = [@A@])
    fun positive(x: Int) {
        assertTrue(x > 0)
    }
",
    second: "
    @ParameterizedTest
    @ValueSource(ints = [@B@])
    fun other(x: Int) {
        assertTrue(x > 0)
    }
",
    tail: "}\n",
    row: ("", "", ", "),
};

const CSHARP: Table = Table {
    path: "tests/CalcTest.cs",
    first: "\
using Xunit;

public class CalcTest {
    [Theory]
@A@
    public void Positive(int x) {
        Assert.True(x > 0);
    }
",
    second: "
    [Theory]
@B@
    public void Other(int x) {
        Assert.True(x > 0);
    }
",
    tail: "}\n",
    row: ("    [InlineData(", ")]", "\n"),
};

const RUST: Table = Table {
    path: "tests/calc.rs",
    first: "\
use rstest::rstest;

#[rstest]
@A@
fn positive(#[case] x: i32) {
    assert!(x > 0);
}
",
    second: "
#[rstest]
@B@
fn other(#[case] x: i32) {
    assert!(x > 0);
}
",
    tail: "",
    row: ("#[case(", ")]", "\n"),
};

const TABLES: [(&str, &Table); 8] = [
    ("python", &PYTHON),
    ("javascript", &JAVASCRIPT),
    ("typescript", &TYPESCRIPT),
    ("go", &GO),
    ("java", &JAVA),
    ("kotlin", &KOTLIN),
    ("csharp", &CSHARP),
    ("rust", &RUST),
];

/// Three cases leave a test while an unrelated test with three other cases arrives in
/// the file: the file's total is unchanged and nothing moved.
#[test]
fn cases_dropped_beside_an_unrelated_new_test_are_reported() {
    for (language, table) in TABLES {
        let base = table.file(&[1, 2, 3, 4], None);
        let head = table.file(&[1], Some(&[7, 8, 9]));
        let run = judge(table.path, &base, &head);
        let violations = run.violations(GATE);
        assert_eq!(
            codes(&run),
            [CASES_REDUCED],
            "{language}: {violations:#?}\nnotes: {}",
            notes(&run)
        );
        let message = violations[0]["message"].as_str().unwrap();
        assert!(
            message.contains("dropped from 4 to 1"),
            "{language}: {message}"
        );
        assert!(
            notes(&run).contains(
                "0 of 3 dropped case(s) found moved to another test of the file, 3 not found"
            ),
            "{language}: {}",
            notes(&run)
        );
        assert_eq!(run.code, 1, "{language}: {}", run.stdout);
    }
}

/// Control: the three cases that leave the test arrive, as written, in a new test of the
/// file. One that arrives changed is not the one that left.
#[test]
fn cases_moved_to_another_test_of_the_file_are_a_note() {
    for (language, table) in TABLES {
        let base = table.file(&[1, 2, 3, 4], None);
        let head = table.file(&[1], Some(&[2, 3, 4]));
        let run = judge(table.path, &base, &head);
        assert!(
            run.violations(GATE).is_empty(),
            "{language}: {:#?}",
            run.violations(GATE)
        );
        assert_eq!(run.code, 0, "{language}: {}{}", run.stdout, run.stderr);
        assert!(
            notes(&run).contains(
                "read as preserved across tests in same file (3 of 3 dropped case(s) found moved to another test of the file, 0 not found)"
            ),
            "{language}: {}",
            notes(&run)
        );

        let head = table.file(&[1], Some(&[2, 3, 9]));
        let run = judge(table.path, &base, &head);
        assert_eq!(codes(&run), [CASES_REDUCED], "{language}: {}", notes(&run));
        assert!(
            notes(&run).contains(
                "2 of 3 dropped case(s) found moved to another test of the file, 1 not found"
            ),
            "{language}: {}",
            notes(&run)
        );
    }
}

const PY_SPACED_ROWS: &str = "\
import pytest

@pytest.mark.parametrize(\"a,b\", [(1, 2), (2, 3), (3, 4)])
def test_x(a, b):
    assert a < b
";

const PY_SPACED_ROWS_MOVED: &str = "\
import pytest

@pytest.mark.parametrize(\"a,b\", [(1, 2)])
def test_x(a, b):
    assert a < b

@pytest.mark.parametrize(\"a,b\", [
    (2,3),  # the middle one
    (
        3,
        4,
    ),
])
def test_other(a, b):
    assert a < b
";

/// A moved row is the same row whatever its layout: whitespace, a trailing comma and a
/// comment beside it are not part of it.
#[test]
fn a_moved_case_is_matched_whatever_its_layout() {
    let run = judge("tests/test_calc.py", PY_SPACED_ROWS, PY_SPACED_ROWS_MOVED);
    assert_clean(&run);
    assert!(
        notes(&run).contains("2 of 2 dropped case(s) found moved"),
        "{}",
        notes(&run)
    );

    // The same rows with one value changed did not move.
    let run = judge(
        "tests/test_calc.py",
        PY_SPACED_ROWS,
        &PY_SPACED_ROWS_MOVED.replace("(2,3)", "(2,30)"),
    );
    assert_reduced(&run, "dropped from 3 to 1");
}

const PY_NAMED_LIST: &str = "
@pytest.mark.parametrize(\"x\", MORE)
def test_other(x):
    assert x > 0
";

/// A case list that is not literal cannot be compared: it does not excuse a drop in
/// another test, and a test that turns to one is not excused by the rows of another.
#[test]
fn a_non_literal_case_list_neither_supplies_nor_receives_the_excusal() {
    let base = PYTHON.file(&[1, 2, 3, 4], None);
    let head = PYTHON.file(&[1], None) + PY_NAMED_LIST;
    let run = judge(PYTHON.path, &base, &head);
    assert_reduced(&run, "dropped from 4 to 1");

    let head = PYTHON
        .file(&[1, 2, 3, 4], None)
        .replace("[1, 2, 3, 4]", "MORE")
        + &PYTHON.second.replace("@B@", "1, 2, 3, 4");
    let run = judge(PYTHON.path, &base, &head);
    assert_reduced(&run, "no longer a literal list");
}

const PY_UNPARAMETRIZED: &str = "\
import pytest

def test_x():
    assert 1 > 0

@pytest.mark.parametrize(\"x\", [1, 2, 3, 4])
def test_x_table(x):
    assert x > 0
";

/// Control: a test that stops being parametrized while its rows arrive in a new test.
#[test]
fn rows_of_a_test_no_longer_parametrized_can_move_to_another_test() {
    let base = PYTHON.file(&[1, 2, 3, 4], None);
    let run = judge(PYTHON.path, &base, PY_UNPARAMETRIZED);
    assert_eq!(
        codes(&run).iter().filter(|c| *c == CASES_REDUCED).count(),
        0,
        "{:#?}",
        run.violations(GATE)
    );
    assert!(
        notes(&run).contains("4 of 4 dropped case(s) found moved"),
        "{}",
        notes(&run)
    );

    let run = judge(
        PYTHON.path,
        &base,
        &PY_UNPARAMETRIZED.replace("[1, 2, 3, 4]", "[5, 6, 7, 8]"),
    );
    assert!(
        codes(&run).contains(&CASES_REDUCED.to_string()),
        "{:#?}",
        run.violations(GATE)
    );
}

// ---------------------------------------------------------------------------
// Decision 2: a list holding a spread or splat element is not a literal list
// ---------------------------------------------------------------------------

const PY_VALUES: &str = "\
import pytest

MORE = [5, 6]

@pytest.mark.parametrize(\"x\", [1, 2, 3])
def test_x(x):
    assert x > 0
";

#[test]
fn python_splat_makes_the_case_list_non_literal() {
    // Rows replaced by a splat.
    let run = judge(
        "tests/test_calc.py",
        PY_VALUES,
        &PY_VALUES.replace("[1, 2, 3]", "[1, *MORE]"),
    );
    assert_reduced(&run, "no longer a literal list");

    // Every row kept, a splat added: the list can no longer be counted.
    let run = judge(
        "tests/test_calc.py",
        PY_VALUES,
        &PY_VALUES.replace("[1, 2, 3]", "[1, 2, 3, *MORE]"),
    );
    assert_reduced(&run, "no longer a literal list");

    // Controls: non-literal on both sides, and a splat removed from a list that had one.
    let with_splat = PY_VALUES.replace("[1, 2, 3]", "[1, 2, *MORE]");
    let run = judge(
        "tests/test_calc.py",
        &with_splat,
        &PY_VALUES.replace("[1, 2, 3]", "[*MORE]"),
    );
    assert_clean(&run);
    assert!(
        notes(&run).contains("non-literal test case source"),
        "{}",
        notes(&run)
    );
    let run = judge(
        "tests/test_calc.py",
        &with_splat,
        &PY_VALUES.replace("[1, 2, 3]", "[1, 2]"),
    );
    assert_clean(&run);
}

const JS_VALUES: &str = "\
const more = [[5], [6]];

test.each([[1], [2], [3]])(\"x %i\", (x) => {
  expect(x).toBeGreaterThan(0);
});
";

#[test]
fn javascript_spread_makes_the_case_list_non_literal() {
    for path in ["tests/calc.test.js", "tests/calc.test.ts"] {
        let run = judge(
            path,
            JS_VALUES,
            &JS_VALUES.replace("[[1], [2], [3]]", "[[1], ...more]"),
        );
        assert_reduced(&run, "no longer a literal list");

        let run = judge(
            path,
            JS_VALUES,
            &JS_VALUES.replace("[[1], [2], [3]]", "[[1], [2], [3], ...more]"),
        );
        assert_reduced(&run, "no longer a literal list");

        // Controls: non-literal on both sides, and a spread removed.
        let with_spread = JS_VALUES.replace("[[1], [2], [3]]", "[[1], [2], ...more]");
        let run = judge(
            path,
            &with_spread,
            &JS_VALUES.replace("[[1], [2], [3]]", "[...more]"),
        );
        assert_clean(&run);
        let run = judge(
            path,
            &with_spread,
            &JS_VALUES.replace("[[1], [2], [3]]", "[[1], [2]]"),
        );
        assert_clean(&run);

        // A spread inside a row is that row's own content.
        let run = judge(
            path,
            JS_VALUES,
            &JS_VALUES.replace("[[1], [2], [3]]", "[[1], [...more[0]], [3]]"),
        );
        assert_clean(&run);
    }
}

const KOTLIN_ARRAY: &str = "\
import org.junit.jupiter.params.ParameterizedTest
import org.junit.jupiter.params.provider.ValueSource
import kotlin.test.assertTrue

class CalcTest {
    @ParameterizedTest
    @ValueSource(ints = intArrayOf(1, 2, 3))
    fun positive(x: Int) {
        assertTrue(x > 0)
    }
}
";

/// The precedent the other two follow, pinned: a Kotlin array with a spread.
#[test]
fn kotlin_spread_makes_the_case_list_non_literal() {
    let run = judge(
        "src/test/kotlin/CalcTest.kt",
        KOTLIN_ARRAY,
        &KOTLIN_ARRAY.replace("intArrayOf(1, 2, 3)", "intArrayOf(1, *MORE)"),
    );
    assert_reduced(&run, "no longer a literal list");

    let run = judge(
        "src/test/kotlin/CalcTest.kt",
        KOTLIN_ARRAY,
        &KOTLIN_ARRAY.replace("intArrayOf(1, 2, 3)", "intArrayOf(1, 2, 3, *MORE)"),
    );
    assert_reduced(&run, "no longer a literal list");
}

// ---------------------------------------------------------------------------
// Property tests: what is a test in `proptest!`
// ---------------------------------------------------------------------------

const PROPTEST: &str = "\
use proptest::prelude::*;

proptest! {
    #[test]
    fn commutes(a in 0..10i32, b in 0..10i32) {
        prop_assert_eq!(a + b, b + a);
    }

    #[test]
    fn bounded(a in 0..10i32) {
        prop_assert!(a < 10);
        prop_assert_eq!(digits(a), 1);
    }
}
";

#[test]
fn proptest_function_that_loses_its_test_attribute_is_a_removed_test() {
    let run = judge(
        "tests/prop.rs",
        PROPTEST,
        &PROPTEST.replace("    #[test]\n    fn bounded", "    fn bounded"),
    );
    let removed: Vec<String> = run
        .violations("deletion-rationale")
        .iter()
        .map(|v| v["message"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(removed.len(), 1, "{removed:?}\n{}", run.stdout);
    assert!(removed[0].contains("bounded"), "{removed:?}");
    assert_eq!(run.code, 1, "{}", run.stdout);

    // Control: a function without the attribute arrives beside the tests. The macro
    // does not run it, so it is not a test, vacuous or otherwise.
    let run = judge(
        "tests/prop.rs",
        PROPTEST,
        &PROPTEST.replace(
            "    #[test]\n    fn bounded",
            "    fn not_run(a in 0..10i32) {}\n\n    #[test]\n    fn bounded",
        ),
    );
    assert_clean(&run);
    assert!(run.violations("vacuous-tests").is_empty(), "{}", run.stdout);
    assert!(
        run.violations("deletion-rationale").is_empty(),
        "{}",
        run.stdout
    );
}

// ---------------------------------------------------------------------------
// Property tests: the checks of an ordinary body, in a re-parsed one
// ---------------------------------------------------------------------------

#[test]
fn proptest_body_bound_and_expected_value_are_compared() {
    let run = judge(
        "tests/prop.rs",
        PROPTEST,
        &PROPTEST.replace("prop_assert!(a < 10);", "prop_assert!(a < 1000);"),
    );
    assert_one(&run, "assertion-bound-loosened", Some(11));

    let run = judge(
        "tests/prop.rs",
        PROPTEST,
        &PROPTEST.replace(
            "prop_assert_eq!(digits(a), 1);",
            "prop_assert_eq!(digits(a), 2);",
        ),
    );
    assert_one(&run, "expected-value-changed", Some(12));

    // Controls: a bound tightened, and a line added above with nothing else changed.
    let run = judge(
        "tests/prop.rs",
        PROPTEST,
        &PROPTEST.replace("prop_assert!(a < 10);", "prop_assert!(a < 9);"),
    );
    assert_clean(&run);
    let run = judge(
        "tests/prop.rs",
        PROPTEST,
        &PROPTEST.replace("proptest! {\n", "// Properties.\nproptest! {\n"),
    );
    assert_clean(&run);
}

const PROPTEST_UNWIND: &str = "\
use proptest::prelude::*;

proptest! {
    #[test]
    fn bounded(a in 0..10i32) {
        check(a);
        assert!(a < 10);
    }
}
";

#[test]
fn proptest_body_assertion_caught_by_a_handler_is_reported() {
    let run = judge(
        "tests/prop.rs",
        PROPTEST_UNWIND,
        &PROPTEST_UNWIND.replace(
            "        assert!(a < 10);\n",
            "        let _ = std::panic::catch_unwind(|| {\n            assert!(a < 10);\n        });\n",
        ),
    );
    let found = codes(&run);
    assert!(
        found.contains(&format!("{GATE}/assertion-failure-caught")),
        "{:#?}",
        run.violations(GATE)
    );
    let caught = run
        .violations(GATE)
        .into_iter()
        .find(|v| v["code"] == format!("{GATE}/assertion-failure-caught"))
        .unwrap();
    assert_eq!(caught["line"], 8, "{caught:#?}");
    assert_eq!(run.code, 1);

    // Control: the result of the unwind is checked.
    let run = judge(
        "tests/prop.rs",
        PROPTEST_UNWIND,
        &PROPTEST_UNWIND.replace(
            "        assert!(a < 10);\n",
            "        let r = std::panic::catch_unwind(|| {\n            assert!(a < 10);\n        });\n        assert!(r.is_ok());\n",
        ),
    );
    assert!(
        !codes(&run).contains(&format!("{GATE}/assertion-failure-caught")),
        "{:#?}",
        run.violations(GATE)
    );
}

const PROPTEST_PANIC: &str = "\
use proptest::prelude::*;

proptest! {
    #[test]
    #[should_panic(expected = \"overflow\")]
    fn overflows(a in 0..10i32) {
        add(a, i32::MAX);
    }
}
";

/// Already read before this change; pinned beside the three that were not.
#[test]
fn proptest_should_panic_widened_is_reported() {
    let run = judge(
        "tests/prop.rs",
        PROPTEST_PANIC,
        &PROPTEST_PANIC.replace(
            "#[should_panic(expected = \"overflow\")]",
            "#[should_panic]",
        ),
    );
    assert_one(&run, "expected-exception-widened", None);
}

// ---------------------------------------------------------------------------
// Property tests: the closure form
// ---------------------------------------------------------------------------

const PROPTEST_CLOSURE: &str = "\
use proptest::prelude::*;

#[test]
fn bounded() {
    proptest!(|(a in 0..10i32)| {
        prop_assert!(a < 10);
        prop_assert_eq!(digits(a), 1);
    });
}

#[test]
fn configured() {
    proptest!(ProptestConfig::with_cases(64), move |(a in 0..10i32, b: u8)| {
        prop_assert!(a < 10);
        prop_assert_eq!(digits(a), 1);
    });
}
";

#[test]
fn proptest_closure_body_is_read() {
    // Each of the two tests, gutted.
    for (name, opening) in [
        ("bounded", "    proptest!(|(a in 0..10i32)| {\n"),
        (
            "configured",
            "    proptest!(ProptestConfig::with_cases(64), move |(a in 0..10i32, b: u8)| {\n",
        ),
    ] {
        let full = format!(
            "{opening}        prop_assert!(a < 10);\n        prop_assert_eq!(digits(a), 1);\n"
        );
        assert!(PROPTEST_CLOSURE.contains(&full));
        let run = judge(
            "tests/prop.rs",
            PROPTEST_CLOSURE,
            &PROPTEST_CLOSURE.replace(&full, opening),
        );
        assert_one(&run, "assertions-reduced", None);
        let message = run.violations(GATE)[0]["message"].to_string();
        assert!(message.contains(name), "{message}");
        assert!(message.contains("from 2 to 0"), "{message}");
    }

    // The checks of an ordinary body apply inside it, on the lines of the file.
    let run = judge(
        "tests/prop.rs",
        PROPTEST_CLOSURE,
        &PROPTEST_CLOSURE.replacen("prop_assert!(a < 10);", "prop_assert!(a < 1000);", 1),
    );
    assert_one(&run, "assertion-bound-loosened", Some(6));
    let run = judge(
        "tests/prop.rs",
        PROPTEST_CLOSURE,
        &PROPTEST_CLOSURE.replacen(
            "prop_assert_eq!(digits(a), 1);",
            "prop_assert_eq!(digits(a), 2);",
            1,
        ),
    );
    assert_one(&run, "expected-value-changed", Some(7));

    // Control: a statement added, nothing lost.
    let run = judge(
        "tests/prop.rs",
        PROPTEST_CLOSURE,
        &PROPTEST_CLOSURE.replacen(
            "        prop_assert!(a < 10);\n",
            "        let _b = a;\n        prop_assert!(a < 10);\n",
            1,
        ),
    );
    assert_clean(&run);
}

const PROPTEST_CLOSURE_ADDED: &str = "
#[test]
fn empty_closure() {
    proptest!(|(a in 0..10i32)| {
    });
}
";

const PROPTEST_CLOSURE_ADDED_CHECKING: &str = "
#[test]
fn checking_closure() {
    proptest!(|(a in 0..10i32)| {
        prop_assert!(a < 10);
    });
}
";

/// A new test whose only checks are inside the closure is not vacuous; an empty closure is.
#[test]
fn proptest_closure_checks_count_for_a_new_test() {
    let run = judge(
        "tests/prop.rs",
        PROPTEST_CLOSURE,
        &format!("{PROPTEST_CLOSURE}{PROPTEST_CLOSURE_ADDED_CHECKING}"),
    );
    assert!(run.violations("vacuous-tests").is_empty(), "{}", run.stdout);
    assert_eq!(run.code, 0, "{}", run.stdout);

    let run = judge(
        "tests/prop.rs",
        PROPTEST_CLOSURE,
        &format!("{PROPTEST_CLOSURE}{PROPTEST_CLOSURE_ADDED}"),
    );
    assert_eq!(run.violations("vacuous-tests").len(), 1, "{}", run.stdout);
}

// ---------------------------------------------------------------------------
// Property tests: the result of a quickcheck property
// ---------------------------------------------------------------------------

/// A `quickcheck!` property whose body is `@BODY@`.
const QUICKCHECK_MACRO: &str = "\
use quickcheck::{quickcheck, TestResult};

quickcheck! {
    fn holds(x: u32) -> @TYPE@ {
@BODY@
    }
}
";

/// The same property as a `#[quickcheck]` function.
const QUICKCHECK_ATTRIBUTE: &str = "\
use quickcheck::TestResult;
use quickcheck_macros::quickcheck;

#[quickcheck]
fn holds(x: u32) -> @TYPE@ {
@BODY@
}
";

fn property(form: &str, ty: &str, body: &str) -> String {
    form.replace("@TYPE@", ty).replace("@BODY@", body)
}

/// `(result type, a body that checks something, the body with a result that is always true)`.
const TAILS: [(&str, &str, &str, &str); 7] = [
    (
        "a comparison with itself",
        "bool",
        "        double(x) == x + x",
        "        x == x",
    ),
    ("true", "bool", "        double(x) == x + x", "        true"),
    (
        "TestResult::passed()",
        "TestResult",
        "        check(x)",
        "        TestResult::passed()",
    ),
    (
        "an if whose branches are all true",
        "bool",
        "        if x > 0 {\n            double(x) > x\n        } else {\n            true\n        }",
        "        if x > 0 {\n            true\n        } else {\n            true\n        }",
    ),
    (
        "a match whose arms are all true",
        "bool",
        "        match x {\n            0 => true,\n            _ => double(x) > x,\n        }",
        "        match x {\n            0 => true,\n            _ => true,\n        }",
    ),
    (
        "an identifier bound to true",
        "bool",
        "        let ok = double(x) > x || x == 0;\n        ok",
        "        let ok = true;\n        ok",
    ),
    (
        "a returned TestResult::passed()",
        "TestResult",
        "        return TestResult::from_bool(double(x) >= x);",
        "        return TestResult::passed();",
    ),
];

#[test]
fn quickcheck_result_turned_always_true_is_an_assertion_loss() {
    for (form_name, form) in [
        ("macro", QUICKCHECK_MACRO),
        ("attribute", QUICKCHECK_ATTRIBUTE),
    ] {
        for (what, ty, checking, gutted) in TAILS {
            let run = judge(
                "tests/prop.rs",
                &property(form, ty, checking),
                &property(form, ty, gutted),
            );
            assert_eq!(
                codes(&run),
                [format!("{GATE}/assertions-reduced")],
                "{form_name}, {what}: {:#?}",
                run.violations(GATE)
            );
            assert_eq!(run.code, 1, "{form_name}, {what}");
        }
    }
}

/// `(result type, a body, the same check written another way)`.
const TAILS_KEPT: [(&str, &str, &str, &str); 4] = [
    (
        "operands swapped",
        "bool",
        "        double(x) == x + x",
        "        x + x == double(x)",
    ),
    (
        "bound to a name",
        "bool",
        "        double(x) == x + x",
        "        let ok = double(x) == x + x;\n        ok",
    ),
    (
        "one branch still checks",
        "bool",
        "        if x > 0 {\n            double(x) > x\n        } else {\n            true\n        }",
        "        if x == 0 {\n            true\n        } else {\n            double(x) > x\n        }",
    ),
    (
        "a result built from a check",
        "TestResult",
        "        TestResult::from_bool(double(x) >= x)",
        "        let r = TestResult::from_bool(double(x) >= x);\n        r",
    ),
];

/// Controls: a result that still depends on a check, in both forms.
#[test]
fn quickcheck_result_that_still_checks_is_clean() {
    for (form_name, form) in [
        ("macro", QUICKCHECK_MACRO),
        ("attribute", QUICKCHECK_ATTRIBUTE),
    ] {
        for (what, ty, before, after) in TAILS_KEPT {
            let run = judge(
                "tests/prop.rs",
                &property(form, ty, before),
                &property(form, ty, after),
            );
            assert!(
                run.violations(GATE).is_empty(),
                "{form_name}, {what}: {:#?}",
                run.violations(GATE)
            );
            assert_eq!(run.code, 0, "{form_name}, {what}: {}", run.stdout);
        }
    }
}

const QUICKCHECK_ADDED: &str = "
#[quickcheck]
fn added(x: u32) -> bool {
    @RESULT@
}
";

/// A new `#[quickcheck]` property is judged by its result, as one in the macro is.
#[test]
fn quickcheck_attribute_property_added_always_true_is_vacuous() {
    let base = property(QUICKCHECK_ATTRIBUTE, "bool", "    double(x) == x + x");
    for result in ["true", "x == x"] {
        let run = judge(
            "tests/prop.rs",
            &base,
            &format!("{base}{}", QUICKCHECK_ADDED.replace("@RESULT@", result)),
        );
        assert_eq!(
            run.violations("vacuous-tests").len(),
            1,
            "`{result}`: {}",
            run.stdout
        );
    }

    // Control: a result that checks.
    let run = judge(
        "tests/prop.rs",
        &base,
        &format!(
            "{base}{}",
            QUICKCHECK_ADDED.replace("@RESULT@", "double(x) >= x")
        ),
    );
    assert!(run.violations("vacuous-tests").is_empty(), "{}", run.stdout);
    assert_eq!(run.code, 0, "{}", run.stdout);
}

// ---------------------------------------------------------------------------
// Case sources read from the syntax tree
// ---------------------------------------------------------------------------

const JS_TEMPLATE_ONE_COLUMN: &str = "\
test.each`
  n
  ${1}
  ${2}
  ${3}
`(\"is positive $n\", ({ n }) => {
  expect(n).toBeGreaterThan(0);
});
";

#[test]
fn javascript_template_with_one_column_is_counted() {
    let run = judge(
        "tests/calc.test.js",
        JS_TEMPLATE_ONE_COLUMN,
        &JS_TEMPLATE_ONE_COLUMN.replace("  ${2}\n  ${3}\n", ""),
    );
    assert_reduced(&run, "dropped from 3 to 1");

    // Controls: the rows indented another way, and a row added.
    let run = judge(
        "tests/calc.test.js",
        JS_TEMPLATE_ONE_COLUMN,
        &JS_TEMPLATE_ONE_COLUMN.replace("  ${2}\n  ${3}\n", "    ${2}\n\n    ${3}\n"),
    );
    assert_clean(&run);
    let run = judge(
        "tests/calc.test.js",
        JS_TEMPLATE_ONE_COLUMN,
        &JS_TEMPLATE_ONE_COLUMN.replace("  ${3}\n", "  ${3}\n  ${4}\n"),
    );
    assert_clean(&run);
}

const JS_TEMPLATE_PIPE_IN_VALUE: &str = "\
test.each`
  a          | b
  ${1}       | ${2}
  ${flags(
    A | B
  )}         | ${3}
`(\"adds $a\", ({ a, b }) => {
  expect(a + 1).toBe(b);
});
";

/// A `|` inside a value that spans lines is not a row of the table.
#[test]
fn javascript_template_rows_are_not_the_lines_of_a_value() {
    let run = judge(
        "tests/calc.test.js",
        JS_TEMPLATE_PIPE_IN_VALUE,
        &JS_TEMPLATE_PIPE_IN_VALUE.replace("  ${1}       | ${2}\n", ""),
    );
    assert_reduced(&run, "dropped from 2 to 1");
}

const GO_NOT_A_TABLE: &str = "\
package calc

import \"testing\"

func TestRank(t *testing.T) {
	got := rank([]Contestant{
		{\"a\", 1},
		{\"b\", 2},
		{\"c\", 3},
	})
	shown := map[string]Showcase{
		\"a\": {1},
		\"b\": {2},
		\"c\": {3},
	}
	if len(got) == 0 || len(shown) == 0 {
		t.Fatal(\"empty\")
	}
}
";

const GO_NAMED_FOR_CASES: &str = "\
package calc

import \"testing\"

func TestAdd(t *testing.T) {
	cases := []testCase{
		{1, 2},
		{2, 3},
		{3, 4},
	}
	for i := 0; i < len(cases); i++ {
		if cases[i].a+1 != cases[i].b {
			t.Fatalf(\"bad %v\", cases[i])
		}
	}
}
";

#[test]
fn go_row_type_is_decided_by_its_name_not_by_letters_inside_it() {
    // `Contestant` holds `test` and `Showcase` holds `case`: neither is a row of cases.
    let run = judge(
        "calc_test.go",
        GO_NOT_A_TABLE,
        &GO_NOT_A_TABLE.replace("\t\t{\"b\", 2},\n\t\t{\"c\", 3},\n", ""),
    );
    assert_clean(&run);
    let run = judge(
        "calc_test.go",
        GO_NOT_A_TABLE,
        &GO_NOT_A_TABLE.replace("\t\t\"b\": {2},\n\t\t\"c\": {3},\n", ""),
    );
    assert_clean(&run);

    // Control: a type named for cases, declared elsewhere and walked by index.
    for name in [
        "testCase",
        "TestCase",
        "addCases",
        "testcase",
        "*pkg.TestCase",
    ] {
        let table = GO_NAMED_FOR_CASES.replace("testCase", name);
        let run = judge(
            "calc_test.go",
            &table,
            &table.replace("\t\t{2, 3},\n\t\t{3, 4},\n", ""),
        );
        assert_reduced(&run, "dropped from 3 to 1");
    }
}

const RUST_NOT_CASES: &str = "\
use rstest::rstest;

#[rstest]
#[case_x(1)]
#[case_x(2)]
#[case_x(3)]
fn adds(#[values_x(1, 2, 3)] a: i32) {
    assert!(a > 0);
}
";

const RUST_QUALIFIED_CASES: &str = "\
#[rstest::rstest]
#[rstest::case(1)]
#[case::two(2)]
#[rstest::case::three(3)]
fn adds(#[case] a: i32, #[rstest::values(1, 2, 3)] b: i32) {
    assert!(a > 0 && b > 0);
}
";

#[test]
fn rust_case_and_values_are_exact_attribute_paths() {
    // `#[case_x]` and `#[values_x]` are other attributes.
    let run = judge(
        "tests/calc.rs",
        RUST_NOT_CASES,
        &RUST_NOT_CASES.replace("#[case_x(2)]\n#[case_x(3)]\n", ""),
    );
    assert_clean(&run);
    let run = judge(
        "tests/calc.rs",
        RUST_NOT_CASES,
        &RUST_NOT_CASES.replace("#[values_x(1, 2, 3)]", "#[values_x(1)]"),
    );
    assert_clean(&run);

    // Controls: the qualified and named spellings of both.
    let run = judge(
        "tests/calc.rs",
        RUST_QUALIFIED_CASES,
        &RUST_QUALIFIED_CASES.replace("#[case::two(2)]\n#[rstest::case::three(3)]\n", ""),
    );
    assert_reduced(&run, "dropped from 9 to 3");
    let run = judge(
        "tests/calc.rs",
        RUST_QUALIFIED_CASES,
        &RUST_QUALIFIED_CASES.replace("#[rstest::values(1, 2, 3)]", "#[rstest::values(1)]"),
    );
    assert_reduced(&run, "dropped from 9 to 3");
}

// ---------------------------------------------------------------------------
// Left as it is, pinned
// ---------------------------------------------------------------------------

const GO_TABLE_ELSEWHERE: &str = "\
package calc

import \"testing\"

func TestAdd(t *testing.T) {
	for _, c := range []struct{ a, b int }{
		{1, 2},
		{2, 3},
		{3, 4},
	} {
		if c.a+1 != c.b {
			t.Fatalf(\"bad %v\", c)
		}
	}
}
";

const GO_TABLE_ELSEWHERE_HEAD: &str = "\
package calc

import \"testing\"

func TestAdd(t *testing.T) {
	for _, c := range addRows {
		if c.a+1 != c.b {
			t.Fatalf(\"bad %v\", c)
		}
	}
}
";

const GO_TABLE_FILE: &str = "\
package calc

var addRows = []struct{ a, b int }{
	{1, 2},
	{2, 3},
	{3, 4},
}
";

/// A table moved to another file of the package is not resolved: the test reads as one
/// that lost its case list.
#[test]
fn go_table_moved_to_another_file_is_not_resolved() {
    let repo = Repo::new();
    repo.commit_base("calc_test.go", GO_TABLE_ELSEWHERE, "test: base");
    repo.write("calc_test.go", GO_TABLE_ELSEWHERE_HEAD);
    repo.write("rows_test.go", GO_TABLE_FILE);
    repo.commit("test: move the table");
    let run = repo.check(&[]);
    assert_reduced(&run, "no longer read as parametrized");
}
