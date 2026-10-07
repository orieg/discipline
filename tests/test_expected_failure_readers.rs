//! Expected failures through the real binary (#649), in the languages and forms that had
//! no reader: Go (`errors.Is` / `errors.As`, testify, gocheck), Swift, Scala and
//! Objective-C, and further forms of C++, Java, JavaScript, Kotlin and C#. A
//! `*_is_reported` case is a widening that must be reported once; a `*_is_silent` case is
//! a control that must stay unreported; a `*_is_not_vacuous` case is a new test whose
//! only check is the form.

mod common;
use common::{Repo, Run};

const GATE: &str = "assertion-reduction";
const WIDENED: &str = "Expected Exception Or Panic Widened";
const VACUOUS: &str = "Vacuous Test Added";

#[derive(Clone, Copy)]
enum Lang {
    Go,
    GoSuite,
    GoCheck,
    XCTest,
    SwiftTesting,
    ScalaTest,
    Specs2,
    ObjC,
    Catch2,
    Java,
    Js,
    Kotlin,
    MsTest,
    CSharpBare,
}

fn indent(body: &str, by: &str) -> String {
    body.lines()
        .map(|l| format!("{by}{l}\n"))
        .collect::<String>()
}

/// The imports of a Go test file: `prelude` when it gives any, else testify's.
fn go_imports(prelude: &str, default: &str) -> String {
    let imports = if prelude.is_empty() { default } else { prelude };
    format!(
        "package sut\n\nimport (\n{}\n)\n\n",
        indent(imports, "\t").trim_end()
    )
}

/// The test file of `lang`: `prelude` (imports and declarations) above one test with `body`.
fn source(lang: Lang, prelude: &str, body: &str) -> (&'static str, String) {
    match lang {
        Lang::Go => (
            "sut_test.go",
            format!(
                "{}func TestRejects(t *testing.T) {{\n\tn, err := f(-1)\n{}}}\n",
                go_imports(
                    prelude,
                    "\"errors\"\n\"testing\"\n\n\"github.com/stretchr/testify/assert\"\n\"github.com/stretchr/testify/require\""
                ),
                indent(body, "\t")
            ),
        ),
        Lang::GoSuite => (
            "sut_test.go",
            format!(
                "{}type S struct {{\n\tsuite.Suite\n}}\n\nfunc (s *S) TestRejects() {{\n\tn, err := f(-1)\n{}}}\n\nfunc TestS(t *testing.T) {{\n\tsuite.Run(t, new(S))\n}}\n",
                go_imports(
                    prelude,
                    "\"testing\"\n\n\"github.com/stretchr/testify/require\"\n\"github.com/stretchr/testify/suite\""
                ),
                indent(body, "\t")
            ),
        ),
        Lang::XCTest => (
            "Tests/SutTests/SutTests.swift",
            format!(
                "{}\n@testable import Sut\n\nfinal class SutTests: XCTestCase {{\n    func testRejects() throws {{\n{}    }}\n}}\n",
                if prelude.is_empty() { "import XCTest" } else { prelude },
                indent(body, "        ")
            ),
        ),
        Lang::SwiftTesting => (
            "Tests/SutTests/SutTests.swift",
            format!(
                "{}\n@testable import Sut\n\n@Test func rejects() throws {{\n{}}}\n",
                if prelude.is_empty() { "import Testing" } else { prelude },
                indent(body, "    ")
            ),
        ),
        Lang::ScalaTest => (
            "src/test/scala/SutSpec.scala",
            format!(
                "import org.scalatest.funsuite.AnyFunSuite\nimport org.scalatest.matchers.should.Matchers\n{prelude}\nclass SutSpec extends AnyFunSuite with Matchers {{\n  test(\"rejects\") {{\n{}  }}\n}}\n",
                indent(body, "    ")
            ),
        ),
        Lang::Specs2 => (
            "src/test/scala/SutSpec.scala",
            format!(
                "import org.specs2.mutable.Specification\n{prelude}\nclass SutSpec extends Specification {{\n  \"sut\" should {{\n    \"reject\" in {{\n{}    }}\n  }}\n}}\n",
                indent(body, "      ")
            ),
        ),
        Lang::ObjC => (
            "Tests/SutTests.m",
            format!(
                "#import <XCTest/XCTest.h>\n{prelude}\n@interface SutTests : XCTestCase\n@end\n\n@implementation SutTests\n- (void)testRejects {{\n{}}}\n@end\n",
                indent(body, "    ")
            ),
        ),
        Lang::Catch2 => (
            "tests/sut_test.cpp",
            format!(
                "#include <catch2/catch_test_macros.hpp>\n#include <stdexcept>\n{prelude}\nTEST_CASE(\"rejects\") {{\n{}}}\n",
                indent(body, "  ")
            ),
        ),
        Lang::Java => (
            "src/test/java/SutTest.java",
            format!(
                "import org.junit.jupiter.api.Test;\nimport static org.assertj.core.api.Assertions.*;\n\nclass SutTest {{\n{}    @Test\n    void rejects() {{\n{}    }}\n}}\n",
                indent(prelude, "    "),
                indent(body, "        ")
            ),
        ),
        Lang::Js => (
            "tests/sut.test.js",
            format!(
                "require(\"chai\").should();\n{prelude}\nit(\"rejects\", () => {{\n{}}});\n",
                indent(body, "  ")
            ),
        ),
        Lang::Kotlin => (
            "src/test/kotlin/SutTest.kt",
            format!(
                "import io.kotest.assertions.throwables.shouldThrow\nimport io.kotest.matchers.shouldBe\nimport org.junit.jupiter.api.Test\n{prelude}\nclass SutTest {{\n    @Test\n    fun rejects() {{\n{}    }}\n}}\n",
                indent(body, "        ")
            ),
        ),
        Lang::MsTest => (
            "tests/SutTests.cs",
            format!(
                "using Microsoft.VisualStudio.TestTools.UnitTesting;\n{prelude}\n[TestClass]\npublic class SutTests {{\n    [TestMethod]\n    public void Rejects() {{\n{}    }}\n}}\n",
                indent(body, "        ")
            ),
        ),
        Lang::CSharpBare => (
            "tests/SutTests.cs",
            format!(
                "{prelude}\npublic class SutTests {{\n    [TestMethod]\n    public void Rejects() {{\n{}    }}\n}}\n",
                indent(body, "        ")
            ),
        ),
        Lang::GoCheck => (
            "sut_test.go",
            format!(
                "{}type S struct{{}}\n\nvar _ = Suite(&S{{}})\n\nfunc (s *S) TestRejects(c *C) {{\n\tn, err := f(-1)\n{}}}\n",
                go_imports(prelude, "\"testing\"\n\n. \"gopkg.in/check.v1\""),
                indent(body, "\t")
            ),
        ),
    }
}

