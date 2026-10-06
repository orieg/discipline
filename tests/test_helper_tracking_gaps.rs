//! Gaps in how `assertion-reduction` tracks assertion helpers (#562), in both directions:
//! a helper that loses checks and is not reported, and checks that move into a helper with
//! no loss and are reported anyway. Each shape is driven through the real binary in every
//! language pack that tracks helpers, with a control beside it that pins the other verdict.

mod common;
use common::{Repo, Run, CONFIG_HEAD};

const WEAKENED: &str = "Test Helper Function Weakened";
const DECREASED: &str = "Assertion Count Decreased In Existing Test";

/// How one language spells a shared helper file and a test file that uses it.
struct Lang {
    name: &'static str,
    /// The helper file, a second one that sorts after it, and the test file.
    helpers: &'static str,
    helpers2: &'static str,
    tests: &'static str,
    /// The same helper file under `examples/`.
    example: &'static str,
    /// The `i`th assertion on the value `r`.
    check: fn(usize) -> String,
    /// One helper named `name` with `body`.
    helper: fn(&str, &str) -> String,
    /// A helper file holding `defs`.
    file: fn(&str) -> String,
    /// A helper file holding `defs` and one test that calls none of them.
    file_with_test: fn(&str) -> String,
    /// A statement calling the helper `name` on `r`.
    call: fn(&str) -> String,
    /// A test file whose one test has `body`.
    test: fn(&str) -> String,
    comment: &'static str,
    /// A helper file whose helper `name` is a method of a type, when the language has
    /// a second way to declare a helper.
    method: Option<fn(&str, &str) -> String>,
}

impl Lang {
    fn checks(&self, n: usize) -> String {
        self.checks_from(0, n)
    }

    fn checks_from(&self, start: usize, n: usize) -> String {
        (start..start + n).map(|i| (self.check)(i)).collect()
    }

    /// A helper file with one helper `check` holding `n` assertions.
    fn helper_file(&self, n: usize) -> String {
        (self.file)(&(self.helper)("check", &self.checks(n)))
    }

    /// The same file with a comment added above the helper.
    fn touched_helper_file(&self, n: usize) -> String {
        (self.file)(&format!(
            "{}{}",
            self.comment,
            (self.helper)("check", &self.checks(n))
        ))
    }

    fn inline_test(&self, n: usize) -> String {
        (self.test)(&self.checks(n))
    }

    fn calling_test(&self) -> String {
        (self.test)(&(self.call)("check"))
    }
}

