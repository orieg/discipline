//! Counters that decide something about a test from its body (#602): they read code, never
//! the text of a string literal or a comment; an assertion made in a method called on a
//! receiver is followed for `vacuous-tests`; and one assertion counts against a test once.
//!
//! Every source below is a fixture handed to the binary, in each language that has the
//! counter. The fixtures are ordinary string literals inside these test bodies on purpose:
//! they hold the very text (`thread::sleep(`, `.is_ok()`, `time.sleep(1)`) that the
//! counters used to read out of a string, so this repository's own check of this file is
//! one more run of the same tests.
//!
//! A test collects one line per language whose verdict is not the expected one, and
//! asserts that the list is empty.

mod common;
use common::{Repo, Run};

const VACUOUS: &str = "vacuous-tests";
const IGNORED: &str = "ignored-tests";
const REDUCTION: &str = "assertion-reduction";
const SLEEP: &str = "Test Sleep Added";
const TRIVIAL: &str = "Test Asserts Only Trivial Properties";
const NO_ASSERTION: &str = "Vacuous Test Added";

/// How one language spells a test file with one test.
struct Lang {
    name: &'static str,
    path: &'static str,
    /// The file holding one test with this body.
    test: fn(&str) -> String,
    /// An assertion that checks a value.
    real: &'static str,
    /// A statement whose string literal holds the text of a delay. The counter read it
    /// before this change in every language but Go, PHP and Objective-C, where no
    /// position of a string that the counter read was found.
    delay_text: &'static str,
    /// A real delay.
    delay: &'static str,
    /// The test's only assertion, a real one, with the text of a trivial assertion in a
    /// string literal (read before this change in the same languages as `delay_text`).
    trivial_text: Option<&'static str>,
    /// The test's only assertion, a trivial one.
    trivial: Option<&'static str>,
    /// The file holding a type with a method `done` of this body, and one test that
    /// builds a value of the type and calls `done` on it.
    method: fn(&str) -> String,
    method_asserts: &'static str,
    method_asserts_nothing: &'static str,
}