/// Commits `base` on the base side and `head` on the work branch, then checks. Each side
/// is `(prelude, body)`.
fn change(lang: Lang, base: (&str, &str), head: (&str, &str)) -> Run {
    let repo = Repo::new();
    let (path, base_src) = source(lang, base.0, base.1);
    let (_, head_src) = source(lang, head.0, head.1);
    repo.commit_base(path, &base_src, "test: base");
    repo.write(path, &head_src);
    repo.commit("test: change the test");
    repo.check(&[])
}

/// Adds one new test with `body` and checks.
fn added(lang: Lang, prelude: &str, body: &str) -> Run {
    let repo = Repo::new();
    let (path, src) = source(lang, prelude, body);
    repo.write(path, &src);
    repo.commit("test: add a test");
    repo.check(&[])
}

fn widened_messages(run: &Run) -> Vec<String> {
    run.violations(GATE)
        .iter()
        .filter(|v| v["title"] == WIDENED)
        .map(|v| {
            assert_eq!(v["code"], "assertion-reduction/expected-exception-widened");
            v["message"].as_str().unwrap_or("").to_string()
        })
        .collect()
}

/// `reported!(name, Lang, [prelude,] base, head [, needle])`: exactly one finding, whose
/// message holds `needle`. The prelude is written `[prelude]`, or
/// `[base prelude => head prelude]` when the change edits it too.
macro_rules! reported {
    ($name:ident, $lang:ident, [$bp:expr => $hp:expr], $base:expr, $head:expr, $needle:expr) => {
        #[test]
        fn $name() {
            let run = change(Lang::$lang, ($bp, $base), ($hp, $head));
            let got = widened_messages(&run);
            assert_eq!(got.len(), 1, "{got:?}\n{}{}", run.stdout, run.stderr);
            assert!(got[0].contains($needle), "{got:?}");
            assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
        }
    };
    ($name:ident, $lang:ident, [$prelude:expr], $base:expr, $head:expr) => {
        reported!($name, $lang, [$prelude => $prelude], $base, $head, "");
    };
    ($name:ident, $lang:ident, [$prelude:expr], $base:expr, $head:expr, $needle:expr) => {
        reported!($name, $lang, [$prelude => $prelude], $base, $head, $needle);
    };
    ($name:ident, $lang:ident, $base:expr, $head:expr) => {
        reported!($name, $lang, ["" => ""], $base, $head, "");
    };
    ($name:ident, $lang:ident, $base:expr, $head:expr, $needle:expr) => {
        reported!($name, $lang, ["" => ""], $base, $head, $needle);
    };
}

macro_rules! silent {
    ($name:ident, $lang:ident, [$bp:expr => $hp:expr], $base:expr, $head:expr) => {
        #[test]
        fn $name() {
            let run = change(Lang::$lang, ($bp, $base), ($hp, $head));
            let got = widened_messages(&run);
            assert!(got.is_empty(), "{got:?}\n{}{}", run.stdout, run.stderr);
        }
    };
    ($name:ident, $lang:ident, [$prelude:expr], $base:expr, $head:expr) => {
        silent!($name, $lang, [$prelude => $prelude], $base, $head);
    };
    ($name:ident, $lang:ident, $base:expr, $head:expr) => {
        silent!($name, $lang, ["" => ""], $base, $head);
    };
}

/// A new test whose only check is `body` is not reported by `vacuous-tests`.
macro_rules! not_vacuous {
    ($name:ident, $lang:ident, [$prelude:expr], $body:expr) => {
        #[test]
        fn $name() {
            let run = added(Lang::$lang, $prelude, $body);
            assert!(
                run.titles("vacuous-tests").is_empty(),
                "{}{}",
                run.stdout,
                run.stderr
            );
        }
    };
    ($name:ident, $lang:ident, $body:expr) => {
        not_vacuous!($name, $lang, [""], $body);
    };
}

/// Control for `not_vacuous!`: a new test with `body` alone is reported.
macro_rules! vacuous {
    ($name:ident, $lang:ident, [$prelude:expr], $body:expr) => {
        #[test]
        fn $name() {
            let run = added(Lang::$lang, $prelude, $body);
            assert_eq!(
                run.titles("vacuous-tests"),
                vec![VACUOUS],
                "{}{}",
                run.stdout,
                run.stderr
            );
        }
    };
}

// ---------------------------------------------------------------------------
// Go: testify on the package, a suite and a bound assertion object.
// ---------------------------------------------------------------------------

reported!(
    go_error_is_replaced_by_error_is_reported,
    Go,
    "require.ErrorIs(t, err, ErrSpecific)",
    "require.Error(t, err)",
    "type was removed"
);

reported!(
    go_error_as_replaced_by_error_is_reported,
    Go,
    "var pe *fs.PathError\nassert.ErrorAs(t, err, &pe)",
    "var pe *fs.PathError\nassert.Error(t, err)",
    "type was removed"
);