fn langs() -> Vec<Lang> {
    vec![
        Lang {
            name: "python",
            helpers: "tests/helpers.py",
            helpers2: "tests/zother/helpers.py",
            tests: "tests/test_api.py",
            example: "examples/helpers.py",
            check: |i| format!("    assert r.f{i} == {i}\n"),
            helper: |name, body| {
                let body = if body.is_empty() { "    pass\n" } else { body };
                format!("def {name}(r):\n{body}\n")
            },
            file: |defs| defs.to_string(),
            file_with_test: |defs| format!("{defs}def test_unrelated():\n    assert 1 + 1 == 2\n"),
            call: |name| format!("    {name}(r)\n"),
            test: |body| {
                format!("from helpers import *\n\ndef test_create():\n    r = create()\n{body}")
            },
            comment: "# shared\n",
            method: Some(|name, body| {
                let body: String = if body.is_empty() {
                    "        pass\n".to_string()
                } else {
                    body.lines().map(|l| format!("    {l}\n")).collect()
                };
                format!("class Base:\n    def {name}(self, r):\n{body}\n")
            }),
        },
        Lang {
            name: "rust",
            helpers: "tests/common/mod.rs",
            helpers2: "tests/zother/mod.rs",
            tests: "tests/api.rs",
            example: "examples/show.rs",
            check: |i| format!("    assert_eq!(r.f{i}, {i});\n"),
            helper: |name, body| format!("pub fn {name}(r: &R) {{\n{body}}}\n\n"),
            file: |defs| defs.to_string(),
            file_with_test: |defs| {
                format!("{defs}#[test]\nfn unrelated() {{\n    assert_eq!(1 + 1, 2);\n}}\n")
            },
            call: |name| format!("    {name}(r);\n"),
            test: |body| {
                format!("mod common;\nuse common::*;\n\n#[test]\nfn create() {{\n    let r = &make();\n{body}}}\n")
            },
            comment: "// shared\n",
            method: Some(|name, body| {
                format!("pub struct Checker;\n\nimpl Checker {{\n    pub fn {name}(&self, r: &R) {{\n{body}    }}\n}}\n\n")
            }),
        },
        Lang {
            name: "go",
            helpers: "helpers_test.go",
            helpers2: "zother/helpers_test.go",
            tests: "api_test.go",
            example: "examples/helpers.go",
            check: |i| format!("\tif r.F{i} != {i} {{\n\t\tt.Fatal(\"f{i}\")\n\t}}\n"),
            helper: |name, body| {
                format!("func {name}(t *testing.T, r R) {{\n\tt.Helper()\n{body}}}\n\n")
            },
            file: |defs| format!("package x\n\nimport \"testing\"\n\n{defs}"),
            file_with_test: |defs| {
                format!("package x\n\nimport \"testing\"\n\n{defs}func TestUnrelated(t *testing.T) {{\n\tif 1+1 != 2 {{\n\t\tt.Fatal(\"math\")\n\t}}\n}}\n")
            },
            call: |name| format!("\t{name}(t, r)\n"),
            test: |body| {
                format!("package x\n\nimport \"testing\"\n\nfunc TestCreate(t *testing.T) {{\n\tr := create()\n{body}}}\n")
            },
            comment: "// shared\n",
            method: Some(|name, body| {
                format!("type Suite struct{{}}\n\nfunc (s *Suite) {name}(t *testing.T, r R) {{\n\tt.Helper()\n{body}}}\n\n")
            }),
        },
        Lang {
            name: "typescript",
            helpers: "tests/test-utils.ts",
            helpers2: "tests/zother/test-utils.ts",
            tests: "tests/api.test.ts",
            example: "examples/helpers.ts",
            check: |i| format!("  expect(r.f{i}).toBe({i});\n"),
            helper: |name, body| format!("export function {name}(r: R) {{\n{body}}}\n\n"),
            file: |defs| format!("import {{ expect }} from 'vitest';\n\n{defs}"),
            file_with_test: |defs| {
                format!("import {{ expect, test }} from 'vitest';\n\n{defs}test('unrelated', () => {{\n  expect(1 + 1).toBe(2);\n}});\n")
            },
            call: |name| format!("  {name}(r);\n"),
            test: |body| {
                format!("import {{ test, expect }} from 'vitest';\nimport {{ check }} from './test-utils';\n\ntest('create', () => {{\n  const r = create();\n{body}}});\n")
            },
            comment: "// shared\n",
            method: Some(|name, body| {
                format!("export class Checker {{\n  {name}(r: R) {{\n{body}  }}\n}}\n\n")
            }),
        },
        Lang {
            name: "java",
            helpers: "src/test/java/Checks.java",
            helpers2: "src/test/java/zother/Checks.java",
            tests: "src/test/java/ApiTest.java",
            example: "examples/Checks.java",
            check: |i| format!("        assertEquals({i}, r.f{i});\n"),
            helper: |name, body| format!("    static void {name}(R r) {{\n{body}    }}\n\n"),
            file: |defs| {
                format!("import static org.junit.jupiter.api.Assertions.*;\n\nclass Checks {{\n{defs}}}\n")
            },
            file_with_test: |defs| {
                format!("import org.junit.jupiter.api.Test;\nimport static org.junit.jupiter.api.Assertions.*;\n\nclass Checks {{\n{defs}    @Test\n    void unrelated() {{\n        assertEquals(2, 1 + 1);\n    }}\n}}\n")
            },
            call: |name| format!("        {name}(r);\n"),
            test: |body| {
                format!("import org.junit.jupiter.api.Test;\nimport static org.junit.jupiter.api.Assertions.*;\n\nclass ApiTest extends Checks {{\n    @Test\n    void create() {{\n        R r = make();\n{body}    }}\n}}\n")
            },
            comment: "    // shared\n",
            method: None,
        },
        Lang {
            name: "csharp",
            helpers: "tests/Checks.cs",
            helpers2: "tests/zother/Checks.cs",
            tests: "tests/ApiTests.cs",
            example: "examples/Checks.cs",
            check: |i| format!("        Assert.Equal({i}, r.F{i});\n"),
            helper: |name, body| {
                format!("    public static void {name}(R r)\n    {{\n{body}    }}\n\n")
            },
            file: |defs| format!("using Xunit;\n\npublic class Checks\n{{\n{defs}}}\n"),
            file_with_test: |defs| {
                format!("using Xunit;\n\npublic class Checks\n{{\n{defs}    [Fact]\n    public void Unrelated()\n    {{\n        Assert.Equal(2, 1 + 1);\n    }}\n}}\n")
            },
            call: |name| format!("        {name}(r);\n"),
            test: |body| {
                format!("using Xunit;\n\npublic class ApiTests : Checks\n{{\n    [Fact]\n    public void Create()\n    {{\n        var r = Make();\n{body}    }}\n}}\n")
            },
            comment: "    // shared\n",
            method: None,
        },
        Lang {
            name: "kotlin",
            helpers: "src/test/kotlin/Checks.kt",
            helpers2: "src/test/kotlin/zother/Checks.kt",
            tests: "src/test/kotlin/ApiTest.kt",
            example: "examples/Checks.kt",
            check: |i| format!("    assertEquals({i}, r.f{i})\n"),
            helper: |name, body| format!("fun {name}(r: R) {{\n{body}}}\n\n"),
            file: |defs| format!("import kotlin.test.assertEquals\n\n{defs}"),
            file_with_test: |defs| {
                format!("import kotlin.test.Test\nimport kotlin.test.assertEquals\n\n{defs}class UnrelatedTest {{\n    @Test\n    fun unrelated() {{\n        assertEquals(2, 1 + 1)\n    }}\n}}\n")
            },
            call: |name| format!("    {name}(r)\n"),
            test: |body| {
                format!("import kotlin.test.Test\nimport kotlin.test.assertEquals\n\nclass ApiTest {{\n    @Test\n    fun create() {{\n    val r = make()\n{body}    }}\n}}\n")
            },
            comment: "// shared\n",
            method: Some(|name, body| {
                format!("object Checks {{\n    fun {name}(r: R) {{\n{body}    }}\n}}\n\n")
            }),
        },
        Lang {
            name: "swift",
            helpers: "tests/AppTests/Checks.swift",
            helpers2: "tests/ZOther/Checks.swift",
            tests: "tests/AppTests/ApiTests.swift",
            example: "examples/Checks.swift",
            check: |i| format!("    XCTAssertEqual(r.f{i}, {i})\n"),
            helper: |name, body| format!("func {name}(_ r: R) {{\n{body}}}\n\n"),
            file: |defs| format!("import XCTest\n\n{defs}"),
            file_with_test: |defs| {
                format!("import XCTest\n\n{defs}final class UnrelatedTests: XCTestCase {{\n    func testUnrelated() {{\n        XCTAssertEqual(1 + 1, 2)\n    }}\n}}\n")
            },
            call: |name| format!("    {name}(r)\n"),
            test: |body| {
                format!("import XCTest\n\nfinal class ApiTests: XCTestCase {{\n    func testCreate() {{\n    let r = make()\n{body}    }}\n}}\n")
            },
            comment: "// shared\n",
            method: Some(|name, body| {
                format!("extension XCTestCase {{\n    func {name}(_ r: R) {{\n{body}    }}\n}}\n\n")
            }),
        },
        Lang {
            name: "scala",
            helpers: "src/test/scala/Checks.scala",
            helpers2: "src/test/scala/zother/Checks.scala",
            tests: "src/test/scala/ApiSpec.scala",
            example: "examples/Checks.scala",
            check: |i| format!("    assert(r.f{i} == {i})\n"),
            helper: |name, body| format!("  def {name}(r: R): Unit = {{\n{body}  }}\n\n"),
            file: |defs| format!("object Checks {{\n{defs}}}\n"),
            file_with_test: |defs| {
                format!("class Checks extends AnyFunSuite {{\n{defs}  test(\"unrelated\") {{\n    assert(1 + 1 == 2)\n  }}\n}}\n")
            },
            call: |name| format!("    {name}(r)\n"),
            test: |body| {
                format!("import Checks._\n\nclass ApiSpec extends AnyFunSuite {{\n  test(\"create\") {{\n    val r = make()\n{body}  }}\n}}\n")
            },
            comment: "  // shared\n",
            method: None,
        },
        Lang {
            name: "ruby",
            helpers: "test/support/checks.rb",
            helpers2: "test/zother/checks.rb",
            tests: "test/api_test.rb",
            example: "examples/checks.rb",
            check: |i| format!("    assert_equal {i}, r.f{i}\n"),
            helper: |name, body| format!("  def {name}(r)\n{body}  end\n\n"),
            file: |defs| format!("module Checks\n{defs}end\n"),
            file_with_test: |defs| {
                format!("class ChecksTest < Minitest::Test\n{defs}  def test_unrelated\n    assert_equal 2, 1 + 1\n  end\nend\n")
            },
            call: |name| format!("    {name}(r)\n"),
            test: |body| {
                format!("require_relative 'support/checks'\n\nclass ApiTest < Minitest::Test\n  include Checks\n\n  def test_create\n    r = make\n{body}  end\nend\n")
            },
            comment: "  # shared\n",
            method: None,
        },
        Lang {
            name: "php",
            helpers: "tests/Checks.php",
            helpers2: "tests/zother/Checks.php",
            tests: "tests/ApiTest.php",
            example: "examples/Checks.php",
            check: |i| format!("        $this->assertSame({i}, $r->f{i});\n"),
            helper: |name, body| {
                format!("    protected function {name}($r): void\n    {{\n{body}    }}\n\n")
            },
            file: |defs| format!("<?php\n\ntrait Checks\n{{\n{defs}}}\n"),
            file_with_test: |defs| {
                format!("<?php\n\nclass ChecksTest extends \\PHPUnit\\Framework\\TestCase\n{{\n{defs}    public function testUnrelated(): void\n    {{\n        $this->assertSame(2, 1 + 1);\n    }}\n}}\n")
            },
            call: |name| format!("        $this->{name}($r);\n"),
            test: |body| {
                format!("<?php\n\nuse PHPUnit\\Framework\\TestCase;\n\nclass ApiTest extends TestCase\n{{\n    use Checks;\n\n    public function testCreate(): void\n    {{\n        $r = make();\n{body}    }}\n}}\n")
            },
            comment: "    // shared\n",
            method: None,
        },
        Lang {
            name: "cpp",
            helpers: "tests/checks.cc",
            helpers2: "tests/zother/checks.cc",
            tests: "tests/api_test.cc",
            example: "examples/checks.cc",
            check: |i| format!("  EXPECT_EQ(r.f{i}, {i});\n"),
            helper: |name, body| format!("void {name}(const R& r) {{\n{body}}}\n\n"),
            file: |defs| format!("#include <gtest/gtest.h>\n\n{defs}"),
            file_with_test: |defs| {
                format!("#include <gtest/gtest.h>\n\n{defs}TEST(Unrelated, Math) {{\n  EXPECT_EQ(1 + 1, 2);\n}}\n")
            },
            call: |name| format!("  {name}(r);\n"),
            test: |body| {
                format!(
                    "#include <gtest/gtest.h>\n\nTEST(Api, Create) {{\n  R r = Make();\n{body}}}\n"
                )
            },
            comment: "// shared\n",
            method: Some(|name, body| format!("void Fixture::{name}(const R& r) {{\n{body}}}\n\n")),
        },
        Lang {
            name: "objc",
            helpers: "tests/Checks.m",
            helpers2: "tests/ZOther/Checks.m",
            tests: "tests/ApiTests.m",
            example: "examples/Checks.m",
            check: |i| format!("    XCTAssertEqual(r.f{i}, {i});\n"),
            helper: |name, body| format!("void {name}(R *r) {{\n{body}}}\n\n"),
            file: |defs| format!("#import <XCTest/XCTest.h>\n\n{defs}"),
            file_with_test: |defs| {
                format!("#import <XCTest/XCTest.h>\n\n{defs}@interface UnrelatedTests : XCTestCase\n@end\n\n@implementation UnrelatedTests\n\n- (void)testUnrelated {{\n    XCTAssertEqual(1 + 1, 2);\n}}\n\n@end\n")
            },
            call: |name| format!("    {name}(r);\n"),
            test: |body| {
                format!("#import <XCTest/XCTest.h>\n\n@interface ApiTests : XCTestCase\n@end\n\n@implementation ApiTests\n\n- (void)testCreate {{\n    R *r = make();\n{body}}}\n\n@end\n")
            },
            comment: "// shared\n",
            method: Some(|name, body| {
                format!("@implementation Base\n\n- (void){name}:(R *)r {{\n{body}}}\n\n@end\n\n")
            }),
        },
    ]
}