fn langs() -> Vec<Lang> {
    vec![
        Lang {
            name: "python",
            path: "tests/test_t.py",
            test: |b| format!("def test_a():\n{b}"),
            real: "    assert f() == 1\n",
            delay_text: "    src = \"time.sleep(1)\".strip()\n",
            delay: "    time.sleep(1)\n",
            trivial_text: Some("    assert f() == \"x is not None\"\n"),
            trivial: Some("    assert f() is not None\n"),
            method: |b| {
                format!("class V:\n    def done(self):\n{b}\n\ndef test_a():\n    v = V()\n    v.done()\n")
            },
            method_asserts: "        assert self.items == []\n",
            method_asserts_nothing: "        self.items.clear()\n",
        },
        Lang {
            name: "rust",
            path: "tests/t.rs",
            test: |b| format!("#[test]\nfn adds() {{\n{b}}}\n"),
            real: "    assert_eq!(add(2, 2), 4);\n",
            delay_text: "    let src = format!(\"{} thread::sleep(d)\", 1);\n",
            delay: "    std::thread::sleep(d);\n",
            trivial_text: Some("    assert_eq!(render(), \"r.is_ok()\");\n"),
            trivial: Some("    assert!(run().is_ok());\n"),
            method: |b| {
                format!("struct V(Vec<u8>);\n\nimpl V {{\n    fn done(self) {{\n{b}    }}\n}}\n\n#[test]\nfn adds() {{\n    let v = V(run());\n    v.done();\n}}\n")
            },
            method_asserts: "        assert_eq!(self.0.len(), 0, \"left\");\n",
            method_asserts_nothing: "        drop(self.0);\n",
        },
        Lang {
            name: "javascript",
            path: "a.test.js",
            test: |b| format!("test('adds', () => {{\n{b}}});\n"),
            real: "  expect(add(2, 2)).toBe(4);\n",
            delay_text: "  const src = \"setTimeout(f, 1)\".trim();\n",
            delay: "  setTimeout(f, 1);\n",
            trivial_text: Some("  expect(\"x.toBeDefined()\").toBe(render());\n"),
            trivial: Some("  expect(run()).toBeDefined();\n"),
            method: |b| {
                format!("class V {{\n  done() {{\n{b}  }}\n}}\n\ntest('adds', () => {{\n  const v = new V();\n  v.done();\n}});\n")
            },
            method_asserts: "    expect(this.items).toEqual([]);\n",
            method_asserts_nothing: "    this.items = [];\n",
        },
        Lang {
            name: "java",
            path: "src/test/java/ATest.java",
            test: |b| {
                format!("import static org.junit.jupiter.api.Assertions.*;\nimport org.junit.jupiter.api.Test;\n\nclass ATest {{\n    @Test\n    void adds() {{\n{b}    }}\n}}\n")
            },
            real: "        assertEquals(4, add(2, 2));\n",
            delay_text: "        String src = \"Thread.sleep(5)\".trim();\n",
            delay: "        Thread.sleep(5);\n",
            trivial_text: Some("        assertTrue(\"assertNotNull(x)\".equals(render()));\n"),
            trivial: Some("        assertNotNull(run());\n"),
            method: |b| {
                format!("import static org.junit.jupiter.api.Assertions.*;\nimport org.junit.jupiter.api.Test;\n\nclass V {{\n    void done() {{\n{b}    }}\n}}\n\nclass ATest {{\n    @Test\n    void adds() {{\n        V v = new V();\n        v.done();\n    }}\n}}\n")
            },
            method_asserts: "        assertEquals(0, items.size());\n",
            method_asserts_nothing: "        items.clear();\n",
        },
        Lang {
            name: "kotlin",
            path: "src/test/kotlin/ATest.kt",
            test: |b| {
                format!("import org.junit.jupiter.api.Test\n\nclass ATest {{\n    @Test\n    fun adds() {{\n{b}    }}\n}}\n")
            },
            real: "        assertEquals(4, add(2, 2))\n",
            delay_text: "        val src = listOf(\"Thread.sleep(5)\")\n",
            delay: "        Thread.sleep(5)\n",
            trivial_text: Some("        assertEquals(\"assertNotNull(x)\", render())\n"),
            trivial: Some("        assertNotNull(run())\n"),
            method: |b| {
                format!("import org.junit.jupiter.api.Test\n\nclass V {{\n    fun done() {{\n{b}    }}\n}}\n\nclass ATest {{\n    @Test\n    fun adds() {{\n        val v = V()\n        v.done()\n    }}\n}}\n")
            },
            method_asserts: "        assertEquals(0, items.size)\n",
            method_asserts_nothing: "        items.clear()\n",
        },
        Lang {
            name: "csharp",
            path: "tests/ATests.cs",
            test: |b| {
                format!("using Xunit;\n\npublic class ATests {{\n    [Fact]\n    public void Adds() {{\n{b}    }}\n}}\n")
            },
            real: "        Assert.Equal(4, Add(2, 2));\n",
            delay_text: "        var src = \"Thread.Sleep(5)\".Trim();\n",
            delay: "        Thread.Sleep(5);\n",
            trivial_text: Some("        Assert.True(\"Assert.NotNull(x)\".Equals(Render()));\n"),
            trivial: Some("        Assert.NotNull(Run());\n"),
            method: |b| {
                format!("using Xunit;\n\npublic class V {{\n    public void Done() {{\n{b}    }}\n}}\n\npublic class ATests {{\n    [Fact]\n    public void Adds() {{\n        var v = new V();\n        v.Done();\n    }}\n}}\n")
            },
            method_asserts: "        Assert.Equal(0, items.Count);\n",
            method_asserts_nothing: "        items.Clear();\n",
        },
        Lang {
            name: "go",
            path: "p_test.go",
            test: |b| {
                format!("package p\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) {{\n{b}}}\n")
            },
            real: "\tif add(2, 2) != 4 {\n\t\tt.Fatal(\"x\")\n\t}\n",
            delay_text: "\tsrc := strings.TrimSpace(\"time.Sleep(1)\")\n",
            delay: "\ttime.Sleep(1)\n",
            trivial_text: Some(
                "\tif render() != \"assert.NotNil(t, x)\" {\n\t\tt.Fatal(\"x\")\n\t}\n",
            ),
            trivial: Some("\tassert.NotNil(t, run())\n"),
            method: |b| {
                format!("package p\n\nimport \"testing\"\n\ntype V struct{{ t *testing.T }}\n\nfunc (v *V) done() {{\n{b}}}\n\nfunc TestAdd(t *testing.T) {{\n\tv := &V{{t: t}}\n\tv.done()\n}}\n")
            },
            method_asserts: "\tif len(v.items) != 0 {\n\t\tv.t.Fatal(\"left\")\n\t}\n",
            method_asserts_nothing: "\tv.items = nil\n",
        },
        Lang {
            name: "swift",
            path: "tests/AppTests/ATests.swift",
            test: |b| {
                format!("import XCTest\n\nfinal class ATests: XCTestCase {{\n    func testAdd() {{\n{b}    }}\n}}\n")
            },
            real: "        XCTAssertEqual(add(2, 2), 4)\n",
            delay_text: "        let src = String(\"Thread.sleep(1)\")\n",
            delay: "        Thread.sleep(forTimeInterval: 1)\n",
            trivial_text: Some("        XCTAssertEqual(render(), \"XCTAssertNotNil(x)\")\n"),
            trivial: Some("        XCTAssertNotNil(run())\n"),
            method: |b| {
                format!("import XCTest\n\nfinal class V {{\n    func done() {{\n{b}    }}\n}}\n\nfinal class ATests: XCTestCase {{\n    func testAdd() {{\n        let v = V()\n        v.done()\n    }}\n}}\n")
            },
            method_asserts: "        XCTAssertEqual(items.count, 0)\n",
            method_asserts_nothing: "        items.removeAll()\n",
        },
        Lang {
            name: "scala",
            path: "src/test/scala/ASpec.scala",
            test: |b| {
                format!("class ASpec extends AnyFunSuite {{\n  test(\"adds\") {{\n{b}  }}\n}}\n")
            },
            real: "    assert(add(2, 2) == 4)\n",
            delay_text: "    val src = \"Thread.sleep(5)\".trim()\n",
            delay: "    Thread.sleep(5)\n",
            trivial_text: Some("    assert(\"assertNotNull(x)\".trim() == render())\n"),
            trivial: Some("    assertNotNull(run())\n"),
            method: |b| {
                format!("class V {{\n  def done(): Unit = {{\n{b}  }}\n}}\n\nclass ASpec extends AnyFunSuite {{\n  test(\"adds\") {{\n    val v = new V()\n    v.done()\n  }}\n}}\n")
            },
            method_asserts: "    assert(items.size == 0)\n",
            method_asserts_nothing: "    items.clear()\n",
        },
        Lang {
            name: "ruby",
            path: "test/a_test.rb",
            test: |b| format!("class ATest < Minitest::Test\n  def test_add\n{b}  end\nend\n"),
            real: "    assert_equal 4, add(2, 2)\n",
            delay_text: "    src = \"sleep(1)\".strip\n",
            delay: "    sleep(1)\n",
            trivial_text: Some("    assert_equal \"x.toBeTruthy()\", render\n"),
            trivial: None,
            method: |b| {
                format!("class V\n  def done\n{b}  end\nend\n\nclass ATest < Minitest::Test\n  def test_add\n    v = V.new\n    v.done\n  end\nend\n")
            },
            method_asserts: "    raise \"left\" unless @items.empty?\n",
            method_asserts_nothing: "    @items.clear\n",
        },
        Lang {
            name: "php",
            path: "tests/ATest.php",
            test: |b| {
                format!("<?php\n\nuse PHPUnit\\Framework\\TestCase;\n\nclass ATest extends TestCase\n{{\n    public function testAdd(): void\n    {{\n{b}    }}\n}}\n")
            },
            real: "        $this->assertSame(4, add(2, 2));\n",
            delay_text: "        $src = trim(\"sleep(1)\");\n",
            delay: "        sleep(1);\n",
            trivial_text: Some("        $this->assertSame(\"assertNotNull(x)\", render());\n"),
            trivial: Some("        $this->assertNotNull(run());\n"),
            method: |b| {
                format!("<?php\n\nuse PHPUnit\\Framework\\TestCase;\n\nclass V\n{{\n    public function done(): void\n    {{\n{b}    }}\n}}\n\nclass ATest extends TestCase\n{{\n    public function testAdd(): void\n    {{\n        $v = new V();\n        $v->done();\n    }}\n}}\n")
            },
            method_asserts: "        Assert::assertSame(0, count($this->items));\n",
            method_asserts_nothing: "        $this->items = [];\n",
        },
        Lang {
            name: "cpp",
            path: "tests/a_test.cc",
            test: |b| format!("#include <gtest/gtest.h>\n\nTEST(A, Add) {{\n{b}}}\n"),
            real: "  EXPECT_EQ(add(2, 2), 4);\n",
            delay_text: "  auto n = std::string(\"sleep(1)\").size();\n",
            delay: "  sleep(1);\n",
            trivial_text: Some("  EXPECT_EQ(std::string(\"IsNotNull(x)\").size(), render());\n"),
            trivial: Some("  EXPECT_THAT(run(), NotNull());\n"),
            // Defined outside its class: the form the pack records as a helper.
            method: |b| {
                format!("#include <gtest/gtest.h>\n\nvoid V::done() {{\n{b}}}\n\nTEST(A, Add) {{\n  V v;\n  v.done();\n}}\n")
            },
            method_asserts: "  EXPECT_EQ(items.size(), 0u);\n",
            method_asserts_nothing: "  items.clear();\n",
        },
        Lang {
            name: "objc",
            path: "tests/ATests.m",
            test: |b| {
                format!("#import <XCTest/XCTest.h>\n\n@interface ATests : XCTestCase\n@end\n\n@implementation ATests\n\n- (void)testAdd {{\n{b}}}\n\n@end\n")
            },
            real: "    XCTAssertEqual(add(2, 2), 4);\n",
            delay_text: "    NSString *src = [@\"sleep(1)\" copy];\n",
            delay: "    sleep(1);\n",
            trivial_text: None,
            trivial: None,
            method: |b| {
                format!("#import <XCTest/XCTest.h>\n\n@implementation V\n\n- (void)done {{\n{b}}}\n\n@end\n\n@interface ATests : XCTestCase\n@end\n\n@implementation ATests\n\n- (void)testAdd {{\n    V *v = [V new];\n    [v done];\n}}\n\n@end\n")
            },
            method_asserts: "    XCTAssertEqual(self.items.count, 0);\n",
            method_asserts_nothing: "    [self.items removeAllObjects];\n",
        },
    ]
}