reported!(
    go_error_contains_replaced_by_error_is_reported,
    Go,
    "require.ErrorContains(t, err, \"negative\")",
    "require.Error(t, err)",
    "matcher was removed"
);

// The count holds: the message assertion made way for an assertion on something else.
reported!(
    go_error_contains_dropped_is_reported,
    Go,
    "require.ErrorContains(t, err, \"negative\")",
    "require.Equal(t, 0, n)",
    "expected message `negative` is no longer checked"
);

reported!(
    go_equal_error_replaced_by_error_contains_is_reported,
    Go,
    "require.EqualError(t, err, \"negative value\")",
    "require.ErrorContains(t, err, \"negative value\")",
    "no longer the whole message"
);

reported!(
    go_equal_error_replaced_by_a_shorter_error_contains_is_reported,
    Go,
    "require.EqualError(t, err, \"negative value\")",
    "require.ErrorContains(t, err, \"negative\")",
    "matches more messages"
);

reported!(
    go_error_contains_emptied_is_reported,
    Go,
    "require.ErrorContains(t, err, \"negative\")",
    "require.ErrorContains(t, err, \"\")",
    "now accepts any message"
);

reported!(
    go_panics_with_value_replaced_by_panics_is_reported,
    Go,
    "assert.PanicsWithValue(t, \"boom\", func() { f(-1) })",
    "assert.Panics(t, func() { f(-1) })",
    "matcher was removed"
);

reported!(
    go_panics_with_error_replaced_by_panics_is_reported,
    Go,
    "require.PanicsWithError(t, \"boom\", func() { f(-1) })",
    "require.Panics(t, func() { f(-1) })",
    "matcher was removed"
);

reported!(
    go_true_of_errors_is_replaced_by_error_is_reported,
    Go,
    "require.True(t, errors.Is(err, ErrSpecific))",
    "require.Error(t, err)",
    "type was removed"
);

reported!(
    go_suite_require_error_is_replaced_by_error_is_reported,
    GoSuite,
    "s.Require().ErrorIs(err, ErrSpecific)",
    "s.Require().Error(err)",
    "type was removed"
);

reported!(
    go_suite_receiver_error_contains_replaced_by_error_is_reported,
    GoSuite,
    "s.ErrorContains(err, \"negative\")",
    "s.Error(err)",
    "matcher was removed"
);

reported!(
    go_bound_assertion_object_error_is_replaced_by_error_is_reported,
    GoSuite,
    "r := s.Require()\nr.ErrorIs(err, ErrSpecific)",
    "r := s.Require()\nr.Error(err)",
    "type was removed"
);

reported!(
    go_require_new_error_is_replaced_by_error_is_reported,
    Go,
    "r := require.New(t)\nr.ErrorIs(err, ErrSpecific)",
    "r := require.New(t)\nr.Error(err)",
    "type was removed"
);

reported!(
    go_testify_under_another_import_name_is_reported,
    Go,
    ["\"testing\"\n\ntr \"github.com/stretchr/testify/require\""],
    "tr.ErrorIs(t, err, ErrSpecific)",
    "tr.Error(t, err)",
    "type was removed"
);

// A sign flip is another assertion, not a wider one.
silent!(
    go_error_replaced_by_no_error_is_silent,
    Go,
    "require.Error(t, err)",
    "require.NoError(t, err)"
);

silent!(
    go_panics_replaced_by_not_panics_is_silent,
    Go,
    "assert.Panics(t, func() { f(-1) })",
    "assert.NotPanics(t, func() { f(-1) })"
);

// Go has no hierarchy: one sentinel replaced by another is a substitution.
silent!(
    go_error_is_sentinel_replaced_is_silent,
    Go,
    "require.ErrorIs(t, err, ErrSpecific)",
    "require.ErrorIs(t, err, pkg.Error)"
);

silent!(
    go_error_narrowed_to_error_is_is_silent,
    Go,
    "require.Error(t, err)",
    "require.ErrorIs(t, err, ErrSpecific)"
);

silent!(
    go_error_contains_tightened_to_equal_error_is_silent,
    Go,
    "require.ErrorContains(t, err, \"negative\")",
    "require.EqualError(t, err, \"negative\")"
);

silent!(
    go_panics_narrowed_to_panics_with_value_is_silent,
    Go,
    "assert.Panics(t, func() { f(-1) })",
    "assert.PanicsWithValue(t, \"boom\", func() { f(-1) })"
);

// `require` is a package of the project here, and `other` is no assertion object.
silent!(
    go_require_of_another_package_is_silent,
    Go,
    ["\"testing\"\n\n\"example.com/own/require\""],
    "require.ErrorIs(t, err, ErrSpecific)",
    "require.Error(t, err)"
);

silent!(
    go_error_is_on_another_receiver_is_silent,
    GoSuite,
    "s.db.ErrorIs(err, ErrSpecific)\ns.Equal(0, n)",
    "s.db.Error(err)\ns.Equal(0, n)"
);

// ---------------------------------------------------------------------------
// Go: `errors.Is` / `errors.As` in the condition that fails the test.
// ---------------------------------------------------------------------------

reported!(
    go_errors_is_condition_replaced_by_error_is_reported,
    Go,
    "if !errors.Is(err, ErrSpecific) {\n\tt.Fatalf(\"got %v\", err)\n}",
    "require.Error(t, err)",
    "type was removed"
);

reported!(
    go_errors_is_condition_replaced_by_a_nil_check_is_reported,
    Go,
    "if err == nil || !errors.Is(err, ErrSpecific) {\n\tt.Fatalf(\"got %v\", err)\n}",
    "if err == nil {\n\tt.Fatalf(\"got %v\", err)\n}",
    "expected exception `ErrSpecific` is no longer checked"
);

reported!(
    go_errors_as_condition_replaced_by_a_nil_check_is_reported,
    Go,
    "var pe *fs.PathError\nif !errors.As(err, &pe) {\n\tt.Fatal(err)\n}",
    "var pe *fs.PathError\nif err == nil {\n\tt.Fatal(err)\n}",
    "expected exception `PathError` is no longer checked"
);

