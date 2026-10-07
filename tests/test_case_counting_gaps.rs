//! `assertion-reduction` test-case counting (`test-cases-reduced`) and property-test
//! bodies, driven through the real binary: one base commit on `main`, one change on
//! `work`. Each defect has the shape that must be reported and the neighbouring shape
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

/// The run reports exactly one finding of the gate, a case reduction whose message
/// contains `fragment`.
#[track_caller]
fn assert_reduced(run: &Run, fragment: &str) {
    let violations = run.violations(GATE);
    assert_eq!(
        violations.len(),
        1,
        "expected one finding: {violations:#?}\n{}",
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
// Rows turned into comments
// ---------------------------------------------------------------------------

const PY_ROWS: &str = "\
import pytest

@pytest.mark.parametrize(\"a,b\", [
    (1, 2),
    (2, 3),
    (3, 4),
])
def test_x(a, b):
    assert a + 1 == b
";

#[test]
fn python_rows_turned_into_comments_are_dropped_cases() {
    let run = judge(
        "tests/test_calc.py",
        PY_ROWS,
        &PY_ROWS.replace(
            "    (2, 3),\n    (3, 4),\n",
            "    # (2, 3),\n    # (3, 4),\n",
        ),
    );
    assert_reduced(&run, "dropped from 3 to 1");

    // Control: a comment added between rows that all stay.
    let run = judge(
        "tests/test_calc.py",
        PY_ROWS,
        &PY_ROWS.replace("    (2, 3),\n", "    # the boundary\n    (2, 3),\n"),
    );
    assert_clean(&run);
}

const JS_ROWS: &str = "\
test.each([
  [1, 2],
  [2, 3],
  [3, 4],
])(\"adds %i\", (a, b) => {
  expect(a + 1).toBe(b);
});
";

#[test]
fn javascript_rows_turned_into_comments_are_dropped_cases() {
    let run = judge(
        "tests/calc.test.js",
        JS_ROWS,
        &JS_ROWS.replace("  [2, 3],\n  [3, 4],\n", "  // [2, 3],\n  // [3, 4],\n"),
    );
    assert_reduced(&run, "dropped from 3 to 1");

    // One block comment over two rows is two rows gone, not one.
    let run = judge(
        "tests/calc.test.js",
        JS_ROWS,
        &JS_ROWS.replace("  [2, 3],\n  [3, 4],\n", "  /* [2, 3],\n  [3, 4], */\n"),
    );
    assert_reduced(&run, "dropped from 3 to 1");

    // Control: a comment added between rows that all stay.
    let run = judge(
        "tests/calc.test.js",
        JS_ROWS,
        &JS_ROWS.replace("  [2, 3],\n", "  // the boundary\n  [2, 3],\n"),
    );
    assert_clean(&run);
}

const GO_TABLE: &str = "\
package calc

import \"testing\"