/// Adds `content` at `path` on a branch and runs `check` against the base.
fn added(path: &str, content: &str) -> Run {
    let repo = Repo::new();
    repo.commit_base_files(
        &[("package.json", "{\"devDependencies\": {\"jest\": \"*\"}}\n")],
        "test: base",
    );
    repo.write(path, content);
    repo.commit("test: add a test");
    repo.check(&[])
}

/// Commits `base` at `path`, then `head` on a branch, and runs `check`.
fn changed(path: &str, base: &str, head: &str) -> Run {
    let repo = Repo::new();
    repo.commit_base_files(&[(path, base)], "test: base");
    repo.write(path, head);
    repo.commit("test: change a test");
    repo.check(&[])
}

/// The languages whose run did not report exactly `expected` in `gate`.
#[derive(Default)]
struct Verdicts(Vec<String>);

impl Verdicts {
    fn expect(&mut self, lang: &str, run: &Run, gate: &str, expected: &[&str]) {
        let got = run.titles(gate);
        if got != expected {
            self.0.push(format!(
                "{lang}: {gate} reported {got:?}, expected {expected:?}"
            ));
        }
    }

    fn done(self) {
        assert!(self.0.is_empty(), "\n{}\n", self.0.join("\n"));
    }
}

// ---------------------------------------------------------------------------
// 1. Text inside a string literal or a comment is not code
// ---------------------------------------------------------------------------