silent!(
    go_errors_is_condition_moved_to_testify_is_silent,
    Go,
    "if !errors.Is(err, ErrSpecific) {\n\tt.Fatalf(\"got %v\", err)\n}",
    "require.ErrorIs(t, err, ErrSpecific)"
);

// The condition fails the test when the error IS the sentinel: no expected failure.
silent!(
    go_errors_is_unnegated_condition_is_silent,
    Go,
    "if errors.Is(err, ErrSpecific) {\n\tt.Fatal(err)\n}",
    "if err != nil {\n\tt.Fatal(err)\n}"
);

silent!(
    go_is_of_another_errors_package_is_silent,
    Go,
    ["\"testing\"\n\n\"example.com/own/errors\""],
    "if !errors.Is(err, ErrSpecific) {\n\tt.Fatal(err)\n}",
    "if err == nil {\n\tt.Fatal(err)\n}"
);

// ---------------------------------------------------------------------------
// Go: gocheck checkers.
// ---------------------------------------------------------------------------

reported!(
    gocheck_error_matches_any_message_is_reported,
    GoCheck,
    "c.Assert(err, ErrorMatches, \"negative value\")",
    "c.Assert(err, ErrorMatches, \".*\")",
    "now accepts any message"
);

reported!(
    gocheck_panic_matches_any_message_is_reported,
    GoCheck,
    "c.Assert(func() { f(-1) }, PanicMatches, \"boom.*\")",
    "c.Assert(func() { f(-1) }, PanicMatches, \".*\")",
    "now accepts any message"
);

reported!(
    gocheck_panics_value_replaced_by_any_pattern_is_reported,
    GoCheck,
    "c.Assert(func() { f(-1) }, Panics, \"boom\")",
    "c.Assert(func() { f(-1) }, PanicMatches, \".*\")",
    "now accepts any message"
);

reported!(
    gocheck_error_matches_replaced_by_an_equality_is_reported,
    GoCheck,
    "c.Assert(err, ErrorMatches, \"negative value\")",
    "c.Assert(n, Equals, 0)",
    "expected message `negative value` is no longer checked"
);

reported!(
    gocheck_qualified_checker_is_reported,
    GoCheck,
    ["\"testing\"\n\ngc \"gopkg.in/check.v1\""],
    "c.Assert(err, gc.ErrorMatches, \"negative value\")",
    "c.Assert(err, gc.ErrorMatches, \".*\")",
    "now accepts any message"
);

silent!(
    gocheck_error_matches_pattern_replaced_is_silent,
    GoCheck,
    "c.Assert(err, ErrorMatches, \"negative .*\")",
    "c.Assert(err, ErrorMatches, \"neg.* value\")"
);

silent!(
    gocheck_error_matches_narrowed_is_silent,
    GoCheck,
    "c.Assert(err, ErrorMatches, \".*\")",
    "c.Assert(err, ErrorMatches, \"negative value\")"
);

// `ErrorMatches` is no gocheck checker in a file that does not dot-import the package.
silent!(
    gocheck_checker_of_another_package_is_silent,
    GoCheck,
    ["\"testing\"\n\n. \"example.com/own/check\""],
    "c.Assert(err, ErrorMatches, \"negative value\")",
    "c.Assert(err, ErrorMatches, \".*\")"
);

// ---------------------------------------------------------------------------
// Swift: XCTest.
// ---------------------------------------------------------------------------

reported!(
    xctest_closure_type_check_removed_is_reported,
    XCTest,
    "XCTAssertThrowsError(try f(-1)) { error in\n    XCTAssertTrue(error is SutError)\n}\nXCTAssertEqual(n, 0)",
    "XCTAssertThrowsError(try f(-1))\nXCTAssertEqual(n, 0)\nXCTAssertEqual(m, 0)",
    "type was removed"
);

reported!(
    xctest_closure_cast_moved_to_any_error_is_reported,
    XCTest,
    "XCTAssertThrowsError(try f(-1)) { error in\n    XCTAssertNotNil(error as? SutError)\n}",
    "XCTAssertThrowsError(try f(-1)) { error in\n    XCTAssertNotNil(error as? any Error)\n}",
    "from `SutError` to `any Error`"
);

reported!(
    xctest_throws_error_replaced_by_an_equality_is_reported,
    XCTest,
    "XCTAssertThrowsError(try f(-1))",
    "XCTAssertEqual(n, 0)",
    "expected failure is no longer checked"
);

silent!(
    xctest_closure_type_replaced_is_silent,
    XCTest,
    "XCTAssertThrowsError(try f(-1)) { error in\n    XCTAssertTrue(error is SutError)\n}",
    "XCTAssertThrowsError(try f(-1)) { error in\n    XCTAssertTrue(error is Client.Error)\n}"
);

silent!(
    xctest_closure_type_added_is_silent,
    XCTest,
    "XCTAssertThrowsError(try f(-1)) { error in\n    XCTAssertTrue(error is any Error)\n}",
    "XCTAssertThrowsError(try f(-1)) { error in\n    XCTAssertTrue(error is SutError)\n}"
);

// A sign flip is another assertion, not a wider one.
silent!(
    xctest_throws_error_replaced_by_no_throw_is_silent,
    XCTest,
    "XCTAssertThrowsError(try f(-1))",
    "XCTAssertNoThrow(try f(-1))"
);

// `XCTAssertThrowsError` is a function of the project in a file that does not import XCTest.
silent!(
    xctest_names_without_the_import_are_silent,
    XCTest,
    ["import Foundation"],
    "XCTAssertThrowsError(try f(-1))",
    "XCTAssertEqual(n, 0)"
);

// ---------------------------------------------------------------------------
// Swift: Swift Testing.
// ---------------------------------------------------------------------------