/// Commits `base` on `main`, then `head` (and the removal of `delete`) on a branch with
/// `body` in the commit message, and runs `check`.
fn change(base: &[(&str, &str)], head: &[(&str, &str)], body: &str) -> Run {
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
    repo.commit(&format!("refactor: change\n\n{body}"));
    repo.check(&["--base", "main"])
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

/// Collects one line per language whose verdict is not the expected one, so one run
/// names every language a shape fails in. Each test asserts the list is empty itself.
#[derive(Default)]
struct Verdicts(Vec<String>);

impl Verdicts {
    fn clean(&mut self, lang: &str, run: &Run) {
        let got = reported(run);
        if run.code != 0 || !got.is_empty() {
            self.0.push(format!(
                "{lang}: expected nothing reported, got exit {} {got:?}",
                run.code
            ));
        }
    }

    fn reports(&mut self, lang: &str, run: &Run, title: &str, file: &str) {
        let got = reported(run);
        let expected = vec![(title.to_string(), file.to_string())];
        if run.code != 1 || got != expected {
            self.0.push(format!(
                "{lang}: expected {expected:?}, got exit {} {got:?}",
                run.code
            ));
        }
    }
}

// ---- pins: what already held -------------------------------------------------------------

/// Pin: a helper in a file that holds no test loses its checks. PHP's shared helpers are
/// the methods of a trait, which the pack did not read before this change.
#[test]
fn a_helper_that_loses_its_checks_in_a_helper_file_is_reported() {
    let mut v = Verdicts::default();
    for l in langs() {
        let run = change(
            &[(l.helpers, &l.helper_file(3))],
            &[(l.helpers, &l.helper_file(0))],
            "",
        );
        v.reports(l.name, &run, WEAKENED, l.helpers);
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

/// Pin of the rule for a call whose helper was not read: three inline assertions are
/// replaced by a call to a helper in a file the change does not touch. Its body is not
/// read, so it stands for nothing and the drop is reported.
#[test]
fn a_call_to_a_helper_outside_the_change_stands_for_no_dropped_assertion() {
    let mut v = Verdicts::default();
    for l in langs() {
        let run = change(
            &[(l.helpers, &l.helper_file(3)), (l.tests, &l.inline_test(3))],
            &[(l.tests, &l.calling_test())],
            "",
        );
        v.reports(l.name, &run, DECREASED, l.tests);
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

/// Pin: assertions moved into a helper of another changed file that holds as many are a
/// lossless move; into one that holds fewer, or none, a drop. PHP read no trait before
/// this change and reported the lossless move.
#[test]
fn assertions_moved_into_a_changed_helper_file_count_what_the_helper_holds() {
    let mut v = Verdicts::default();
    for l in langs() {
        for (held, lossless) in [(3, true), (1, false), (0, false)] {
            let run = change(
                &[
                    (l.helpers, &l.helper_file(held)),
                    (l.tests, &l.inline_test(3)),
                ],
                &[
                    (l.helpers, &l.touched_helper_file(held)),
                    (l.tests, &l.calling_test()),
                ],
                "",
            );
            let name = format!("{} ({held} held)", l.name);
            if lossless {
                v.clean(&name, &run);
            } else {
                v.reports(&name, &run, DECREASED, l.tests);
            }
        }
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

// ---- passes unreported -------------------------------------------------------------------

/// A helper file that also holds one test was not tracked at all: gutting its helper
/// passed. The test here calls no helper, so only the helper can carry the finding.
#[test]
fn a_helper_that_loses_its_checks_beside_an_unrelated_test_is_reported() {
    let mut v = Verdicts::default();
    for l in langs() {
        let held = (l.file_with_test)(&(l.helper)("check", &l.checks(3)));
        let gutted = (l.file_with_test)(&(l.helper)("check", ""));
        let run = change(&[(l.helpers, &held)], &[(l.helpers, &gutted)], "");
        v.reports(l.name, &run, WEAKENED, l.helpers);
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

/// Control: a comment added to such a file loses nothing.
#[test]
fn a_comment_added_beside_a_helper_and_a_test_reports_nothing() {
    let mut v = Verdicts::default();
    for l in langs() {
        let helper = (l.helper)("check", &l.checks(3));
        let held = (l.file_with_test)(&helper);
        let commented = (l.file_with_test)(&format!("{}{helper}", l.comment));
        let run = change(&[(l.helpers, &held)], &[(l.helpers, &commented)], "");
        v.clean(l.name, &run);
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

/// `conftest.py` with a fixture named `test_*`: the pack reads the fixture as a test, so
/// the file held a test and its helpers were not tracked.
#[test]
fn a_conftest_helper_beside_a_fixture_named_like_a_test_is_reported() {
    let fixture = "import pytest\n\n@pytest.fixture\ndef test_client():\n    return make()\n\n";
    let held = "def check(r):\n    assert r.ok == True\n    assert r.ready == True\n";
    let run = change(
        &[("tests/conftest.py", &format!("{fixture}{held}"))],
        &[(
            "tests/conftest.py",
            &format!("{fixture}def check(r):\n    pass\n"),
        )],
        "",
    );
    assert_eq!(
        reported(&run),
        vec![(WEAKENED.to_string(), "tests/conftest.py".to_string())],
        "{}",
        run.stdout
    );
}

/// A helper declared as a method of a type lost its checks unreported in Go (a method
/// with a receiver) and in JS / TS (a class method); the other packs already read
/// methods. Java, C#, Scala, Ruby and PHP declare every helper as a method, which the
/// tests above cover.
#[test]
fn a_method_helper_that_loses_its_checks_is_reported() {
    let mut v = Verdicts::default();
    for l in langs() {
        let Some(method) = l.method else { continue };
        let run = change(
            &[(l.helpers, &(l.file)(&method("check", &l.checks(3))))],
            &[(l.helpers, &(l.file)(&method("check", "")))],
            "",
        );
        v.reports(l.name, &run, WEAKENED, l.helpers);
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

/// Control: a comment above a method helper loses nothing.
#[test]
fn a_comment_added_above_a_method_helper_reports_nothing() {
    let mut v = Verdicts::default();
    for l in langs() {
        let Some(method) = l.method else { continue };
        let helper = method("check", &l.checks(3));
        let run = change(
            &[(l.helpers, &(l.file)(&helper))],
            &[(l.helpers, &(l.file)(&format!("{}{helper}", l.comment)))],
            "",
        );
        v.clean(l.name, &run);
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

/// `check` holds one assertion and calls `check_body`, which holds two.
fn with_sub_helper(l: &Lang, called: bool, sub_checks: usize) -> String {
    let call = if called {
        (l.call)("check_body")
    } else {
        String::new()
    };
    (l.file)(&format!(
        "{}{}",
        (l.helper)("check", &format!("{}{call}", l.checks(1))),
        (l.helper)("check_body", &l.checks_from(1, sub_checks))
    ))
}

/// A helper's count was its own body only, so a helper that stopped calling the helper
/// holding most of its checks passed.
#[test]
fn a_helper_that_stops_calling_a_checking_helper_is_reported() {
    let mut v = Verdicts::default();
    for l in langs() {
        let run = change(
            &[(l.helpers, &with_sub_helper(&l, true, 2))],
            &[(l.helpers, &with_sub_helper(&l, false, 2))],
            "",
        );
        v.reports(l.name, &run, WEAKENED, l.helpers);
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

/// The same reading reported a lossless extraction: three assertions become one and a
/// call to a new helper holding the other two.
#[test]
fn checks_extracted_into_a_helper_the_helper_calls_report_nothing() {
    let mut v = Verdicts::default();
    for l in langs() {
        let run = change(
            &[(l.helpers, &l.helper_file(3))],
            &[(l.helpers, &with_sub_helper(&l, true, 2))],
            "",
        );
        v.clean(l.name, &run);
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

/// Control: an extraction that keeps two of the three assertions is a drop.
#[test]
fn an_extraction_that_loses_a_check_is_reported() {
    let mut v = Verdicts::default();
    for l in langs() {
        let run = change(
            &[(l.helpers, &l.helper_file(3))],
            &[(l.helpers, &with_sub_helper(&l, true, 1))],
            "",
        );
        v.reports(l.name, &run, WEAKENED, l.helpers);
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

/// A helper reached through a second helper, in a file that holds a test: `check_all`
/// calls `check`, and `check` loses its checks. The file's test calls neither, so the
/// loss is the helpers' alone, reported once on the helper whose body changed.
#[test]
fn a_helper_reached_through_a_second_helper_that_loses_its_checks_is_reported() {
    let mut v = Verdicts::default();
    for l in langs() {
        let chain = |held: usize| {
            (l.file_with_test)(&format!(
                "{}{}",
                (l.helper)("check_all", &(l.call)("check")),
                (l.helper)("check", &l.checks(held))
            ))
        };
        let run = change(&[(l.helpers, &chain(3))], &[(l.helpers, &chain(0))], "");
        v.reports(l.name, &run, WEAKENED, l.helpers);
        let message = run
            .violations("assertion-reduction")
            .first()
            .and_then(|v| v["message"].as_str().map(str::to_string))
            .unwrap_or_default();
        // Named for `check`, whose body changed, not for `check_all`, which only calls it.
        if !message.contains("check`: ") || message.contains("check_all") {
            v.0.push(format!("{}: {message}", l.name));
        }
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

/// A directive that names a weakened helper lifted an unrelated drop in a test that calls
/// it: the helper is in another file, so nothing of it is counted in the test, and the
/// assertion the test lost is its own.
#[test]
fn a_directive_naming_a_helper_does_not_lift_the_calling_tests_own_drop() {
    let mut v = Verdicts::default();
    for l in langs() {
        let test =
            |own: usize| (l.test)(&format!("{}{}", (l.call)("check"), l.checks_from(5, own)));
        let run = change(
            &[(l.helpers, &l.helper_file(2)), (l.tests, &test(3))],
            &[(l.helpers, &l.helper_file(1)), (l.tests, &test(2))],
            "allow-assertion-drop: check the second field moved to the model",
        );
        v.reports(l.name, &run, DECREASED, l.tests);
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

/// Control: with the test's own assertions kept, the directive lifts the helper's finding
/// and nothing is left.
#[test]
fn a_directive_naming_a_helper_lifts_the_helpers_finding() {
    let mut v = Verdicts::default();
    for l in langs() {
        let test = (l.test)(&format!("{}{}", (l.call)("check"), l.checks_from(5, 3)));
        let run = change(
            &[(l.helpers, &l.helper_file(2)), (l.tests, &test)],
            &[
                (l.helpers, &l.helper_file(1)),
                (l.tests, &format!("{test}{}", l.comment.trim_start())),
            ],
            "allow-assertion-drop: check the second field moved to the model",
        );
        v.clean(l.name, &run);
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

/// The same block cleared a strength drop with no strength check: the helper loses a
/// truthiness assertion, the test loses an equality.
#[test]
fn a_weakened_helper_does_not_stand_for_an_equality_the_calling_test_lost() {
    let helper = |ready: &str| format!("def check(r):\n    assert r.ok == True\n{ready}");
    let test = |own: &str| {
        format!("from helpers import check\n\ndef test_create():\n    r = create()\n    check(r)\n    assert r.status == 200\n{own}")
    };
    let run = change(
        &[
            ("tests/helpers.py", &helper("    assert r.ready\n")),
            ("tests/test_api.py", &test("    assert r.id == 1\n")),
        ],
        &[
            ("tests/helpers.py", &helper("")),
            ("tests/test_api.py", &test("")),
        ],
        "allow-assertion-drop: check readiness moved to the model",
    );
    assert_eq!(
        reported(&run),
        vec![(DECREASED.to_string(), "tests/test_api.py".to_string())],
        "{}",
        run.stdout
    );
}

// ---- reported wrongly --------------------------------------------------------------------

/// A helper renamed with nothing else changed was reported as deleted.
#[test]
fn a_renamed_helper_reports_nothing() {
    let mut v = Verdicts::default();
    for l in langs() {
        let run = change(
            &[(l.helpers, &l.helper_file(3))],
            &[(
                l.helpers,
                &(l.file)(&(l.helper)("check_response", &l.checks(3))),
            )],
            "",
        );
        v.clean(l.name, &run);
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

/// Control: a rename that also drops two of three checks is reported.
#[test]
fn a_renamed_helper_that_loses_checks_is_reported() {
    let mut v = Verdicts::default();
    for l in langs() {
        let run = change(
            &[(l.helpers, &l.helper_file(3))],
            &[(
                l.helpers,
                &(l.file)(&(l.helper)("check_response", &l.checks(1))),
            )],
            "",
        );
        v.reports(l.name, &run, WEAKENED, l.helpers);
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

/// A helper deleted from one file was paired with the unchanged helper of the same name
/// in another changed file, and that one was then reported as deleted in its place.
#[test]
fn a_deleted_helper_is_reported_in_its_own_file() {
    let mut v = Verdicts::default();
    for l in langs() {
        let run = change(
            &[
                (l.helpers, &l.helper_file(2)),
                (l.helpers2, &l.helper_file(3)),
            ],
            &[
                (l.helpers, &(l.file)(&(l.helper)("unrelated", ""))),
                (l.helpers2, &l.touched_helper_file(3)),
            ],
            "",
        );
        v.reports(l.name, &run, WEAKENED, l.helpers);
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

/// Control: a helper that moves to another changed file with its checks is not deleted.
#[test]
fn a_helper_moved_to_another_changed_file_reports_nothing() {
    let mut v = Verdicts::default();
    for l in langs() {
        let run = change(
            &[
                (l.helpers, &l.helper_file(2)),
                (l.helpers2, &(l.file)(&(l.helper)("other", &l.checks(1)))),
            ],
            &[
                (l.helpers, &(l.file)(&(l.helper)("unrelated", ""))),
                (
                    l.helpers2,
                    &(l.file)(&format!(
                        "{}{}",
                        (l.helper)("other", &l.checks(1)),
                        (l.helper)("check", &l.checks(2))
                    )),
                ),
            ],
            "",
        );
        v.clean(l.name, &run);
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

/// Two helpers of one name in one file, each of which asserts: a comment added to the
/// file reported the second as deleted. Python, PHP, Go and JS / TS record a method
/// under its type's name, so two types cannot collide there; C has no overloads.
const SAME_NAMED: &[(&str, &str, &str, &str)] = &[
    (
        "tests/common/mod.rs",
        "pub struct A;\npub struct B;\nimpl A {\n    pub fn check(&self, v: u32) {\n        assert_eq!(v, 1);\n    }\n}\nimpl B {\n    pub fn check(&self, v: u32) {\n        assert_eq!(v, 2);\n        assert_eq!(v % 2, 0);\n    }\n}\n",
        "        assert_eq!(v % 2, 0);\n",
        "// shared\n",
    ),
    (
        "src/test/java/Checks.java",
        "class Checks {\n    static void check(int v) {\n        assertEquals(1, v);\n    }\n    static void check(String v) {\n        assertEquals(\"a\", v);\n        assertEquals(1, v.length());\n    }\n}\n",
        "        assertEquals(1, v.length());\n",
        "// shared\n",
    ),
    (
        "tests/Checks.cs",
        "public class Checks\n{\n    public static void Check(int v)\n    {\n        Assert.Equal(1, v);\n    }\n    public static void Check(string v)\n    {\n        Assert.Equal(\"a\", v);\n        Assert.Equal(1, v.Length);\n    }\n}\n",
        "        Assert.Equal(1, v.Length);\n",
        "// shared\n",
    ),
    (
        "src/test/kotlin/Checks.kt",
        "fun check(v: Int) {\n    assertEquals(1, v)\n}\n\nfun check(v: String) {\n    assertEquals(\"a\", v)\n    assertEquals(1, v.length)\n}\n",
        "    assertEquals(1, v.length)\n",
        "// shared\n",
    ),
    (
        "tests/AppTests/Checks.swift",
        "import XCTest\n\nfunc check(_ v: Int) {\n    XCTAssertEqual(v, 1)\n}\n\nfunc check(_ v: String) {\n    XCTAssertEqual(v, \"a\")\n    XCTAssertEqual(v.count, 1)\n}\n",
        "    XCTAssertEqual(v.count, 1)\n",
        "// shared\n",
    ),
    (
        "src/test/scala/Checks.scala",
        "object Checks {\n  def check(v: Int): Unit = {\n    assert(v == 1)\n  }\n  def check(v: String): Unit = {\n    assert(v == \"a\")\n    assert(v.length == 1)\n  }\n}\n",
        "    assert(v.length == 1)\n",
        "// shared\n",
    ),
    (
        "tests/checks.cc",
        "#include <gtest/gtest.h>\n\nvoid Check(int v) {\n  EXPECT_EQ(v, 1);\n}\n\nvoid Check(const std::string& v) {\n  EXPECT_EQ(v, \"a\");\n  EXPECT_EQ(v.size(), 1);\n}\n",
        "  EXPECT_EQ(v.size(), 1);\n",
        "// shared\n",
    ),
    (
        "test/support/checks.rb",
        "module A\n  def check(v)\n    assert_equal 1, v\n  end\nend\n\nmodule B\n  def check(v)\n    assert_equal 2, v\n    assert_equal 0, v % 2\n  end\nend\n",
        "    assert_equal 0, v % 2\n",
        "# shared\n",
    ),
    (
        "tests/Checks.m",
        "#import <XCTest/XCTest.h>\n\n@implementation A\n\n- (void)check:(int)v {\n    XCTAssertEqual(v, 1);\n}\n\n@end\n\n@implementation B\n\n- (void)check:(int)v {\n    XCTAssertEqual(v, 2);\n    XCTAssertEqual(v % 2, 0);\n}\n\n@end\n",
        "    XCTAssertEqual(v % 2, 0);\n",
        "// shared\n",
    ),
];

#[test]
fn a_comment_beside_two_helpers_of_one_name_reports_nothing() {
    let mut v = Verdicts::default();
    for (path, file, _, comment) in SAME_NAMED {
        let run = change(&[(path, file)], &[(path, &format!("{comment}{file}"))], "");
        v.clean(path, &run);
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

/// Control: one of the two losing an assertion is reported.
#[test]
fn one_of_two_helpers_of_one_name_losing_a_check_is_reported() {
    let mut v = Verdicts::default();
    for (path, file, last_check, _) in SAME_NAMED {
        let run = change(
            &[(path, file)],
            &[(path, &file.replace(last_check, ""))],
            "",
        );
        v.reports(path, &run, WEAKENED, path);
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

/// Cargo's `examples/` and `benches/` are test paths for other gates, but no test calls
/// into them: a function there is not an assertion helper. `.unwrap()` becoming
/// `.unwrap_or_default()` in an example was reported as a weakened helper, and so was
/// any function that lost an assertion there, in every pack that reads the shared test
/// paths.
#[test]
fn a_function_under_examples_or_benches_is_not_an_assertion_helper() {
    let mut v = Verdicts::default();
    let main = |call: &str| {
        format!("fn main() {{\n    let v = load().{call}();\n    println!(\"{{}}\", v);\n}}\n")
    };
    for path in ["examples/show.rs", "benches/load.rs"] {
        let run = change(
            &[(path, &main("unwrap"))],
            &[(path, &main("unwrap_or_default"))],
            "",
        );
        v.clean(path, &run);
    }
    for l in langs() {
        for dir in ["examples/", "benches/"] {
            let path = l.example.replace("examples/", dir);
            let run = change(
                &[(&path, &l.helper_file(3))],
                &[(&path, &l.helper_file(0))],
                "",
            );
            v.clean(&path, &run);
        }
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

/// Control, and the part of that item left as it was: a function of a file under `tests/`
/// that holds no test is still read as a helper, so the same edit there is reported.
#[test]
fn a_function_in_a_test_support_file_that_drops_an_unwrap_is_reported() {
    let gen =
        |call: &str| format!("pub fn gen() -> u32 {{\n    let v = load().{call}();\n    v\n}}\n");
    let run = change(
        &[("tests/support/gen.rs", &gen("unwrap"))],
        &[("tests/support/gen.rs", &gen("unwrap_or_default"))],
        "",
    );
    assert_eq!(
        reported(&run),
        vec![(WEAKENED.to_string(), "tests/support/gen.rs".to_string())],
        "{}",
        run.stdout
    );
}

/// Assertions moved into a helper file that the same change adds were reported: only a
/// helper that existed on the base side was paired.
#[test]
fn assertions_moved_into_a_new_helper_file_report_nothing() {
    let mut v = Verdicts::default();
    for l in langs() {
        let run = change(
            &[(l.tests, &l.inline_test(3))],
            &[(l.helpers, &l.helper_file(3)), (l.tests, &l.calling_test())],
            "",
        );
        v.clean(l.name, &run);
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

/// Control: the new helper holds one of the three assertions.
#[test]
fn assertions_moved_into_a_new_helper_that_holds_fewer_are_reported() {
    let mut v = Verdicts::default();
    for l in langs() {
        let run = change(
            &[(l.tests, &l.inline_test(3))],
            &[(l.helpers, &l.helper_file(1)), (l.tests, &l.calling_test())],
            "",
        );
        v.reports(l.name, &run, DECREASED, l.tests);
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

/// Assertions moved into a helper of a changed file that also holds a test were reported:
/// that file's helpers were not paired.
#[test]
fn assertions_moved_into_a_helper_beside_a_test_report_nothing() {
    let mut v = Verdicts::default();
    for l in langs() {
        let helper = (l.helper)("check", &l.checks(3));
        let run = change(
            &[
                (l.helpers, &(l.file_with_test)(&helper)),
                (l.tests, &l.inline_test(3)),
            ],
            &[
                (
                    l.helpers,
                    &(l.file_with_test)(&format!("{}{helper}", l.comment)),
                ),
                (l.tests, &l.calling_test()),
            ],
            "",
        );
        v.clean(l.name, &run);
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

/// Control: that helper holds one of the three assertions.
#[test]
fn assertions_moved_into_a_helper_beside_a_test_that_holds_fewer_are_reported() {
    let mut v = Verdicts::default();
    for l in langs() {
        let helper = (l.helper)("check", &l.checks(1));
        let run = change(
            &[
                (l.helpers, &(l.file_with_test)(&helper)),
                (l.tests, &l.inline_test(3)),
            ],
            &[
                (
                    l.helpers,
                    &(l.file_with_test)(&format!("{}{helper}", l.comment)),
                ),
                (l.tests, &l.calling_test()),
            ],
            "",
        );
        v.reports(l.name, &run, DECREASED, l.tests);
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

/// A test that follows a renamed helper while dropping its own assertions: the helper is
/// the one the test already called, under a new name, so it stands for nothing new.
#[test]
fn a_renamed_helper_the_test_already_called_stands_for_no_dropped_assertion() {
    let mut v = Verdicts::default();
    for l in langs() {
        let test = |name: &str, own: usize| {
            (l.test)(&format!("{}{}", (l.call)(name), l.checks_from(5, own)))
        };
        let run = change(
            &[(l.helpers, &l.helper_file(3)), (l.tests, &test("check", 3))],
            &[
                (
                    l.helpers,
                    &(l.file)(&(l.helper)("check_response", &l.checks(3))),
                ),
                (l.tests, &test("check_response", 0)),
            ],
            "",
        );
        v.reports(l.name, &run, DECREASED, l.tests);
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}

/// Go: assertions that abort the test moved into a helper that aborts too were reported
/// as weakened to non-fatal.
#[test]
fn fatal_assertions_moved_into_a_helper_that_aborts_are_not_weakened() {
    let go = langs().into_iter().find(|l| l.name == "go").unwrap();
    let run = change(
        &[
            (go.helpers, &go.helper_file(3)),
            (go.tests, &go.inline_test(3)),
        ],
        &[
            (go.helpers, &go.touched_helper_file(3)),
            (go.tests, &go.calling_test()),
        ],
        "",
    );
    assert_eq!(reported(&run), Vec::new(), "{}", run.stdout);

    // Control: the helper reports with `t.Error`, which does not abort.
    let soft = go.helper_file(3).replace("t.Fatal(", "t.Error(");
    let run = change(
        &[(go.helpers, &soft), (go.tests, &go.inline_test(3))],
        &[
            (
                go.helpers,
                &soft.replace("func check", "// shared\nfunc check"),
            ),
            (go.tests, &go.calling_test()),
        ],
        "",
    );
    assert_eq!(
        run.titles("assertion-reduction"),
        vec!["Fatal Assertions Weakened To Non-Fatal".to_string()],
        "{}",
        run.stdout
    );
}

// ---- test paths --------------------------------------------------------------------------

/// A file whose name only ends with the letters of a test-utility name
/// (`latest-utils.ts`, `latestutil.go`) was read as test code, which exempted it from
/// `error-swallowing`.
#[test]
fn a_file_that_only_ends_like_a_test_utility_is_not_test_code() {
    let ts = |body: &str| format!("export function f() {{\n{body}  return 1;\n}}\n");
    let swallow = "  try {\n    g();\n  } catch (e) {}\n";
    let mut failures = Vec::new();
    for (path, reported) in [
        ("src/latest-utils.ts", true),
        ("src/latest_utils.js", true),
        ("src/test-utils.ts", false),
        ("src/render-test-utils.ts", false),
        ("src/db_test_utils.js", false),
    ] {
        let run = change(&[(path, &ts(""))], &[(path, &ts(swallow))], "");
        let got = !run.titles("error-swallowing").is_empty();
        if got != reported {
            failures.push(format!("{path}: reported {got}, expected {reported}"));
        }
    }
    let go = |body: &str| format!("package x\n\nfunc f() int {{\n{body}\treturn 1\n}}\n");
    for (path, reported) in [
        ("latestutil.go", true),
        ("testutil.go", false),
        ("db_testutil.go", false),
    ] {
        let run = change(&[(path, &go(""))], &[(path, &go("\t_, _ = h()\n"))], "");
        let got = !run.titles("error-swallowing").is_empty();
        if got != reported {
            failures.push(format!("{path}: reported {got}, expected {reported}"));
        }
    }
    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}

// ---- left as they are --------------------------------------------------------------------

/// Pin of a documented rule this change leaves alone: a test that drops assertions while
/// it starts calling a same-file helper that fails is read as a refactor whatever the
/// helper holds, with a note. Three assertions replaced by a call to a same-file helper
/// that holds one are not reported as a count drop.
#[test]
fn a_new_call_to_a_same_file_helper_that_fails_still_excuses_the_whole_drop() {
    let base = "def test_create():\n    r = create()\n    assert r.a == 1\n    assert r.b == 2\n    assert r.c == 3\n";
    let head = "def check(r):\n    assert r.a == 1\n\ndef test_create():\n    r = create()\n    check(r)\n";
    let run = change(
        &[("tests/test_api.py", base)],
        &[("tests/test_api.py", head)],
        "",
    );
    assert_eq!(reported(&run), Vec::new(), "{}", run.stdout);
    let notes = run.outcome("assertion-reduction")["notes"].to_string();
    assert!(
        notes.contains("read as moved into same-file helpers that fail"),
        "{notes}"
    );
}

/// Pin: a helper called through a type or an object (`Checks.check(r)`, `s.check(t, r)`,
/// `checker.check(r)`) is not resolved in the packs that read only bare and `this` calls,
/// so a lossless move behind such a call stays reported there. The packs that match a
/// method call by its name read the move.
#[test]
fn a_move_behind_a_qualified_call_is_reported_where_the_pack_does_not_resolve_it() {
    let mut v = Verdicts::default();
    for l in langs() {
        let (qualified, helper): (String, String) = match l.name {
            "go" => (
                "\tSuite{}.check(t, r)\n".into(),
                (l.file)(&(l.method.unwrap())("check", &l.checks(3))),
            ),
            "typescript" => (
                "  checker.check(r);\n".into(),
                (l.file)(&(l.method.unwrap())("check", &l.checks(3))),
            ),
            "java" => ("        Checks.check(r);\n".into(), l.helper_file(3)),
            "csharp" => ("        Checks.check(r);\n".into(), l.helper_file(3)),
            "kotlin" => (
                "    Checks.check(r)\n".into(),
                (l.file)(&(l.method.unwrap())("check", &l.checks(3))),
            ),
            "scala" => ("    Checks.check(r)\n".into(), l.helper_file(3)),
            "ruby" => ("    Checks.check(r)\n".into(), l.helper_file(3)),
            _ => continue,
        };
        let run = change(
            &[(l.helpers, &helper), (l.tests, &l.inline_test(3))],
            &[
                (l.helpers, &format!("{helper}{}", l.comment.trim_start())),
                (l.tests, &(l.test)(&qualified)),
            ],
            "",
        );
        v.reports(l.name, &run, DECREASED, l.tests);
    }
    assert!(v.0.is_empty(), "\n{}\n", v.0.join("\n"));
}