/// A test that holds the text of a delay in a string literal carries no delay.
#[test]
fn the_text_of_a_delay_in_a_string_literal_is_not_a_delay() {
    let mut v = Verdicts::default();
    for l in langs() {
        let body = format!("{}{}", l.delay_text, l.real);
        let run = added(l.path, &(l.test)(&body));
        v.expect(l.name, &run, IGNORED, &[]);
        v.expect(l.name, &run, VACUOUS, &[]);
    }
    v.done();
}

/// Control: the same delay as a statement of the test is still reported.
#[test]
fn a_delay_in_the_code_of_a_test_is_still_reported() {
    let mut v = Verdicts::default();
    for l in langs() {
        let body = format!("{}{}", l.delay, l.real);
        let run = added(l.path, &(l.test)(&body));
        v.expect(l.name, &run, IGNORED, &[SLEEP]);
    }
    v.done();
}

/// A real assertion whose operand is a string holding the text of a trivial assertion
/// is not a trivial assertion.
#[test]
fn the_text_of_a_trivial_assertion_in_a_string_literal_is_not_one() {
    let mut v = Verdicts::default();
    for l in langs() {
        let Some(body) = l.trivial_text else {
            continue;
        };
        let run = added(l.path, &(l.test)(body));
        v.expect(l.name, &run, VACUOUS, &[]);
    }
    v.done();
}

/// Control: a test whose only assertion is trivial in its code is still reported.
#[test]
fn a_test_whose_only_assertion_is_trivial_is_still_reported() {
    let mut v = Verdicts::default();
    for l in langs() {
        let Some(body) = l.trivial else {
            continue;
        };
        let run = added(l.path, &(l.test)(body));
        v.expect(l.name, &run, VACUOUS, &[TRIVIAL]);
    }
    v.done();
}

fn rust_test(body: &str) -> String {
    format!("#[test]\nfn adds() {{\n{body}}}\n")
}