reported!(
    swift_testing_type_moved_to_any_error_is_reported,
    SwiftTesting,
    "#expect(throws: SutError.self) {\n    try f(-1)\n}",
    "#expect(throws: (any Error).self) {\n    try f(-1)\n}",
    "from `SutError` to `any Error`"
);

reported!(
    swift_testing_require_type_moved_to_error_is_reported,
    SwiftTesting,
    "try #require(throws: SutError.self) { try f(-1) }",
    "try #require(throws: Error.self) { try f(-2) }",
    "from `SutError` to `any Error`"
);

reported!(
    swift_testing_error_value_replaced_by_its_type_is_reported,
    SwiftTesting,
    "#expect(throws: SutError.negative) { try f(-1) }",
    "#expect(throws: SutError.self) { try f(-1) }",
    "matcher was removed"
);

reported!(
    swift_testing_throws_replaced_by_a_comparison_is_reported,
    SwiftTesting,
    "#expect(throws: SutError.self) { try f(-1) }",
    "#expect(n > 0)",
    "expected exception `SutError` is no longer checked"
);

silent!(
    swift_testing_type_replaced_is_silent,
    SwiftTesting,
    "#expect(throws: SutError.self) { try f(-1) }",
    "#expect(throws: Client.Error.self) { try f(-1) }"
);

silent!(
    swift_testing_type_narrowed_to_a_value_is_silent,
    SwiftTesting,
    "#expect(throws: SutError.self) { try f(-1) }",
    "#expect(throws: SutError.negative) { try f(-1) }"
);

silent!(
    swift_testing_throws_replaced_by_never_is_silent,
    SwiftTesting,
    "#expect(throws: SutError.self) { try f(-1) }",
    "#expect(throws: Never.self) { try f(-1) }"
);

silent!(
    swift_testing_macro_without_the_import_is_silent,
    SwiftTesting,
    ["import Foundation"],
    "#expect(throws: SutError.self) { try f(-1) }",
    "#expect(throws: (any Error).self) { try f(-1) }"
);

// ---------------------------------------------------------------------------
// Scala: ScalaTest and specs2, on Java's table.
// ---------------------------------------------------------------------------

reported!(
    scalatest_intercept_widened_is_reported,
    ScalaTest,
    "intercept[NumberFormatException] { f(-1) }",
    "intercept[IllegalArgumentException] { f(-1) }",
    "from `NumberFormatException` to `IllegalArgumentException`"
);

reported!(
    scalatest_assert_throws_moved_to_exception_is_reported,
    ScalaTest,
    "assertThrows[OrderError] {\n  f(-1)\n}",
    "assertThrows[Exception] {\n  f(-2)\n}",
    "from `OrderError` to `Exception`"
);

reported!(
    scalatest_the_thrown_by_message_dropped_is_reported,
    ScalaTest,
    "the [IllegalArgumentException] thrownBy { f(-1) } should have message \"negative\"",
    "an [IllegalArgumentException] should be thrownBy { f(-1) }",
    "matcher was removed"
);

reported!(
    scalatest_a_thrown_by_widened_is_reported,
    ScalaTest,
    "a [java.io.FileNotFoundException] should be thrownBy { f(-1) }",
    "an [java.io.IOException] should be thrownBy { f(-1) }",
    "from `FileNotFoundException` to `IOException`"
);

reported!(
    scalatest_intercept_replaced_by_an_equality_is_reported,
    ScalaTest,
    "intercept[IllegalArgumentException] { f(-1) }",
    "assertResult(0)(n)",
    "expected exception `IllegalArgumentException` is no longer checked"
);

const SCALA_ERRORS: &str =
    "class AppError(m: String) extends RuntimeException(m)\nclass OrderError(m: String) extends AppError(m)\nclass PaymentError(m: String) extends AppError(m)\n";

reported!(
    scalatest_project_class_moved_to_its_same_file_base_is_reported,
    ScalaTest,
    [SCALA_ERRORS],
    "intercept[OrderError] { f(-1) }",
    "intercept[AppError] { f(-1) }",
    "from `OrderError` to `AppError`"
);

silent!(
    scalatest_same_file_sibling_replacement_is_silent,
    ScalaTest,
    [SCALA_ERRORS],
    "intercept[OrderError] { f(-1) }",
    "intercept[PaymentError] { f(-1) }"
);

silent!(
    scalatest_intercept_narrowed_is_silent,
    ScalaTest,
    "intercept[IllegalArgumentException] { f(-1) }",
    "intercept[NumberFormatException] { f(-2) }"
);

silent!(
    scalatest_project_class_sharing_a_standard_name_is_silent,
    ScalaTest,
    "intercept[my.pkg.NumberFormatException] { f(-1) }",
    "intercept[IllegalArgumentException] { f(-1) }"
);

silent!(
    scalatest_intercept_replaced_by_no_exception_is_silent,
    ScalaTest,
    "an [IllegalArgumentException] should be thrownBy { f(-1) }",
    "noException should be thrownBy { f(-1) }"
);

// `intercept` of another object is no ScalaTest assertion.
silent!(
    scalatest_intercept_on_another_object_is_silent,
    ScalaTest,
    "proxy.intercept[NumberFormatException] { f(-1) }\nassert(n == 0)",
    "proxy.intercept[IllegalArgumentException] { f(-1) }\nassert(n == 0)"
);

reported!(
    specs2_throw_a_widened_is_reported,
    Specs2,
    "f(-1) must throwA[NumberFormatException]",
    "f(-1) must throwAn[IllegalArgumentException]",
    "from `NumberFormatException` to `IllegalArgumentException`"
);

reported!(
    specs2_throw_a_message_dropped_is_reported,
    Specs2,
    "f(-1) must throwAn[IllegalArgumentException](\"negative\")",
    "f(-1) must throwAn[IllegalArgumentException]",
    "matcher was removed"
);