func TestAdd(t *testing.T) {
	cases := []struct{ a, b int }{
		{1, 2},
		{2, 3},
		{3, 4},
	}
	for _, c := range cases {
		if c.a+1 != c.b {
			t.Fatalf(\"bad %v\", c)
		}
	}
}
";

#[test]
fn go_rows_turned_into_comments_are_dropped_cases() {
    let run = judge(
        "calc_test.go",
        GO_TABLE,
        &GO_TABLE.replace(
            "\t\t{2, 3},\n\t\t{3, 4},\n",
            "\t\t// {2, 3},\n\t\t// {3, 4},\n",
        ),
    );
    assert_reduced(&run, "dropped from 3 to 1");

    // Control: a comment added between rows that all stay.
    let run = judge(
        "calc_test.go",
        GO_TABLE,
        &GO_TABLE.replace("\t\t{2, 3},\n", "\t\t// the boundary\n\t\t{2, 3},\n"),
    );
    assert_clean(&run);
}

/// A JUnit 5 parameterized test in Java with `annotations` on it.
fn java(annotations: &str) -> String {
    format!(
        "\
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.*;
import static org.junit.jupiter.api.Assertions.assertNotNull;

class CalcTest {{
    static final String ONE = \"a\";

    @ParameterizedTest
{annotations}
    void accepts(String x) {{
        assertNotNull(x);
    }}
}}
"
    )
}

const JAVA_PATH: &str = "src/test/java/CalcTest.java";

#[test]
fn java_rows_turned_into_comments_are_dropped_cases() {
    let values =
        java("    @ValueSource(strings = {\n        \"a\",\n        \"b\",\n        \"c\"\n    })");
    let run = judge(
        JAVA_PATH,
        &values,
        &values.replace(
            "        \"b\",\n        \"c\"\n",
            "        // \"b\",\n        // \"c\"\n",
        ),
    );
    assert_reduced(&run, "dropped from 3 to 1");

    let rows = java("    @CsvSource({\n        \"a\",\n        \"b\",\n        \"c\"\n    })");
    let run = judge(
        JAVA_PATH,
        &rows,
        &rows.replace(
            "        \"b\",\n        \"c\"\n",
            "        /* \"b\",\n        \"c\" */\n",
        ),
    );
    assert_reduced(&run, "dropped from 3 to 1");

    // Control: a comment added between values that all stay.
    let run = judge(
        JAVA_PATH,
        &values,
        &values.replace(
            "        \"b\",\n",
            "        // the boundary\n        \"b\",\n",
        ),
    );
    assert_clean(&run);
}

const JAVA_TEXT_BLOCK: &str =
    "    @CsvSource(textBlock = \"\"\"\n        a\n        b\n        c\n        \"\"\")";

#[test]
fn java_text_block_rows_are_counted_without_delimiters_or_comment_lines() {
    // A text block and the array holding the same rows are the same cases.
    let run = judge(
        JAVA_PATH,
        &java(JAVA_TEXT_BLOCK),
        &java("    @CsvSource({\"a\", \"b\", \"c\"})"),
    );
    assert_clean(&run);

    // A row behind `#` is a comment to JUnit: it no longer runs.
    let run = judge(
        JAVA_PATH,
        &java(JAVA_TEXT_BLOCK),
        &java(&JAVA_TEXT_BLOCK.replace("        b\n        c\n", "        # b\n        # c\n")),
    );
    assert_reduced(&run, "dropped from 3 to 1");

    // Control: rows removed from a text block.
    let run = judge(
        JAVA_PATH,
        &java(JAVA_TEXT_BLOCK),
        &java(&JAVA_TEXT_BLOCK.replace("        b\n        c\n", "")),
    );
    assert_reduced(&run, "dropped from 3 to 1");
}

const KOTLIN_TEXT_BLOCK: &str = "\
import org.junit.jupiter.params.ParameterizedTest
import org.junit.jupiter.params.provider.CsvSource
import kotlin.test.assertNotNull

class CalcTest {
    @ParameterizedTest
    @CsvSource(textBlock = \"\"\"
        a
        b
        c
    \"\"\")
    fun accepts(x: String) {
        assertNotNull(x)
    }
}
";

#[test]
fn kotlin_text_block_comment_lines_are_not_rows() {
    let run = judge(
        "src/test/kotlin/CalcTest.kt",
        KOTLIN_TEXT_BLOCK,
        &KOTLIN_TEXT_BLOCK.replace("        b\n        c\n", "        # b\n        # c\n"),
    );
    assert_reduced(&run, "dropped from 3 to 1");

    // Control: a blank line added between rows that all stay.
    let run = judge(
        "src/test/kotlin/CalcTest.kt",
        KOTLIN_TEXT_BLOCK,
        &KOTLIN_TEXT_BLOCK.replace("        b\n", "\n        b\n"),
    );
    assert_clean(&run);
}

const KOTLIN_VALUES: &str = "\
import org.junit.jupiter.params.ParameterizedTest
import org.junit.jupiter.params.provider.ValueSource
import kotlin.test.assertTrue

class CalcTest {
    @ParameterizedTest
    @ValueSource(ints = [
        1,
        2,
        3
    ])
    fun positive(x: Int) {
        assertTrue(x > 0)
    }
}
";

const RUST_CASES: &str = "\
use rstest::rstest;

#[rstest]
#[case(1, 2)]
#[case(2, 3)]
#[case(3, 4)]
fn adds(#[case] a: i32, #[case] b: i32) {
    assert_eq!(a + 1, b);
}
";

const XUNIT_ROWS: &str = "\
using Xunit;

public class CalcTest {
    [Theory]
    [InlineData(1, 2)]
    [InlineData(2, 3)]
    [InlineData(3, 4)]
    public void Adds(int a, int b) {
        Assert.Equal(b, a + 1);
    }
}
";

/// Kotlin values, Rust `#[case]` attributes and C# `[InlineData]` rows turned into
/// comments were already reported; this pins it.
#[test]
fn kotlin_rust_and_csharp_rows_turned_into_comments_are_dropped_cases() {
    let run = judge(
        "src/test/kotlin/CalcTest.kt",
        KOTLIN_VALUES,
        &KOTLIN_VALUES.replace("        2,\n        3\n", "        // 2,\n        // 3\n"),
    );
    assert_reduced(&run, "dropped from 3 to 1");

    let run = judge(
        "tests/calc.rs",
        RUST_CASES,
        &RUST_CASES.replace(
            "#[case(2, 3)]\n#[case(3, 4)]\n",
            "// #[case(2, 3)]\n// #[case(3, 4)]\n",
        ),
    );
    assert_reduced(&run, "dropped from 3 to 1");

    let run = judge(
        "tests/CalcTest.cs",
        XUNIT_ROWS,
        &XUNIT_ROWS.replace(
            "    [InlineData(2, 3)]\n    [InlineData(3, 4)]\n",
            "    // [InlineData(2, 3)]\n    // [InlineData(3, 4)]\n",
        ),
    );
    assert_reduced(&run, "dropped from 3 to 1");
}

// ---------------------------------------------------------------------------
// Go tables
// ---------------------------------------------------------------------------

const GO_PACKAGE_TABLE: &str = "\
package calc

import \"testing\"

var addCases = []struct{ a, b int }{
	{1, 2},
	{2, 3},
	{3, 4},
}

func TestAdd(t *testing.T) {
	for _, c := range addCases {
		if c.a+1 != c.b {
			t.Fatalf(\"bad %v\", c)
		}
	}
}
";

#[test]
fn go_table_moved_to_a_package_variable_keeps_its_cases() {
    // The same rows, moved out of the test function.
    let run = judge("calc_test.go", GO_TABLE, GO_PACKAGE_TABLE);
    assert_clean(&run);

    // And moved back in.
    let run = judge("calc_test.go", GO_PACKAGE_TABLE, GO_TABLE);
    assert_clean(&run);

    // Control: the move drops a row.
    let run = judge(
        "calc_test.go",
        GO_TABLE,
        &GO_PACKAGE_TABLE.replace("\t{2, 3},\n", ""),
    );
    assert_reduced(&run, "dropped from 3 to 2");
}

#[test]
fn go_package_level_table_rows_are_counted() {
    let run = judge(
        "calc_test.go",
        GO_PACKAGE_TABLE,
        &GO_PACKAGE_TABLE.replace("\t{2, 3},\n", ""),
    );
    assert_reduced(&run, "dropped from 3 to 2");

    // A row turned into a comment there is dropped too.
    let run = judge(
        "calc_test.go",
        GO_PACKAGE_TABLE,
        &GO_PACKAGE_TABLE.replace("\t{2, 3},\n", "\t// {2, 3},\n"),
    );
    assert_reduced(&run, "dropped from 3 to 2");

    // Control: a package-level table no test reads is not a test's cases.
    let unread = GO_PACKAGE_TABLE.replace("range addCases", "range otherCases()");
    let run = judge("calc_test.go", &unread, &unread.replace("\t{2, 3},\n", ""));
    assert_clean(&run);
}

const GO_TWO_TABLES: &str = "\
package calc

import \"testing\"

func TestAdd(t *testing.T) {
	good := []struct{ a, b int }{
		{1, 2},
		{2, 3},
		{3, 4},
	}
	bad := []struct{ a, b int }{
		{1, 1},
		{2, 2},
	}
	for _, c := range good {
		if c.a+1 != c.b {
			t.Fatalf(\"bad %v\", c)
		}
	}
	for _, c := range bad {
		if c.a+1 == c.b {
			t.Fatalf(\"bad %v\", c)
		}
	}
}
";

#[test]
fn go_every_table_in_a_test_is_counted() {
    // A row removed from the smaller of two tables.
    let run = judge(
        "calc_test.go",
        GO_TWO_TABLES,
        &GO_TWO_TABLES.replace("\t\t{2, 2},\n", ""),
    );
    assert_reduced(&run, "dropped from 5 to 4");

    // Control: a row added to the smaller table.
    let run = judge(
        "calc_test.go",
        GO_TWO_TABLES,
        &GO_TWO_TABLES.replace("\t\t{2, 2},\n", "\t\t{2, 2},\n\t\t{3, 3},\n"),
    );
    assert_clean(&run);
}

const GO_NAMED_ROWS: &str = "\
package calc

import \"testing\"

type pair struct{ a, b int }

func TestAdd(t *testing.T) {
	rows := []pair{
		{1, 2},
		{2, 3},
		{3, 4},
	}
	want := []pair{
		{1, 2},
		{2, 3},
	}
	for _, c := range rows {
		if c.a+1 != c.b {
			t.Fatalf(\"bad %v\", c)
		}
	}
	if len(want) == 0 {
		t.Fatal(\"empty\")
	}
}
";

#[test]
fn go_table_of_a_named_struct_type_is_counted_when_ranged_over() {
    let run = judge(
        "calc_test.go",
        GO_NAMED_ROWS,
        &GO_NAMED_ROWS.replace("\t\t{2, 3},\n\t\t{3, 4},\n", "\t\t{3, 4},\n"),
    );
    assert_reduced(&run, "dropped from 3 to 2");

    // Control: a slice of the same type the test does not range over is a value, not a
    // table.
    let run = judge(
        "calc_test.go",
        GO_NAMED_ROWS,
        &GO_NAMED_ROWS.replace(
            "\t\t{1, 2},\n\t\t{2, 3},\n\t}\n\tfor",
            "\t\t{1, 2},\n\t}\n\tfor",
        ),
    );
    assert_clean(&run);
}

const GO_SET: &str = "\
package calc

import \"testing\"

func TestSeen(t *testing.T) {
	seen := map[string]struct{}{
		\"a\": {},
		\"b\": {},
		\"c\": {},
	}
	if len(seen) == 0 {
		t.Fatal(\"empty\")
	}
}
";

const GO_MAP_TABLE: &str = "\
package calc

import \"testing\"

func TestAdd(t *testing.T) {
	cases := map[string]struct{ a, b int }{
		\"one\":   {1, 2},
		\"two\":   {2, 3},
		\"three\": {3, 4},
	}
	for name, c := range cases {
		if c.a+1 != c.b {
			t.Fatalf(\"bad %s\", name)
		}
	}
}
";

#[test]
fn go_set_of_empty_structs_is_not_a_case_table() {
    let run = judge(
        "calc_test.go",
        GO_SET,
        &GO_SET.replace("\t\t\"b\": {},\n", ""),
    );
    assert_clean(&run);

    // Control: a map whose values are case structs is a table.
    let run = judge(
        "calc_test.go",
        GO_MAP_TABLE,
        &GO_MAP_TABLE.replace("\t\t\"two\":   {2, 3},\n", ""),
    );
    assert_reduced(&run, "dropped from 3 to 2");
}

// ---------------------------------------------------------------------------
// Rust: rstest and test_case
// ---------------------------------------------------------------------------

/// An `rstest` function whose one argument takes `values`.
fn rust_values(values: &str) -> String {
    format!(
        "use rstest::rstest;\n\n#[rstest]\nfn adds(#[values({values})] a: i32) {{\n    assert!(a != 0);\n}}\n"
    )
}

#[test]
fn rust_values_are_counted_by_argument() {
    // Three values written with more tokens are still three values.
    let run = judge(
        "tests/calc.rs",
        &rust_values("-1, f(2), 3"),
        &rust_values("1, 2, 3"),
    );
    assert_clean(&run);

    // Two values written with as many tokens as three are two values.
    let run = judge(
        "tests/calc.rs",
        &rust_values("1, 2, 3"),
        &rust_values("-1, f(2)"),
    );
    assert_reduced(&run, "dropped from 3 to 2");

    // Values turned into a comment are gone.
    let run = judge(
        "tests/calc.rs",
        &rust_values("1, 2, 3"),
        &rust_values("1 /* , 2, 3 */"),
    );
    assert_reduced(&run, "dropped from 3 to 1");

    // Control: a trailing comma adds no value.
    let run = judge(
        "tests/calc.rs",
        &rust_values("1, 2, 3"),
        &rust_values("1, 2, 3,"),
    );
    assert_clean(&run);
}

const RUST_CASES_AND_VALUES: &str = "\
use rstest::rstest;

#[rstest]
#[case(1)]
#[case(2)]
fn orders(#[case] a: i32, #[values(10, 20, 30)] b: i32) {
    assert!(a < b);
}
";

#[test]
fn rust_values_multiply_the_cases_they_sit_beside() {
    let run = judge(
        "tests/calc.rs",
        RUST_CASES_AND_VALUES,
        &RUST_CASES_AND_VALUES.replace("#[values(10, 20, 30)]", "#[values(10)]"),
    );
    assert_reduced(&run, "dropped from 6 to 2");

    // Control: the same values in another order.
    let run = judge(
        "tests/calc.rs",
        RUST_CASES_AND_VALUES,
        &RUST_CASES_AND_VALUES.replace("#[values(10, 20, 30)]", "#[values(30, 20, 10)]"),
    );
    assert_clean(&run);
}

const RUST_TEST_CASE: &str = "\
use test_case::test_case;

#[test_case(1, 2)]
#[test_case(2, 3 ; \"two\")]
#[test_case(3, 4)]
fn adds(a: i32, b: i32) {
    assert_eq!(a + 1, b);
}
";

#[test]
fn rust_test_case_attributes_are_counted() {
    let run = judge(
        "tests/calc.rs",
        RUST_TEST_CASE,
        &RUST_TEST_CASE.replace("#[test_case(2, 3 ; \"two\")]\n#[test_case(3, 4)]\n", ""),
    );
    assert_reduced(&run, "dropped from 3 to 1");

    // Control: a case added.
    let run = judge(
        "tests/calc.rs",
        RUST_TEST_CASE,
        &RUST_TEST_CASE.replace(
            "#[test_case(3, 4)]\n",
            "#[test_case(3, 4)]\n#[test_case(4, 5)]\n",
        ),
    );
    assert_clean(&run);
}

// ---------------------------------------------------------------------------
// JavaScript
// ---------------------------------------------------------------------------

const JS_DESCRIBE_EACH: &str = "\
describe.each([
  [1, 2],
  [2, 3],
  [3, 4],
])(\"add %i\", (a, b) => {
  test(\"adds\", () => {
    expect(a + 1).toBe(b);
  });
});
";

#[test]
fn javascript_describe_each_rows_are_the_cases_of_its_tests() {
    let run = judge(
        "tests/calc.test.js",
        JS_DESCRIBE_EACH,
        &JS_DESCRIBE_EACH.replace("  [2, 3],\n  [3, 4],\n", ""),
    );
    assert_reduced(&run, "dropped from 3 to 1");

    // Control: a row added.
    let run = judge(
        "tests/calc.test.js",
        JS_DESCRIBE_EACH,
        &JS_DESCRIBE_EACH.replace("  [3, 4],\n", "  [3, 4],\n  [4, 5],\n"),
    );
    assert_clean(&run);
}

const JS_TEMPLATE_ONE_ROW: &str = "\
test.each`
  a    | b
  ${1} | ${2}
`(\"adds $a\", ({ a, b }) => {
  expect(a + 1).toBe(b);
});
";

#[test]
fn javascript_template_with_only_its_header_has_no_cases() {
    let run = judge(
        "tests/calc.test.js",
        JS_TEMPLATE_ONE_ROW,
        &JS_TEMPLATE_ONE_ROW.replace("  ${1} | ${2}\n", ""),
    );
    assert_reduced(&run, "dropped from 1 to 0");

    // Control: a row added.
    let run = judge(
        "tests/calc.test.js",
        JS_TEMPLATE_ONE_ROW,
        &JS_TEMPLATE_ONE_ROW.replace("  ${1} | ${2}\n", "  ${1} | ${2}\n  ${2} | ${3}\n"),
    );
    assert_clean(&run);
}

// ---------------------------------------------------------------------------
// Java and Kotlin sources
// ---------------------------------------------------------------------------

#[test]
fn java_case_sources_on_one_test_add_up() {
    let both =
        java("    @ValueSource(strings = {\"a\", \"b\", \"c\"})\n    @CsvSource({\"x\", \"y\"})");
    // The first of two sources loses values.
    let run = judge(
        JAVA_PATH,
        &both,
        &both.replace("{\"a\", \"b\", \"c\"}", "{\"a\"}"),
    );
    assert_reduced(&run, "dropped from 5 to 3");

    // Control: a value moved from one source to the other.
    let run = judge(
        JAVA_PATH,
        &both,
        &java("    @ValueSource(strings = {\"a\", \"b\"})\n    @CsvSource({\"x\", \"y\", \"c\"})"),
    );
    assert_clean(&run);
}

#[test]
fn java_csv_source_rows_are_its_value_not_its_other_arrays() {
    let rows =
        java("    @CsvSource(nullValues = {\"N/A\", \"NIL\"}, value = {\"a\", \"b\", \"c\"})");
    // Rows dropped behind an unchanged `nullValues`.
    let run = judge(
        JAVA_PATH,
        &rows,
        &rows.replace("{\"a\", \"b\", \"c\"}", "{\"a\"}"),
    );
    assert_reduced(&run, "dropped from 3 to 1");

    // Control: `nullValues` shortened, every row kept.
    let run = judge(
        JAVA_PATH,
        &rows,
        &rows.replace("{\"N/A\", \"NIL\"}", "{\"N/A\"}"),
    );
    assert_clean(&run);
}

#[test]
fn java_enum_source_names_and_null_sources_are_cases() {
    let names = java("    @EnumSource(value = Mode.class, names = {\"A\", \"B\", \"C\"})");
    let run = judge(
        JAVA_PATH,
        &names,
        &names.replace("{\"A\", \"B\", \"C\"}", "{\"A\"}"),
    );
    assert_reduced(&run, "dropped from 3 to 1");

    let nulls = java("    @NullAndEmptySource\n    @ValueSource(strings = {\"a\", \"b\"})");
    let run = judge(
        JAVA_PATH,
        &nulls,
        &nulls.replace("    @NullAndEmptySource\n", ""),
    );
    assert_reduced(&run, "dropped from 4 to 2");

    // Control: `@NullAndEmptySource` written as its two halves.
    let run = judge(
        JAVA_PATH,
        &nulls,
        &nulls.replace(
            "    @NullAndEmptySource\n",
            "    @NullSource\n    @EmptySource\n",
        ),
    );
    assert_clean(&run);
}

/// A `@ValueSource` that names a constant is one case (#626): an annotation element
/// takes constant expressions, and no array is one.
#[test]
fn java_value_source_that_names_a_constant_is_one_case() {
    let literal = java("    @ValueSource(strings = {\"a\", \"b\", \"c\"})");
    let run = judge(
        JAVA_PATH,
        &literal,
        &literal.replace("{\"a\", \"b\", \"c\"}", "ONE"),
    );
    assert_reduced(&run, "3 to 1");

    // Control: the constant written with and without braces is the same one case.
    let named = java("    @ValueSource(strings = ONE)");
    let run = judge(JAVA_PATH, &named, &named.replace("= ONE", "= {ONE}"));
    assert_clean(&run);

    // Control: one literal value written with and without braces is one case.
    let one = java("    @ValueSource(strings = {\"a\"})");
    let run = judge(JAVA_PATH, &one, &one.replace("{\"a\"}", "\"a\""));
    assert_clean(&run);
}

const KOTLIN_NULL_SOURCE: &str = "\
import org.junit.jupiter.params.ParameterizedTest
import org.junit.jupiter.params.provider.NullAndEmptySource
import org.junit.jupiter.params.provider.ValueSource
import kotlin.test.assertTrue

class CalcTest {
    @ParameterizedTest
    @NullAndEmptySource
    @ValueSource(strings = [\"a\", \"b\"])
    fun accepts(x: String?) {
        assertTrue(x == null || x.length < 2)
    }
}
";

#[test]
fn kotlin_null_sources_are_cases() {
    let run = judge(
        "src/test/kotlin/CalcTest.kt",
        KOTLIN_NULL_SOURCE,
        &KOTLIN_NULL_SOURCE.replace("    @NullAndEmptySource\n", ""),
    );
    assert_reduced(&run, "dropped from 4 to 2");

    // Control: `@NullAndEmptySource` written as its two halves.
    let run = judge(
        "src/test/kotlin/CalcTest.kt",
        KOTLIN_NULL_SOURCE,
        &KOTLIN_NULL_SOURCE.replace(
            "    @NullAndEmptySource\n",
            "    @NullSource\n    @EmptySource\n",
        ),
    );
    assert_clean(&run);
}

// ---------------------------------------------------------------------------
// C#
// ---------------------------------------------------------------------------

const NUNIT_ROWS: &str = "\
using NUnit.Framework;

public class CalcTest {
    [TestCase(1, 2)]
    [TestCase(2, 3)]
    [TestCase(3, 4)]
    public void Adds(int a, int b) {
        Assert.AreEqual(b, a + 1);
    }
}
";

#[test]
fn csharp_rows_marked_skipped_are_dropped_cases() {
    let run = judge(
        "tests/CalcTest.cs",
        XUNIT_ROWS,
        &XUNIT_ROWS.replace(
            "    [InlineData(2, 3)]\n    [InlineData(3, 4)]\n",
            "    [InlineData(2, 3, Skip = \"flaky\")]\n    [InlineData(3, 4, Skip = \"flaky\")]\n",
        ),
    );
    assert_reduced(&run, "dropped from 3 to 1");

    let run = judge(
        "tests/CalcTest.cs",
        NUNIT_ROWS,
        &NUNIT_ROWS.replace("[TestCase(2, 3)]", "[TestCase(2, 3, Ignore = \"flaky\")]"),
    );
    assert_reduced(&run, "dropped from 3 to 2");

    // Control: a named argument that does not skip the row.
    let run = judge(
        "tests/CalcTest.cs",
        NUNIT_ROWS,
        &NUNIT_ROWS.replace("[TestCase(2, 3)]", "[TestCase(2, 3, TestName = \"two\")]"),
    );
    assert_clean(&run);
}

const MSTEST_ROWS: &str = "\
using Microsoft.VisualStudio.TestTools.UnitTesting;

[TestClass]
public class CalcTest {
    [DataTestMethod]
    [DataRow(1, 2)]
    [DataRow(2, 3)]
    [DataRow(3, 4)]
    public void Adds(int a, int b) {
        Assert.AreEqual(b, a + 1);
    }
}
";

#[test]
fn csharp_data_rows_are_counted() {
    let run = judge(
        "tests/CalcTest.cs",
        MSTEST_ROWS,
        &MSTEST_ROWS.replace("    [DataRow(2, 3)]\n    [DataRow(3, 4)]\n", ""),
    );
    assert_reduced(&run, "dropped from 3 to 1");

    // Control: a row added.
    let run = judge(
        "tests/CalcTest.cs",
        MSTEST_ROWS,
        &MSTEST_ROWS.replace(
            "    [DataRow(3, 4)]\n",
            "    [DataRow(3, 4)]\n    [DataRow(4, 5)]\n",
        ),
    );
    assert_clean(&run);
}

// ---------------------------------------------------------------------------
// Python: parametrization above the function
// ---------------------------------------------------------------------------

const PY_CLASS_PARAMS: &str = "\
import pytest

@pytest.mark.parametrize(\"x\", [1, 2, 3])
class TestCalc:
    def test_x(self, x):
        assert x > 0
";

const PY_MODULE_PARAMS: &str = "\
import pytest

pytestmark = pytest.mark.parametrize(\"x\", [1, 2, 3])

def test_x(x):
    assert x > 0
";

#[test]
fn python_class_and_module_parametrization_is_counted() {
    let run = judge(
        "tests/test_calc.py",
        PY_CLASS_PARAMS,
        &PY_CLASS_PARAMS.replace("[1, 2, 3]", "[1]"),
    );
    assert_reduced(&run, "dropped from 3 to 1");

    let run = judge(
        "tests/test_calc.py",
        PY_MODULE_PARAMS,
        &PY_MODULE_PARAMS.replace("[1, 2, 3]", "[1]"),
    );
    assert_reduced(&run, "dropped from 3 to 1");

    // A `pytestmark` list in the class body.
    let in_class = "\
import pytest

class TestCalc:
    pytestmark = [pytest.mark.parametrize(\"x\", [1, 2, 3])]

    def test_x(self, x):
        assert x > 0
";
    let run = judge(
        "tests/test_calc.py",
        in_class,
        &in_class.replace("[1, 2, 3]", "[1, 2]"),
    );
    assert_reduced(&run, "dropped from 3 to 2");

    // Control: the same values moved from the class onto its one test.
    let on_method = "\
import pytest

class TestCalc:
    @pytest.mark.parametrize(\"x\", [1, 2, 3])
    def test_x(self, x):
        assert x > 0
";
    let run = judge("tests/test_calc.py", PY_CLASS_PARAMS, on_method);
    assert_clean(&run);
}

// ---------------------------------------------------------------------------
// proptest! bodies
// ---------------------------------------------------------------------------

const PROPTEST: &str = "\
use proptest::prelude::*;

proptest! {
    #[test]
    fn commutes(a in 0..10i32, b in 0..10i32) {
        prop_assert_eq!(a + b, b + a);
        prop_assert!(a + b >= a);
    }
}
";

#[test]
fn proptest_assertions_made_unreachable_are_dropped() {
    let run = judge(
        "tests/prop.rs",
        PROPTEST,
        &PROPTEST.replace(
            "        prop_assert!(a + b >= a);\n",
            "        if false {\n            prop_assert!(a + b >= a);\n        }\n",
        ),
    );
    assert_eq!(
        run.titles(GATE).len(),
        1,
        "an assertion under `if false` no longer runs: {}",
        run.stdout
    );
    assert!(
        run.violations(GATE)[0]["message"]
            .as_str()
            .unwrap()
            .contains("dropped from 2 to 1"),
        "{}",
        run.stdout
    );
    assert_eq!(run.code, 1);

    let run = judge(
        "tests/prop.rs",
        PROPTEST,
        &PROPTEST.replace(
            "        prop_assert_eq!(a + b, b + a);\n",
            "        return Ok(());\n        prop_assert_eq!(a + b, b + a);\n",
        ),
    );
    assert_eq!(run.titles(GATE).len(), 1, "{}", run.stdout);
    assert!(
        run.violations(GATE)[0]["message"]
            .as_str()
            .unwrap()
            .contains("dropped from 2 to 0"),
        "{}",
        run.stdout
    );
    assert_eq!(run.code, 1);

    // Control: the assertion under a condition that can hold still counts.
    let run = judge(
        "tests/prop.rs",
        PROPTEST,
        &PROPTEST.replace(
            "        prop_assert!(a + b >= a);\n",
            "        if a >= 0 {\n            prop_assert!(a + b >= a);\n        }\n",
        ),
    );
    assert_clean(&run);
}

#[test]
fn proptest_body_that_does_not_parse_is_not_read_as_clean() {
    let run = judge(
        "tests/prop.rs",
        PROPTEST,
        &PROPTEST.replace(
            "        prop_assert!(a + b >= a);\n",
            "        prop_assert!(a + b >= a);\n        let x = ;\n",
        ),
    );
    let codes: Vec<String> = run
        .violations(GATE)
        .iter()
        .map(|v| v["code"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        codes,
        ["assertion-reduction/source-parsed-with-errors"],
        "{}",
        run.stdout
    );
    assert_eq!(run.code, 1);

    // Control: a statement added that parses.
    let run = judge(
        "tests/prop.rs",
        PROPTEST,
        &PROPTEST.replace(
            "        prop_assert!(a + b >= a);\n",
            "        prop_assert!(a + b >= a);\n        let _x = a;\n",
        ),
    );
    assert_clean(&run);
}