/// Rust: a macro's arguments are a token tree. The string literals and comments among its
/// tokens are not code, whichever macro holds them.
#[test]
fn rust_macro_tokens_that_are_strings_or_comments_are_not_code() {
    let real = "    assert_eq!(add(2, 2), 4);\n";
    let mut v = Verdicts::default();
    for (name, statement) in [
        (
            "format! with the text of a predicate",
            "    let s = format!(\"fn t() {{ assert!(r.is_ok()); }} {}\", 1);\n",
        ),
        (
            "a string holding an assertion",
            "    let s = \"assert!(r.is_ok())\";\n",
        ),
        (
            "vec! of fixture lines",
            "    let lines = vec![\"std::thread::sleep(d);\", \"assert!(x.is_some());\"];\n",
        ),
        (
            "a comment inside a macro",
            "    let s = format!(\"{}\", 1 /* r.is_ok() thread::sleep(d) */);\n",
        ),
        (
            "a raw string",
            "    let s = format!(r#\"tokio::time::sleep(d).await; assert!(v.is_some())\"#);\n",
        ),
        (
            "a macro that runs none of its arguments",
            "    let s = stringify!(std::thread::sleep(d));\n",
        ),
    ] {
        let run = added("tests/t.rs", &rust_test(&format!("{statement}{real}")));
        v.expect(name, &run, VACUOUS, &[]);
        v.expect(name, &run, IGNORED, &[]);
    }
    // The assertion's own operand is a string: the test asserts on a value.
    for (name, assertion) in [
        (
            "assert_eq! against a string",
            "    assert_eq!(render(), \"r.is_ok()\");\n",
        ),
        (
            "assert! on contains",
            "    assert!(out.contains(\"x.is_some()\"));\n",
        ),
        (
            "a predicate in the failure message only",
            "    assert_eq!(n, 3, \"ok={}\", r.is_ok());\n",
        ),
    ] {
        let run = added("tests/t.rs", &rust_test(assertion));
        v.expect(name, &run, VACUOUS, &[]);
    }
    v.done();
}

/// Control: what a Rust macro's code tokens say is still read.
#[test]
fn rust_macro_tokens_that_are_code_are_still_read() {
    let real = "    assert_eq!(add(2, 2), 4);\n";
    let mut v = Verdicts::default();
    for (name, assertion) in [
        ("assert! on is_ok", "    assert!(run().is_ok());\n"),
        (
            "assert! on is_some with a message",
            "    assert!(run().is_some(), \"got {}\", 1);\n",
        ),
        (
            "debug_assert! on is_ok",
            "    debug_assert!(run().is_ok());\n",
        ),
        (
            "an assertion inside another macro",
            "    tokio::select! { r = run() => assert!(r.is_ok()) }\n",
        ),
    ] {
        let run = added("tests/t.rs", &rust_test(assertion));
        v.expect(name, &run, VACUOUS, &[TRIVIAL]);
    }
    for (name, statement) in [
        (
            "a delay inside a macro",
            "    tokio::select! { _ = tokio::time::sleep(d) => {} }\n",
        ),
        (
            "a delay beside a string",
            "    wait(\"until\"); std::thread::sleep(d);\n",
        ),
    ] {
        let run = added("tests/t.rs", &rust_test(&format!("{statement}{real}")));
        v.expect(name, &run, IGNORED, &[SLEEP]);
    }
    v.done();
}

/// An interpolated expression is code although it sits in a string: a delay there is
/// still a delay, and the text around it is not.
#[test]
fn an_interpolated_expression_is_still_code() {
    let mut v = Verdicts::default();
    let run = added(
        "a.test.js",
        "test('adds', () => {\n  const s = `sleep(1) ${setTimeout(f, 1)}`;\n  expect(add(2, 2)).toBe(4);\n});\n",
    );
    v.expect("javascript template", &run, IGNORED, &[SLEEP]);
    let run = added(
        "a.test.js",
        "test('adds', () => {\n  const s = `setTimeout(f, 1) ${name}`.trim();\n  expect(add(2, 2)).toBe(4);\n});\n",
    );
    v.expect("javascript template text", &run, IGNORED, &[]);
    v.done();
}

// ---------------------------------------------------------------------------
// This file as its own case
// ---------------------------------------------------------------------------
//
// The three tests below are written the way the defects were met in this repository's
// own tests, so the repository's check of this file exercises the fix: before it, the
// first was reported as asserting only trivial properties, the second as carrying a
// delay, and the third as having no assertion.

/// Its one assertion holds a fixture whose text ends in a call of `is_ok`.
#[test]
fn this_test_asserts_a_value_and_only_spells_a_trivial_assertion() {
    assert_eq!(
        added(
            "tests/t.rs",
            "#[test]\nfn adds() {\n    assert_eq!(render(), \"assert!(r.is_ok())\");\n}\n"
        )
        .titles(VACUOUS),
        Vec::<String>::new()
    );
}