reported!(
    specs2_throw_a_message_pattern_emptied_is_reported,
    Specs2,
    "f(-1) must throwAn[IllegalArgumentException](message = \"negative.*\")",
    "f(-1) must throwAn[IllegalArgumentException](message = \".*\")",
    "now accepts any message"
);

silent!(
    specs2_throw_a_narrowed_is_silent,
    Specs2,
    "f(-1) must throwAn[IllegalArgumentException]",
    "f(-1) must throwA[NumberFormatException](\"negative\")"
);

// ---------------------------------------------------------------------------
// Objective-C: XCTest.
// ---------------------------------------------------------------------------

reported!(
    objc_throws_specific_moved_to_nsexception_is_reported,
    ObjC,
    "XCTAssertThrowsSpecific([sut run:-1], SutException);",
    "XCTAssertThrowsSpecific([sut run:-1], NSException);",
    "from `SutException` to `NSException`"
);

reported!(
    objc_throws_specific_replaced_by_throws_is_reported,
    ObjC,
    "XCTAssertThrowsSpecific([sut run:-1], SutException, @\"why\");",
    "XCTAssertThrows([sut run:-1], @\"why\");",
    "type was removed"
);

reported!(
    objc_throws_specific_named_name_dropped_is_reported,
    ObjC,
    "XCTAssertThrowsSpecificNamed([sut run:-1], NSException, NSInvalidArgumentException);",
    "XCTAssertThrowsSpecific([sut run:-1], NSException);",
    "matcher was removed"
);

reported!(
    objc_throws_replaced_by_an_equality_is_reported,
    ObjC,
    "XCTAssertThrowsSpecific([sut run:-1], SutException);",
    "XCTAssertEqual(n, 0);",
    "expected exception `SutException` is no longer checked"
);

reported!(
    objc_no_throw_gaining_a_class_is_reported,
    ObjC,
    "XCTAssertNoThrow([sut run:1]);\nXCTAssertEqual(n, 0);",
    "XCTAssertNoThrowSpecific([sut run:1], SutException);\nXCTAssertEqual(n, 0);",
    "negated expectation now names a type"
);

silent!(
    objc_throws_specific_class_replaced_is_silent,
    ObjC,
    "XCTAssertThrowsSpecific([sut run:-1], SutException);",
    "XCTAssertThrowsSpecific([sut run:-1], OtherException);"
);

silent!(
    objc_throws_narrowed_is_silent,
    ObjC,
    "XCTAssertThrows([sut run:-1]);",
    "XCTAssertThrowsSpecificNamed([sut run:-2], NSException, NSRangeException);"
);

silent!(
    objc_throws_replaced_by_no_throw_is_silent,
    ObjC,
    "XCTAssertThrows([sut run:-1]);",
    "XCTAssertNoThrow([sut run:-1]);"
);

silent!(
    objc_no_throw_specific_losing_its_class_is_silent,
    ObjC,
    "XCTAssertNoThrowSpecific([sut run:1], SutException);\nXCTAssertEqual(n, 0);",
    "XCTAssertNoThrow([sut run:1]);\nXCTAssertEqual(n, 0);"
);

not_vacuous!(
    objc_no_throw_specific_only_test_is_not_vacuous,
    ObjC,
    "XCTAssertNoThrowSpecific([sut run:1], SutException);"
);

vacuous!(
    objc_test_without_a_check_is_vacuous,
    ObjC,
    [""],
    "[sut run:1];"
);

// ---------------------------------------------------------------------------
// C++: Catch2 and doctest macros.
// ---------------------------------------------------------------------------

reported!(
    catch2_require_throws_as_widened_is_reported,
    Catch2,
    "REQUIRE_THROWS_AS(f(-1), std::invalid_argument);",
    "REQUIRE_THROWS_AS(f(-1), std::logic_error);",
    "from `invalid_argument` to `logic_error`"
);

reported!(
    catch2_check_throws_as_replaced_by_throws_is_reported,
    Catch2,
    "CHECK_THROWS_AS(f(-1), std::invalid_argument);",
    "CHECK_THROWS(f(-1));",
    "type was removed"
);

reported!(
    catch2_require_throws_with_replaced_by_throws_is_reported,
    Catch2,
    "REQUIRE_THROWS_WITH(f(-1), \"negative\");",
    "REQUIRE_THROWS(f(-1));",
    "matcher was removed"
);

reported!(
    doctest_throws_with_as_message_losing_its_text_is_reported,
    Catch2,
    "CHECK_THROWS_WITH_AS_MESSAGE(f(-1), \"negative\", std::invalid_argument, \"why\");",
    "CHECK_THROWS_AS_MESSAGE(f(-1), std::invalid_argument, \"why\");",
    "matcher was removed"
);

reported!(
    catch2_require_throws_as_replaced_by_an_equality_is_reported,
    Catch2,
    "REQUIRE_THROWS_AS(f(-1), std::invalid_argument);",
    "REQUIRE(n == 0);",
    "expected exception `invalid_argument` is no longer checked"
);

silent!(
    catch2_require_throws_narrowed_is_silent,
    Catch2,
    "REQUIRE_THROWS(f(-1));",
    "REQUIRE_THROWS_AS(f(-2), std::invalid_argument);"
);

silent!(
    catch2_require_throws_replaced_by_nothrow_is_silent,
    Catch2,
    "REQUIRE_THROWS(f(-1));",
    "REQUIRE_NOTHROW(f(-1));"
);

// The trailing argument of a `_MESSAGE` macro is the assertion's own message.
silent!(
    doctest_message_argument_changed_is_silent,
    Catch2,
    "CHECK_THROWS_AS_MESSAGE(f(-1), std::invalid_argument, \"negative value\");",
    "CHECK_THROWS_AS_MESSAGE(f(-1), std::invalid_argument, \"value\");"
);

// ---------------------------------------------------------------------------
// Java: AssertJ typed entry points.
// ---------------------------------------------------------------------------

reported!(
    assertj_typed_entry_point_widened_from_a_subclass_is_reported,
    Java,
    "assertThatExceptionOfType(NumberFormatException.class).isThrownBy(() -> f(-1));",
    "assertThatIllegalArgumentException().isThrownBy(() -> f(-1));",
    "from `NumberFormatException` to `IllegalArgumentException`"
);

reported!(
    assertj_typed_entry_point_message_dropped_is_reported,
    Java,
    "assertThatIllegalStateException().isThrownBy(() -> f(-1)).withMessage(\"negative\");",
    "assertThatIllegalStateException().isThrownBy(() -> f(-1));",
    "matcher was removed"
);

reported!(
    assertj_typed_entry_point_moved_to_its_parent_is_reported,
    Java,
    "assertThatNullPointerException().isThrownBy(() -> f(-1));",
    "assertThatExceptionOfType(RuntimeException.class).isThrownBy(() -> f(-1));",
    "from `NullPointerException` to `RuntimeException`"
);

silent!(
    assertj_typed_entry_point_for_the_same_class_is_silent,
    Java,
    "assertThatExceptionOfType(java.io.IOException.class).isThrownBy(() -> f(-1));",
    "assertThatIOException().isThrownBy(() -> f(-1));"
);

silent!(
    assertj_typed_entry_point_narrowed_is_silent,
    Java,
    "assertThatIllegalArgumentException().isThrownBy(() -> f(-1));",
    "assertThatExceptionOfType(NumberFormatException.class).isThrownBy(() -> f(-1));"
);

// ---------------------------------------------------------------------------
// JavaScript: Chai's `should` style.
// ---------------------------------------------------------------------------

reported!(
    chai_should_throw_message_dropped_is_reported,
    Js,
    "(() => f(-1)).should.throw(RangeError, \"negative\");",
    "(() => f(-1)).should.throw(RangeError);",
    "matcher was removed"
);

reported!(
    chai_should_throw_widened_is_reported,
    Js,
    "(() => f(-1)).should.throw(RangeError);",
    "(() => f(-1)).should.throw(Error);",
    "from `RangeError` to `Error`"
);

reported!(
    chai_should_not_throw_gaining_a_class_is_reported,
    Js,
    "(() => f(1)).should.not.throw();",
    "(() => f(1)).should.not.throw(RangeError);",
    "negated expectation now names a type"
);

silent!(
    chai_should_throw_narrowed_is_silent,
    Js,
    "(() => f(-1)).should.throw(Error);",
    "(() => f(-1)).should.throw(RangeError, \"negative\");"
);

// A stub told to throw is no expectation.
silent!(
    sinon_stub_throws_is_silent,
    Js,
    "stub.throws(new RangeError());\nf(1).should.equal(1);",
    "stub.throws(new Error());\nf(1).should.equal(1);"
);

// ---------------------------------------------------------------------------
// Java: AssertJ `catchThrowable` with the assertions on the caught value.
// ---------------------------------------------------------------------------

reported!(
    assertj_catch_throwable_class_widened_is_reported,
    Java,
    "Throwable thrown = catchThrowable(() -> f(-1));\nassertThat(thrown).isInstanceOf(NumberFormatException.class);",
    "Throwable thrown = catchThrowable(() -> f(-1));\nassertThat(thrown).isInstanceOf(IllegalArgumentException.class);",
    "from `NumberFormatException` to `IllegalArgumentException`"
);

reported!(
    assertj_catch_throwable_message_loosened_is_reported,
    Java,
    "Throwable thrown = catchThrowable(() -> f(-1));\nassertThat(thrown).hasMessage(\"negative\");",
    "Throwable thrown = catchThrowable(() -> f(-1));\nassertThat(thrown).hasMessageContaining(\"negative\");",
    "no longer the whole message"
);

reported!(
    assertj_catch_throwable_of_type_widened_is_reported,
    Java,
    "NumberFormatException e = catchThrowableOfType(() -> f(-1), NumberFormatException.class);\nassertThat(e).hasMessage(\"negative\");",
    "RuntimeException e = catchThrowableOfType(() -> f(-1), RuntimeException.class);\nassertThat(e).hasMessage(\"negative\");",
    "from `NumberFormatException` to `RuntimeException`"
);

reported!(
    assertj_thrown_by_rewritten_as_a_wider_catch_throwable_is_reported,
    Java,
    "assertThatThrownBy(() -> f(-1)).isInstanceOf(NumberFormatException.class);",
    "Throwable thrown = catchThrowable(() -> f(-1));\nassertThat(thrown).isInstanceOf(RuntimeException.class);",
    "from `NumberFormatException` to `RuntimeException`"
);

silent!(
    assertj_thrown_by_rewritten_as_the_same_catch_throwable_is_silent,
    Java,
    "assertThatThrownBy(() -> f(-1)).isInstanceOf(NumberFormatException.class).hasMessage(\"negative\");",
    "Throwable thrown = catchThrowable(() -> f(-1));\nassertThat(thrown).isInstanceOf(NumberFormatException.class).hasMessage(\"negative\");"
);

// An assertion on another value says nothing about the caught one.
silent!(
    assertj_assertion_on_another_value_is_silent,
    Java,
    "Throwable thrown = catchThrowable(() -> f(-1));\nassertThat(other).isInstanceOf(NumberFormatException.class);",
    "Throwable thrown = catchThrowable(() -> f(-1));\nassertThat(other).isInstanceOf(IllegalArgumentException.class);"
);

// ---------------------------------------------------------------------------
// Kotlin: the Kotest assertion on the message of the value `shouldThrow` returns.
// ---------------------------------------------------------------------------

reported!(
    kotest_message_assertion_replaced_is_reported,
    Kotlin,
    "val e = shouldThrow<IllegalArgumentException> { f(-1) }\ne.message shouldBe \"negative\"",
    "val e = shouldThrow<IllegalArgumentException> { f(-1) }\ne.cause shouldBe null",
    "matcher was removed"
);