/// Its assertion holds a fixture that spells a delay.
#[test]
fn this_test_waits_for_nothing_and_only_spells_a_delay() {
    assert_eq!(
        added(
            "tests/t.rs",
            "#[test]\nfn adds() {\n    let s = \"std::thread::sleep(d);\";\n    assert_eq!(add(2, 2), 4);\n}\n"
        )
        .titles(IGNORED),
        Vec::<String>::new()
    );
}

/// Collects what went wrong and fails on it: an assertion in a method of a type of
/// this file, as `Verdicts::done` is.
struct Problems(Vec<String>);

impl Problems {
    fn none(self) {
        assert!(self.0.is_empty(), "\n{}\n", self.0.join("\n"));
    }
}

/// Its only assertion is the one `Problems::none` makes.
#[test]
fn this_test_asserts_only_through_a_method_of_a_type_of_this_file() {
    let mut problems = Problems(Vec::new());
    let run = added("tests/t.rs", &rust_test("    assert_eq!(add(2, 2), 4);\n"));
    for gate in [VACUOUS, IGNORED] {
        for title in run.titles(gate) {
            problems.0.push(format!("{gate}: {title}"));
        }
    }
    problems.none();
}

// ---------------------------------------------------------------------------
// 2. An assertion made in a method called on a receiver
// ---------------------------------------------------------------------------

/// A test that ends in `v.done()`, where `done` is a method defined in the same file that
/// asserts, is not a test without an assertion.
#[test]
fn a_method_that_asserts_called_on_a_receiver_is_followed() {
    let mut v = Verdicts::default();
    for l in langs() {
        let run = added(l.path, &(l.method)(l.method_asserts));
        v.expect(l.name, &run, VACUOUS, &[]);
    }
    v.done();
}

/// Control: the same call to a method that asserts nothing leaves the test vacuous.
#[test]
fn a_method_that_asserts_nothing_leaves_the_test_vacuous() {
    let mut v = Verdicts::default();
    for l in langs() {
        let run = added(l.path, &(l.method)(l.method_asserts_nothing));
        v.expect(l.name, &run, VACUOUS, &[NO_ASSERTION]);
    }
    v.done();
}

const TWO_TYPES: &str = "struct A(Vec<u8>);\nstruct B(Vec<u8>);\n\nimpl A {\n    fn done(self) {\nA_BODY    }\n}\n\nimpl B {\n    fn done(self) {\nB_BODY    }\n}\n\n#[test]\nfn adds() {\n    let v = make();\n    v.done();\n}\n";

/// Two methods of one name in the file: which one the receiver runs is not known, so the
/// call is worth what the one that checks least checks. One of them asserting nothing
/// leaves the test vacuous.
#[test]
fn of_several_methods_of_one_name_the_one_that_checks_least_counts() {
    let asserts = "        assert_eq!(self.0.len(), 0);\n";
    let nothing = "        drop(self.0);\n";
    let file = |a: &str, b: &str| TWO_TYPES.replace("A_BODY", a).replace("B_BODY", b);
    for (a, b) in [(asserts, nothing), (nothing, asserts)] {
        let run = added("tests/t.rs", &file(a, b));
        assert_eq!(run.titles(VACUOUS), vec![NO_ASSERTION], "{}", run.stdout);
    }
    // Control: when both assert, the call is followed.
    let run = added("tests/t.rs", &file(asserts, asserts));
    assert_eq!(run.titles(VACUOUS), Vec::<String>::new(), "{}", run.stdout);
    // The same rule in a language with classes.
    let py = |a: &str, b: &str| {
        format!("class A:\n    def done(self):\n{a}\n\nclass B:\n    def done(self):\n{b}\n\ndef test_a():\n    v = make()\n    v.done()\n")
    };
    let (asserts, nothing) = ("        assert self.items == []\n", "        pass\n");
    let run = added("tests/test_t.py", &py(asserts, nothing));
    assert_eq!(run.titles(VACUOUS), vec![NO_ASSERTION], "{}", run.stdout);
    let run = added("tests/test_t.py", &py(asserts, asserts));
    assert_eq!(run.titles(VACUOUS), Vec::<String>::new(), "{}", run.stdout);
}

/// The method is followed as far as a free helper is: its own callees count, and a
/// method that only calls a helper that asserts is followed through it.
#[test]
fn a_method_is_followed_through_the_helpers_it_calls() {
    let through = "fn check(items: &[u8]) {\n    assert_eq!(items.len(), 0);\n}\n\nstruct V(Vec<u8>);\n\nimpl V {\n    fn done(self) {\n        check(&self.0);\n    }\n}\n\n#[test]\nfn adds() {\n    let v = V(run());\n    v.done();\n}\n";
    let run = added("tests/t.rs", through);
    assert_eq!(run.titles(VACUOUS), Vec::<String>::new(), "{}", run.stdout);
    // Control: the chain ends in a function that asserts nothing.
    let run = added(
        "tests/t.rs",
        &through.replace("    assert_eq!(items.len(), 0);\n", "    let _ = items;\n"),
    );
    assert_eq!(run.titles(VACUOUS), vec![NO_ASSERTION], "{}", run.stdout);
}

/// A method that asserts keeps a test that also makes a trivial assertion from reading as
/// trivial only, and does not hide a test that has nothing else.
#[test]
fn a_followed_method_counts_beside_a_trivial_assertion() {
    let file = |method: &str| {
        format!("struct V(Vec<u8>);\n\nimpl V {{\n    fn done(self) {{\n{method}    }}\n}}\n\n#[test]\nfn adds() {{\n    let v = V(run());\n    assert!(v.0.first().is_some());\n    v.done();\n}}\n")
    };
    let run = added(
        "tests/t.rs",
        &file("        assert_eq!(self.0.len(), 1);\n"),
    );
    assert_eq!(run.titles(VACUOUS), Vec::<String>::new(), "{}", run.stdout);
    // Control.
    let run = added("tests/t.rs", &file("        drop(self.0);\n"));
    assert_eq!(run.titles(VACUOUS), vec![TRIVIAL], "{}", run.stdout);
}

/// The other gate. `assertion-reduction` counts a test as it did (the test's own count
/// goes from three to none here), and since #595 it reads the call as a move: a call to a
/// same-file method on a receiver stands for the checks that method holds. Assertions
/// replaced by a call to a method holding the same three report nothing, with a note.
/// Control: a method that holds fewer than the test dropped leaves a decrease.
#[test]
fn assertion_reduction_reads_a_move_into_a_method_called_on_a_receiver() {
    let file = |checks: &str, body: &str| {
        format!("struct Checker;\n\nimpl Checker {{\n    fn check(&self, r: &R) {{\n{checks}    }}\n}}\n\n#[test]\nfn create() {{\n    let r = &make();\n{body}}}\n")
    };
    let three =
        "        assert_eq!(r.a, 1);\n        assert_eq!(r.b, 2);\n        assert_eq!(r.c, 3);\n";
    let inline = "    assert_eq!(r.a, 1);\n    assert_eq!(r.b, 2);\n    assert_eq!(r.c, 3);\n";
    let call = "    Checker.check(r);\n";

    let run = changed("tests/t.rs", &file(three, inline), &file(three, call));
    assert_eq!(
        run.titles(REDUCTION),
        Vec::<String>::new(),
        "{}",
        run.stdout
    );
    let notes = run.outcome(REDUCTION)["notes"].to_string();
    assert!(
        notes.contains("assertions 3 -> 0 read as moved into helper `check` (3 check(s))"),
        "{notes}"
    );

    let one = "        assert_eq!(r.a, 1);\n";
    let run = changed("tests/t.rs", &file(one, inline), &file(one, call));
    assert_eq!(
        run.titles(REDUCTION),
        vec!["Assertion Count Decreased In Existing Test"],
        "{}",
        run.stdout
    );
}

// ---------------------------------------------------------------------------
// 3. One assertion counts against a test once
// ---------------------------------------------------------------------------

/// How one language swallows the failure of an assertion.
struct Swallow {
    name: &'static str,
    /// Wraps one line of assertions in a handler that swallows their failure.
    handler: fn(&str) -> String,
    tautology: &'static str,
    one: &'static str,
    two_on_a_line: &'static str,
}