reported!(
    kotest_message_assertion_loosened_is_reported,
    Kotlin,
    "val e = shouldThrow<IllegalArgumentException> { f(-1) }\ne.message shouldBe \"negative\"",
    "val e = shouldThrow<IllegalArgumentException> { f(-1) }\ne.message shouldContain \"negative\"",
    "no longer the whole message"
);

reported!(
    kotest_chained_message_assertion_shortened_is_reported,
    Kotlin,
    "shouldThrow<IllegalArgumentException> { f(-1) }.message shouldContain \"negative value\"",
    "shouldThrow<IllegalArgumentException> { f(-1) }.message shouldContain \"value\"",
    "was shortened"
);

silent!(
    kotest_message_assertion_tightened_is_silent,
    Kotlin,
    "val e = shouldThrow<IllegalArgumentException> { f(-1) }\ne.message shouldContain \"negative\"",
    "val e = shouldThrow<IllegalArgumentException> { f(-1) }\ne.message shouldBe \"negative\""
);

// The message of another value is no constraint on the exception.
silent!(
    kotest_assertion_on_another_message_is_silent,
    Kotlin,
    "val e = shouldThrow<IllegalArgumentException> { f(-1) }\nother.message shouldBe \"negative\"",
    "val e = shouldThrow<IllegalArgumentException> { f(-1) }\nother.message shouldBe \"value\""
);

// ---------------------------------------------------------------------------
// C#: `Assert.ThrowsException<T>` is exact, and a name only MSTest has. Whose
// `Assert.Throws<T>` replaces it comes from the head file's `using` directives: MSTest's
// accepts subclasses (b), xUnit's and NUnit's is exact (a), and with no framework named
// the `Assert.Throws<T>` opposite an MSTest name is read as MSTest's (c).
// ---------------------------------------------------------------------------

reported!(
    mstest_throws_exception_replaced_by_throws_is_reported,
    MsTest,
    "Assert.ThrowsException<ArgumentException>(() => sut.Run());",
    "Assert.Throws<ArgumentException>(() => sut.Run());",
    "no longer checked as the exact type"
);

reported!(
    mstest_throws_exception_async_replaced_by_throws_async_is_reported,
    MsTest,
    "Assert.ThrowsExceptionAsync<ArgumentException>(() => sut.RunAsync()).Wait();",
    "Assert.ThrowsAsync<ArgumentException>(() => sut.RunAsync()).Wait();",
    "no longer checked as the exact type"
);

silent!(
    mstest_throws_exception_replaced_by_throws_exactly_is_silent,
    MsTest,
    "Assert.ThrowsException<ArgumentException>(() => sut.Run());",
    "Assert.ThrowsExactly<ArgumentException>(() => sut.Run());"
);

silent!(
    mstest_throws_replaced_by_throws_exception_is_silent,
    MsTest,
    "Assert.Throws<ArgumentException>(() => sut.Run());",
    "Assert.ThrowsException<ArgumentException>(() => sut.Run());"
);

// (a) The file has become an xUnit or an NUnit file: exact on both sides.
silent!(
    mstest_throws_exception_replaced_by_xunit_throws_is_silent,
    CSharpBare,
    ["using Microsoft.VisualStudio.TestTools.UnitTesting;" => "using Xunit;"],
    "Assert.ThrowsException<ArgumentException>(() => sut.Run());",
    "Assert.Throws<ArgumentException>(() => sut.Run());"
);

silent!(
    mstest_throws_exactly_replaced_by_nunit_throws_is_silent,
    CSharpBare,
    ["using Microsoft.VisualStudio.TestTools.UnitTesting;" => "using NUnit.Framework;"],
    "Assert.ThrowsExactly<ArgumentException>(() => sut.Run());",
    "Assert.Throws<ArgumentException>(() => sut.Run());"
);

// (b) Without `Assert.ThrowsExactly` anywhere: MSTest's `Assert.Throws<T>` still accepts
// subclasses, so a move to the parent class is a widening.
reported!(
    mstest_throws_moved_to_a_parent_class_is_reported,
    MsTest,
    "Assert.Throws<ArgumentNullException>(() => sut.Run());",
    "Assert.Throws<ArgumentException>(() => sut.Run());",
    "from `ArgumentNullException` to `ArgumentException`"
);

// (c) No directive names a framework: the calls decide.
reported!(
    throws_exception_replaced_by_throws_without_a_framework_directive_is_reported,
    CSharpBare,
    ["using System;"],
    "Assert.ThrowsException<ArgumentException>(() => sut.Run());",
    "Assert.Throws<ArgumentException>(() => sut.Run());",
    "no longer checked as the exact type"
);

silent!(
    throws_moved_to_a_parent_class_without_a_framework_directive_is_silent,
    CSharpBare,
    ["using System;"],
    "Assert.Throws<ArgumentNullException>(() => sut.Run());",
    "Assert.Throws<ArgumentException>(() => sut.Run());"
);

// ---------------------------------------------------------------------------
// Objective-C: `XCTAssertNoThrowSpecific` is an assertion to the counts.
// ---------------------------------------------------------------------------

#[test]
fn objc_no_throw_specific_removed_is_an_assertion_dropped() {
    let run = change(
        Lang::ObjC,
        ("", "XCTAssertNoThrowSpecific([sut run:1], SutException);\nXCTAssertNoThrowSpecificNamed([sut run:2], NSException, NSRangeException);\nXCTAssertEqual(n, 0);"),
        ("", "XCTAssertEqual(n, 0);"),
    );
    let got: Vec<String> = run
        .violations(GATE)
        .iter()
        .map(|v| v["message"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(got.len(), 1, "{}{}", run.stdout, run.stderr);
    assert!(got[0].contains("dropped from 3 to 1"), "{got:?}");
}