fn swallows() -> Vec<Swallow> {
    vec![
        Swallow {
            name: "python",
            handler: |a| {
                format!("    try:\n        {a}\n    except AssertionError:\n        pass\n")
            },
            tautology: "assert True",
            one: "assert g() == 2",
            two_on_a_line: "assert g() == 2; assert h() == 3",
        },
        Swallow {
            name: "rust",
            handler: |a| format!("    let _ = std::panic::catch_unwind(|| {{ {a} }});\n"),
            tautology: "assert!(true);",
            one: "assert_eq!(g(), 2);",
            two_on_a_line: "assert_eq!(g(), 2); assert_eq!(h(), 3);",
        },
        Swallow {
            name: "javascript",
            handler: |a| format!("  try {{\n    {a}\n  }} catch (e) {{}}\n"),
            tautology: "expect(1).toBe(1);",
            one: "expect(g()).toBe(2);",
            two_on_a_line: "expect(g()).toBe(2); expect(h()).toBe(3);",
        },
        Swallow {
            name: "java",
            handler: |a| {
                format!("        try {{\n            {a}\n        }} catch (AssertionError e) {{\n        }}\n")
            },
            tautology: "assertTrue(true);",
            one: "assertEquals(2, g());",
            two_on_a_line: "assertEquals(2, g()); assertEquals(3, h());",
        },
        Swallow {
            name: "kotlin",
            handler: |a| {
                format!("        try {{\n            {a}\n        }} catch (e: AssertionError) {{\n        }}\n")
            },
            tautology: "assertTrue(true)",
            one: "assertEquals(2, g())",
            two_on_a_line: "assertEquals(2, g()); assertEquals(3, h())",
        },
        Swallow {
            name: "csharp",
            handler: |a| {
                format!("        try {{\n            {a}\n        }} catch (Exception) {{\n        }}\n")
            },
            tautology: "Assert.True(true);",
            one: "Assert.Equal(2, G());",
            two_on_a_line: "Assert.Equal(2, G()); Assert.Equal(3, H());",
        },
        Swallow {
            name: "go",
            handler: |a| format!("\tdefer func() {{ recover() }}()\n\t{a}\n"),
            tautology: "require.True(t, true)",
            one: "require.Equal(t, 2, g())",
            two_on_a_line: "require.Equal(t, 2, g()); require.Equal(t, 3, h())",
        },
    ]
}

/// The real assertion placed before the handler, in the language of `s`.
fn real_before(s: &Swallow) -> (Lang, String) {
    let l = langs().into_iter().find(|l| l.name == s.name).unwrap();
    let real = if s.name == "go" {
        "\trequire.Equal(t, 4, add(2, 2))\n".to_string()
    } else {
        l.real.to_string()
    };
    (l, real)
}

/// An added test with one real assertion, and a tautology whose failure is swallowed, has
/// one effective assertion: the tautology is taken out once, not once as a tautology and
/// once as swallowed.
#[test]
fn a_swallowed_tautology_beside_a_real_assertion_does_not_make_the_test_vacuous() {
    let mut v = Verdicts::default();
    for s in swallows() {
        let (l, real) = real_before(&s);
        let body = format!("{real}{}", (s.handler)(s.tautology));
        let run = added(l.path, &(l.test)(&body));
        v.expect(s.name, &run, VACUOUS, &[]);
    }
    v.done();
}

/// Control: a test whose only assertion is a real one that is swallowed is still vacuous.
#[test]
fn a_test_whose_only_assertion_is_swallowed_is_still_vacuous() {
    let mut v = Verdicts::default();
    for s in swallows() {
        let (l, _) = real_before(&s);
        let run = added(l.path, &(l.test)(&(s.handler)(s.one)));
        v.expect(s.name, &run, VACUOUS, &[NO_ASSERTION]);
    }
    v.done();
}

/// Control: a test whose only assertion is a swallowed tautology is still vacuous.
#[test]
fn a_test_whose_only_assertion_is_a_swallowed_tautology_is_still_vacuous() {
    let mut v = Verdicts::default();
    for s in swallows() {
        let (l, _) = real_before(&s);
        let run = added(l.path, &(l.test)(&(s.handler)(s.tautology)));
        v.expect(s.name, &run, VACUOUS, &[NO_ASSERTION]);
    }
    v.done();
}

/// Two assertions on one line inside one handler are two swallowed assertions: a test
/// that has nothing else has no effective assertion. Keyed on the line, the second was
/// not counted as swallowed and the test read as having one assertion left.
#[test]
fn two_swallowed_assertions_on_one_line_are_both_swallowed() {
    let mut v = Verdicts::default();
    for s in swallows() {
        let (l, _) = real_before(&s);
        let run = added(l.path, &(l.test)(&(s.handler)(s.two_on_a_line)));
        v.expect(s.name, &run, VACUOUS, &[NO_ASSERTION]);
    }
    v.done();
}

/// Control: with a real assertion beside the two swallowed ones the test is not vacuous.
#[test]
fn a_real_assertion_beside_two_swallowed_ones_keeps_the_test() {
    let mut v = Verdicts::default();
    for s in swallows() {
        let (l, real) = real_before(&s);
        let body = format!("{real}{}", (s.handler)(s.two_on_a_line));
        let run = added(l.path, &(l.test)(&body));
        v.expect(s.name, &run, VACUOUS, &[]);
    }
    v.done();
}
