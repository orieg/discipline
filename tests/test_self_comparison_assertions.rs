//! Equality assertions that compare an expression with itself (#481, D3), through the
//! real binary: `vacuous-tests/self-comparison-assertion-added` in a new test and
//! `assertion-reduction/self-comparison-assertion-introduced` in a changed one.
//!
//! The detector reads the exact form only: two operands that are the same tokens. Each
//! test here holds the exact form beside the near-misses that must stay silent, and
//! [`NEGATIVE_CONTROLS`] is the enumerated list of forms that are never reported.

mod common;
use common::{Repo, Run, CONFIG_HEAD};

const ADDED: &str = "vacuous-tests/self-comparison-assertion-added";
const INTRODUCED: &str = "assertion-reduction/self-comparison-assertion-introduced";
const VACUOUS: &str = "vacuous-tests/vacuous-test-added";
const REDUCED: &str = "assertion-reduction/assertions-reduced";
/// The scope limit every message of the two codes states.
const SCOPE: &str = "Exact form only";
const NO_VALUE_FLOW: &str = "no alias or value-flow analysis";

/// How one pack spells a test file, its equality assertions with the same tokens on both
/// sides, and the near-misses beside them. No operand of an exact form is also an
/// operand of another assertion, so no form is left out as a reflexivity check.
struct Lang {
    name: &'static str,
    path: &'static str,
    /// One file holding one test named `create` (in the pack's spelling) whose
    /// statements are the lines given.
    file: fn(&str) -> String,
    indent: &'static str,
    /// Two assertions that can fail.
    real: [&'static str; 2],
    exact: &'static [&'static str],
    near: &'static [&'static str],
}

impl Lang {
    fn source(&self, statements: &[&str]) -> String {
        let body: String = statements
            .iter()
            .map(|s| format!("{}{s}\n", self.indent))
            .collect();
        (self.file)(&body)
    }

    /// The 1-based lines of `statements` in `source`.
    fn lines(&self, source: &str, statements: &[&str]) -> Vec<u64> {
        statements
            .iter()
            .map(|s| {
                let line = format!("{}{s}", self.indent);
                source
                    .lines()
                    .position(|l| l == line)
                    .unwrap_or_else(|| panic!("{}: no line `{line}`", self.name))
                    as u64
                    + 1
            })
            .collect()
    }
}

fn langs() -> Vec<Lang> {
    vec![
        Lang {
            name: "rust",
            path: "tests/api.rs",
            file: |body| format!("#[test]\nfn create() {{\n    let r = make();\n{body}}}\n"),
            indent: "    ",
            real: ["assert_eq!(r.f8, 8);", "assert_eq!(r.f9, 9);"],
            exact: &[
                "assert_eq!(y, y);",
                "assert_eq!(r.a, r.a);",
                "assert_eq!( r.b , (r.b) );",
                "assert_eq!(r.c /* one */, r.c, \"why\");",
                "debug_assert_eq!(v[0], v[0]);",
            ],
            near: &[
                "assert_eq!(x.clone(), x);",
                "assert_eq!(r.d, r.e);",
                "assert_ne!(z, z);",
                "assert_eq!(it.next(), it.next());",
            ],
        },
        Lang {
            name: "python",
            path: "tests/test_api.py",
            file: |body| format!("def test_create():\n    r = make()\n{body}"),
            indent: "    ",
            real: ["assert r.f8 == 8", "assert r.f9 == 9"],
            exact: &["assert y == y", "assert (r.a == r.a)", "assert r.b is r.b"],
            near: &[
                "assert r.d == r.e",
                "assert z != z",
                "assert next(it) == next(it)",
            ],
        },
        Lang {
            name: "python-unittest",
            path: "tests/test_case.py",
            file: |body| {
                format!("import unittest\n\n\nclass ApiTest(unittest.TestCase):\n    def test_create(self):\n        r = make()\n{body}")
            },
            indent: "        ",
            real: ["self.assertEqual(r.f8, 8)", "self.assertEqual(r.f9, 9)"],
            exact: &["self.assertEqual(r.a, r.a)", "self.assertIs(y, y)"],
            near: &[
                "self.assertEqual(r.d, r.e)",
                "self.assertNotEqual(z, z)",
                "self.assertEqual(next(it), next(it))",
            ],
        },
        Lang {
            name: "typescript",
            path: "tests/api.test.ts",
            file: |body| {
                format!("import {{ test, expect }} from 'vitest';\n\ntest('create', () => {{\n  const r = make();\n{body}}});\n")
            },
            indent: "  ",
            real: ["expect(r.f8).toBe(8);", "expect(r.f9).toBe(9);"],
            exact: &[
                "expect(y).toBe(y);",
                "expect(r.a).toEqual(r.a);",
                "expect(r.b).toStrictEqual((r.b));",
                "assert.strictEqual(r.c, r.c);",
            ],
            near: &[
                "expect(r.d).toBe(r.e);",
                "expect(z).not.toBe(z);",
                "expect(next()).toBe(next());",
            ],
        },
        Lang {
            name: "go",
            path: "api_test.go",
            file: |body| {
                format!("package x\n\nimport (\n\t\"testing\"\n\n\t\"github.com/stretchr/testify/assert\"\n\t\"github.com/stretchr/testify/require\"\n)\n\nfunc TestCreate(t *testing.T) {{\n\tr := make()\n{body}}}\n")
            },
            indent: "\t",
            real: ["assert.Equal(t, 8, r.F8)", "assert.Equal(t, 9, r.F9)"],
            exact: &["assert.Equal(t, y, y)", "require.Equal(t, r.A, r.A)"],
            near: &[
                "assert.Equal(t, r.D, r.E)",
                "assert.NotEqual(t, z, z)",
                "assert.Equal(t, next(), next())",
            ],
        },
        Lang {
            name: "java",
            path: "src/test/java/ApiTest.java",
            file: |body| {
                format!("import org.junit.jupiter.api.Test;\nimport static org.junit.jupiter.api.Assertions.*;\n\nclass ApiTest {{\n    @Test\n    void create() {{\n        R r = make();\n{body}    }}\n}}\n")
            },
            indent: "        ",
            real: ["assertEquals(8, r.f8);", "assertEquals(9, r.f9);"],
            exact: &[
                "assertEquals(y, y);",
                "assertSame(r.a, r.a);",
                "assertThat(r.b).isEqualTo(r.b);",
            ],
            near: &[
                "assertEquals(r.d, r.e);",
                "assertNotEquals(z, z);",
                "assertEquals(it.next(), it.next());",
            ],
        },
        Lang {
            name: "kotlin",
            path: "src/test/kotlin/ApiTest.kt",
            file: |body| {
                format!("import kotlin.test.Test\nimport kotlin.test.assertEquals\n\nclass ApiTest {{\n    @Test\n    fun create() {{\n        val r = make()\n{body}    }}\n}}\n")
            },
            indent: "        ",
            real: ["assertEquals(8, r.f8)", "assertEquals(9, r.f9)"],
            exact: &["assertEquals(y, y)", "assertSame(r.a, r.a)"],
            near: &[
                "assertEquals(r.d, r.e)",
                "assertNotEquals(z, z)",
                "assertEquals(it.next(), it.next())",
            ],
        },
        Lang {
            name: "csharp",
            path: "tests/ApiTests.cs",
            file: |body| {
                format!("using Xunit;\n\npublic class ApiTests\n{{\n    [Fact]\n    public void Create()\n    {{\n        var r = Make();\n{body}    }}\n}}\n")
            },
            indent: "        ",
            real: ["Assert.Equal(8, r.F8);", "Assert.Equal(9, r.F9);"],
            exact: &[
                "Assert.Equal(y, y);",
                "Assert.Same(r.A, r.A);",
                "Assert.AreEqual(r.B, r.B);",
                "Assert.That(r.C, Is.EqualTo(r.C));",
            ],
            near: &[
                "Assert.Equal(r.D, r.E);",
                "Assert.NotEqual(z, z);",
                "Assert.Equal(Next(), Next());",
            ],
        },
        Lang {
            name: "cpp",
            path: "tests/api_test.cc",
            file: |body| {
                format!(
                    "#include <gtest/gtest.h>\n\nTEST(Api, Create) {{\n  R r = Make();\n{body}}}\n"
                )
            },
            indent: "  ",
            real: ["EXPECT_EQ(r.f8, 8);", "EXPECT_EQ(r.f9, 9);"],
            exact: &["EXPECT_EQ(y, y);", "ASSERT_EQ(r.a, r.a);"],
            near: &[
                "EXPECT_EQ(r.d, r.e);",
                "EXPECT_EQ(Next(), Next());",
                "EXPECT_DOUBLE_EQ(z, z);",
            ],
        },
        Lang {
            name: "ruby",
            path: "test/api_test.rb",
            file: |body| {
                format!("class ApiTest < Minitest::Test\n  def test_create\n    r = make\n    s = other\n{body}  end\nend\n")
            },
            indent: "    ",
            real: ["assert_equal 8, r.f8", "assert_equal 9, r.f9"],
            exact: &["assert_equal r, r", "assert_same @x, @x"],
            near: &[
                "assert_equal s, t",
                "refute_equal z, z",
                "assert_equal e.next, e.next",
            ],
        },
        Lang {
            name: "php",
            path: "tests/ApiTest.php",
            file: |body| {
                format!("<?php\n\nuse PHPUnit\\Framework\\TestCase;\n\nclass ApiTest extends TestCase\n{{\n    public function testCreate(): void\n    {{\n        $r = make();\n{body}    }}\n}}\n")
            },
            indent: "        ",
            real: [
                "$this->assertSame(8, $r->f8);",
                "$this->assertSame(9, $r->f9);",
            ],
            exact: &[
                "$this->assertSame($y, $y);",
                "$this->assertEquals($r->a, $r->a);",
            ],
            near: &[
                "$this->assertSame($r->d, $r->e);",
                "$this->assertNotSame($z, $z);",
                "$this->assertSame(next($it), next($it));",
            ],
        },
        Lang {
            name: "swift",
            path: "tests/AppTests/ApiTests.swift",
            file: |body| {
                format!("import XCTest\n\nfinal class ApiTests: XCTestCase {{\n    func testCreate() {{\n        let r = make()\n{body}    }}\n}}\n")
            },
            indent: "        ",
            real: ["XCTAssertEqual(r.f8, 8)", "XCTAssertEqual(r.f9, 9)"],
            exact: &["XCTAssertEqual(y, y)", "XCTAssertEqual(r.a, r.a)"],
            near: &[
                "XCTAssertEqual(r.d, r.e)",
                "XCTAssertNotEqual(z, z)",
                "XCTAssertEqual(next(), next())",
            ],
        },
        Lang {
            name: "scala",
            path: "src/test/scala/ApiSpec.scala",
            file: |body| {
                format!("class ApiSpec extends AnyFunSuite {{\n  test(\"create\") {{\n    val r = make()\n{body}  }}\n}}\n")
            },
            indent: "    ",
            real: ["assert(r.f8 == 8)", "assert(r.f9 == 9)"],
            exact: &[
                "assert(y == y)",
                "assertEquals(r.a, r.a)",
                "r.b shouldBe r.b",
            ],
            near: &[
                "assert(r.d == r.e)",
                "assert(z != z)",
                "assert(it.next() == it.next())",
            ],
        },
        Lang {
            name: "objc",
            path: "tests/ApiTests.m",
            file: |body| {
                format!("#import <XCTest/XCTest.h>\n\n@interface ApiTests : XCTestCase\n@end\n\n@implementation ApiTests\n\n- (void)testCreate {{\n    R *r = make();\n{body}}}\n\n@end\n")
            },
            indent: "    ",
            real: ["XCTAssertEqual(r.f8, 8);", "XCTAssertEqual(r.f9, 9);"],
            exact: &["XCTAssertEqual(y, y);", "XCTAssertEqualObjects(r.a, r.a);"],
            near: &[
                "XCTAssertEqual(r.d, r.e);",
                "XCTAssertNotEqual(z, z);",
                "XCTAssertEqual([it next], [it next]);",
            ],
        },
    ]
}

fn repo() -> Repo {
    let repo = Repo::new();
    repo.write("discipline.toml", CONFIG_HEAD);
    repo.commit("chore: configuration");
    repo.git(&["branch", "-f", "main"]);
    repo
}

/// `(code, severity, line, message)` of what `gate` reported, in report order.
fn reported(run: &Run, gate: &str) -> Vec<(String, String, u64, String)> {
    run.violations(gate)
        .iter()
        .map(|v| {
            (
                v["code"].as_str().unwrap().to_string(),
                v["severity"].as_str().unwrap().to_string(),
                v["line"].as_u64().unwrap_or(0),
                v["message"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn codes(run: &Run, gate: &str) -> Vec<(String, String, u64)> {
    reported(run, gate)
        .into_iter()
        .map(|(code, severity, line, _)| (code, severity, line))
        .collect()
}

fn joined(lines: &[u64]) -> String {
    lines
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// A new test that holds an assertion that can fail, every exact form of its pack and
/// every near-miss: one warning that names the lines of the exact forms and no other.
#[test]
fn a_new_test_is_reported_for_its_exact_forms_and_not_for_a_near_miss() {
    let mut wrong = Vec::new();
    for lang in langs() {
        let mut statements = vec![lang.real[0]];
        statements.extend(lang.exact);
        statements.extend(lang.near);
        let source = lang.source(&statements);
        let lines = lang.lines(&source, lang.exact);

        let r = repo();
        r.write(lang.path, &source);
        r.commit("test: add a test");
        let run = r.check(&[]);
        let got = reported(&run, "vacuous-tests");
        let ok = run.code == 0
            && got.len() == 1
            && got[0].0 == ADDED
            && got[0].1 == "warning"
            && got[0].2 == lines[0]
            && got[0].3.contains(&format!("(line {})", joined(&lines)))
            && got[0].3.contains(SCOPE)
            && got[0].3.contains(NO_VALUE_FLOW);
        if !ok {
            wrong.push(format!(
                "{}: exit {}, exact forms on {lines:?}, reported {got:?}",
                lang.name, run.code
            ));
        }
        let notes = run.outcome("vacuous-tests")["notes"].to_string();
        if !notes.contains("no alias/value-flow analysis") {
            wrong.push(format!("{}: no scope note in {notes}", lang.name));
        }

        // Control: the same test without the exact forms reports nothing, and carries
        // no note about a finding it did not make.
        let mut statements = vec![lang.real[0]];
        statements.extend(lang.near);
        let r = repo();
        r.write(lang.path, &lang.source(&statements));
        r.commit("test: add a test");
        let run = r.check(&[]);
        let got = reported(&run, "vacuous-tests");
        let notes = run.outcome("vacuous-tests")["notes"].to_string();
        if run.code != 0 || !got.is_empty() || notes.contains("value-flow") {
            wrong.push(format!(
                "{}: near-misses alone: exit {}, reported {got:?}, notes {notes}",
                lang.name, run.code
            ));
        }
    }
    assert_eq!(wrong, Vec::<String>::new());
}

/// A new test whose only assertions compare an expression with itself cannot fail: it
/// is the vacuous test, reported once at the gate's severity, and its message names the
/// lines. The warning is not added to it.
#[test]
fn a_new_test_with_nothing_but_self_comparisons_is_one_vacuous_test_finding() {
    let mut wrong = Vec::new();
    for lang in langs() {
        // The Pest matcher of the PHP pack is recorded and still counted; every form
        // used here is one its pack counts as a tautology.
        let source = lang.source(lang.exact);
        let lines = lang.lines(&source, lang.exact);
        let r = repo();
        r.write(lang.path, &source);
        r.commit("test: add a test");
        let run = r.check(&[]);
        let got = reported(&run, "vacuous-tests");
        let ok = run.code == 1
            && got.len() == 1
            && got[0].0 == VACUOUS
            && got[0].1 == "error"
            && got[0].3.contains(&format!(
                "{} compare(s) an expression with itself (line {})",
                lines.len(),
                joined(&lines)
            ));
        if !ok {
            wrong.push(format!(
                "{}: exit {}, reported {got:?}",
                lang.name, run.code
            ));
        }
    }
    assert_eq!(wrong, Vec::<String>::new());
}

/// An existing test: a self-comparison the change introduces is reported at its line
/// while the count holds, and in the reduction's message when the count drops; a
/// self-comparison the base side already held is not reported.
#[test]
fn a_changed_test_is_reported_for_the_self_comparisons_the_change_introduces() {
    let mut wrong = Vec::new();
    for lang in langs() {
        let base = lang.source(&lang.real);

        // Added beside the assertions that were there, with every near-miss: one
        // warning an exact form, at its line, and nothing else from the gate.
        let mut statements = lang.real.to_vec();
        statements.extend(lang.exact);
        statements.extend(lang.near);
        let head = lang.source(&statements);
        let lines = lang.lines(&head, lang.exact);
        let r = repo();
        r.commit_base(lang.path, &base, "test: base");
        r.write(lang.path, &head);
        r.commit("test: change a test");
        let run = r.check(&[]);
        let got = reported(&run, "assertion-reduction");
        let expected: Vec<(String, String, u64)> = lines
            .iter()
            .map(|l| (INTRODUCED.to_string(), "warning".to_string(), *l))
            .collect();
        let scoped = got
            .iter()
            .all(|g| g.3.contains(SCOPE) && g.3.contains(NO_VALUE_FLOW));
        if run.code != 0 || codes(&run, "assertion-reduction") != expected || !scoped {
            wrong.push(format!(
                "{} (added): exit {}, exact forms on {lines:?}, reported {got:?}",
                lang.name, run.code
            ));
        }

        // An assertion that compared two operands now compares one with itself, and the
        // count drops: one finding, the reduction at the gate's severity, whose message
        // names the line. The warning is not added to it.
        let head = lang.source(&[lang.real[0], lang.exact[0]]);
        let line = lang.lines(&head, &[lang.exact[0]])[0];
        let r = repo();
        r.commit_base(lang.path, &base, "test: base");
        r.write(lang.path, &head);
        r.commit("test: change a test");
        let run = r.check(&[]);
        let got = reported(&run, "assertion-reduction");
        let ok = run.code == 1
            && got.len() == 1
            && got[0].0 == REDUCED
            && got[0].1 == "error"
            && got[0].3.contains(&format!(
                "1 equality assertion(s) now compare an expression with itself (line {line})"
            ))
            && got[0].3.contains(SCOPE)
            && got[0].3.contains(NO_VALUE_FLOW);
        if !ok {
            wrong.push(format!(
                "{} (replaced): exit {}, line {line}, reported {got:?}",
                lang.name, run.code
            ));
        }

        // Swapped in while another assertion is added: the count holds, so the warning
        // is the finding.
        let head = lang.source(&[lang.real[0], lang.exact[0], lang.near[0]]);
        let line = lang.lines(&head, &[lang.exact[0]])[0];
        let r = repo();
        r.commit_base(lang.path, &base, "test: base");
        r.write(lang.path, &head);
        r.commit("test: change a test");
        let run = r.check(&[]);
        let got = codes(&run, "assertion-reduction");
        if run.code != 0 || got != [(INTRODUCED.to_string(), "warning".to_string(), line)] {
            wrong.push(format!(
                "{} (swapped): exit {}, line {line}, reported {got:?}",
                lang.name, run.code
            ));
        }

        // Control: the base side already holds the self-comparison and the change adds
        // an assertion above it. Its line moves; it is not introduced.
        let base = lang.source(&[lang.real[0], lang.exact[0]]);
        let head = lang.source(&[lang.real[0], lang.real[1], lang.exact[0]]);
        let r = repo();
        r.commit_base(lang.path, &base, "test: base");
        r.write(lang.path, &head);
        r.commit("test: change a test");
        let run = r.check(&[]);
        let got = reported(&run, "assertion-reduction");
        if run.code != 0 || !got.is_empty() {
            wrong.push(format!(
                "{} (already there): exit {}, reported {got:?}",
                lang.name, run.code
            ));
        }
    }
    assert_eq!(wrong, Vec::<String>::new());
}

/// The severity matrix of the two codes: the exact form against its near-misses, in a
/// new test and in a changed one. `None` is silence from the gate.
#[test]
fn the_exact_form_is_a_warning_and_a_near_miss_is_silent() {
    const REAL: &str = "    assert_eq!(r.f9, 9);\n";
    let file = |body: &str| format!("#[test]\nfn create() {{\n    let r = make();\n{body}}}\n");
    // (what, the assertion, its verdict beside an assertion that can fail)
    let matrix: &[(&str, &str, Option<&str>)] = &[
        ("exact", "assert_eq!(y, y);", Some("warning")),
        ("exact, a field", "assert_eq!(r.a, r.a);", Some("warning")),
        ("exact, spaced", "assert_eq!(r . a,r.a);", Some("warning")),
        ("exact, wrapped", "assert_eq!((y), ((y)));", Some("warning")),
        (
            "exact, a comment",
            "assert_eq!(y, /* same */ y);",
            Some("warning"),
        ),
        ("exact, a literal", "assert_eq!(1, 1);", Some("warning")),
        ("a clone", "assert_eq!(y.clone(), y);", None),
        ("a reference", "assert_eq!(&y, y);", None),
        ("another field", "assert_eq!(r.a, r.b);", None),
        ("another index", "assert_eq!(v[0], v[1]);", None),
        (
            "parentheses that change the grouping",
            "assert_eq!((a + b) * c, a + b * c);",
            None,
        ),
        ("an alias: no value flow", "assert_eq!(y, same_y);", None),
        ("an inequality", "assert_ne!(y, y);", None),
        (
            "the same call twice",
            "assert_eq!(it.next(), it.next());",
            None,
        ),
        ("a constructor call", "assert_eq!(Some(y), Some(y));", None),
        ("a boolean assertion", "assert!(y == y);", None),
        ("in a string", "let _ = \"assert_eq!(y, y)\";", None),
        ("in a comment", "// assert_eq!(y, y);", None),
    ];
    let mut wrong = Vec::new();
    for (what, assertion, verdict) in matrix {
        let line = format!("    {assertion}\n");

        // A new test.
        let r = repo();
        r.write("tests/api.rs", &file(&format!("{REAL}{line}")));
        r.commit("test: add a test");
        let run = r.check(&[]);
        let got = codes(&run, "vacuous-tests");
        let expected: Vec<(String, String, u64)> = verdict
            .iter()
            .map(|s| (ADDED.to_string(), s.to_string(), 5))
            .collect();
        if run.code != 0 || got != expected {
            wrong.push(format!("new, {what}: exit {}, {got:?}", run.code));
        }

        // A changed test.
        let r = repo();
        r.commit_base("tests/api.rs", &file(REAL), "test: base");
        r.write("tests/api.rs", &file(&format!("{REAL}{line}")));
        r.commit("test: change a test");
        let run = r.check(&[]);
        let got = codes(&run, "assertion-reduction");
        let expected: Vec<(String, String, u64)> = verdict
            .iter()
            .map(|s| (INTRODUCED.to_string(), s.to_string(), 5))
            .collect();
        if run.code != 0 || got != expected {
            wrong.push(format!("changed, {what}: exit {}, {got:?}", run.code));
        }
    }
    assert_eq!(wrong, Vec::<String>::new());

    // The severity is the code's own: a gate set to `warning` or `error` reports the
    // exact form as a warning either way.
    for setting in ["error", "warning"] {
        let r = repo();
        r.write(
            "discipline.toml",
            &format!("{CONFIG_HEAD}\n[gates.vacuous-tests]\nseverity = \"{setting}\"\n"),
        );
        r.commit("chore: severity");
        r.git(&["branch", "-f", "main"]);
        r.write(
            "tests/api.rs",
            &file(&format!("{REAL}    assert_eq!(y, y);\n")),
        );
        r.commit("test: add a test");
        let run = r.check(&[]);
        let got = codes(&run, "vacuous-tests");
        assert_eq!(run.code, 0, "{setting}: {got:?}");
        assert_eq!(got.len(), 1, "{setting}: {got:?}");
        assert_eq!(got[0].1, "warning", "{setting}");
    }
}

/// One form that is never reported, as a whole file.
struct Control {
    name: &'static str,
    path: &'static str,
    /// The file before the change, for the changed-test side; empty when the control is
    /// read as a new test only.
    base: &'static str,
    head: &'static str,
}

/// The enumerated negative controls of the detector (docs/GATES.md, "Self-comparison
/// assertions"). Each is silent as a new test, and as a changed one where it has a base.
const NEGATIVE_CONTROLS: &[Control] = &[
    Control {
        name: "a reflexivity check beside a comparison of the same operand (Rust)",
        path: "tests/eq.rs",
        base: "#[test]\nfn test_eq_reflexive() {\n    let (a, b) = (P::new(1), P::new(1));\n    assert_eq!(a, b);\n    assert_eq!(b, a);\n}\n",
        head: "#[test]\nfn test_eq_reflexive() {\n    let (a, b) = (P::new(1), P::new(1));\n    assert_eq!(a, b);\n    assert_eq!(b, a);\n    assert_eq!(a, a);\n    assert_eq!(b, b);\n}\n",
    },
    Control {
        name: "a reflexivity check beside a comparison of the same operand (Python)",
        path: "tests/test_eq.py",
        base: "def test_eq_reflexive():\n    a, b = P(1), P(1)\n    assert a == b\n",
        head: "def test_eq_reflexive():\n    a, b = P(1), P(1)\n    assert a == b\n    assert a == a\n",
    },
    Control {
        name: "a clone compared with its source",
        path: "tests/clone.rs",
        base: "#[test]\nfn clones() {\n    let x = make();\n    assert_eq!(x.len(), 2);\n}\n",
        head: "#[test]\nfn clones() {\n    let x = make();\n    assert_eq!(x.len(), 2);\n    assert_eq!(x.clone(), x);\n}\n",
    },
    Control {
        name: "an assertion in a macro_rules! body, with a metavariable",
        path: "tests/generated.rs",
        base: "#[test]\nfn generated() {\n    let v = make();\n    assert_eq!(v.len(), 2);\n}\n",
        head: "macro_rules! same {\n    ($a:expr) => {\n        assert_eq!($a, $a);\n    };\n}\n\n#[test]\nfn generated() {\n    let v = make();\n    assert_eq!(v.len(), 2);\n    same!(v);\n}\n",
    },
    Control {
        name: "an assertion in a macro_rules! body inside the test, without a metavariable",
        path: "tests/local_macro.rs",
        base: "#[test]\nfn local() {\n    let y = make();\n    assert_eq!(y.len(), 2);\n}\n",
        head: "#[test]\nfn local() {\n    let y = make();\n    macro_rules! same {\n        () => {\n            assert_eq!(y, y);\n        };\n    }\n    assert_eq!(y.len(), 2);\n    same!();\n}\n",
    },
    Control {
        name: "an assertion in a #define body (C++)",
        path: "tests/define_test.cc",
        base: "#include <gtest/gtest.h>\n\nTEST(Api, Define) {\n  V v = Make();\n  EXPECT_EQ(v.size(), 2);\n}\n",
        head: "#include <gtest/gtest.h>\n\n#define CHECK_SAME(a) EXPECT_EQ(a, a)\n\nTEST(Api, Define) {\n  V v = Make();\n#define CHECK_LOCAL() EXPECT_EQ(v, v)\n  EXPECT_EQ(v.size(), 2);\n  CHECK_SAME(v);\n  CHECK_LOCAL();\n}\n",
    },
    Control {
        name: "the same call on both sides (Java iterator)",
        path: "src/test/java/IterTest.java",
        base: "import org.junit.jupiter.api.Test;\n\nclass IterTest {\n    @Test\n    void advances() {\n        assertEquals(1, it.size());\n    }\n}\n",
        head: "import org.junit.jupiter.api.Test;\n\nclass IterTest {\n    @Test\n    void advances() {\n        assertEquals(1, it.size());\n        assertEquals(it.next(), it.next());\n    }\n}\n",
    },
    Control {
        name: "the same call on both sides (Python generator, Rust random source)",
        path: "tests/test_calls.py",
        base: "def test_calls():\n    assert size(it) == 1\n",
        head: "def test_calls():\n    assert size(it) == 1\n    assert next(it) == next(it)\n    assert random.random() == random.random()\n",
    },
    Control {
        name: "the same generic call on both sides (Rust)",
        path: "tests/random.rs",
        base: "#[test]\nfn random() {\n    assert_eq!(rng.len(), 1);\n}\n",
        head: "#[test]\nfn random() {\n    assert_eq!(rng.len(), 1);\n    assert_eq!(rng.gen::<u8>(), rng.gen::<u8>());\n    assert_eq!(vec![x], vec![x]);\n}\n",
    },
    Control {
        name: "a NaN check written as an inequality",
        path: "tests/test_nan.py",
        base: "def test_nan():\n    assert kind(x) == 1\n",
        head: "def test_nan():\n    assert kind(x) == 1\n    assert x != x\n",
    },
    Control {
        name: "a not-equal assertion on one operand",
        path: "src/test/java/NanTest.java",
        base: "import org.junit.jupiter.api.Test;\n\nclass NanTest {\n    @Test\n    void nan() {\n        assertEquals(1, kind(x));\n    }\n}\n",
        head: "import org.junit.jupiter.api.Test;\n\nclass NanTest {\n    @Test\n    void nan() {\n        assertEquals(1, kind(x));\n        assertNotEquals(x, x);\n    }\n}\n",
    },
    Control {
        name: "a negated matcher (JavaScript)",
        path: "tests/nan.test.js",
        base: "test('nan', () => {\n  expect(kind(x)).toBe(1);\n});\n",
        head: "test('nan', () => {\n  expect(kind(x)).toBe(1);\n  expect(x).not.toBe(x);\n});\n",
    },
    Control {
        name: "two names bound to one value: no value flow is followed",
        path: "tests/alias.rs",
        base: "#[test]\nfn alias() {\n    let a = make();\n    assert_eq!(a.len(), 2);\n}\n",
        head: "#[test]\nfn alias() {\n    let a = make();\n    let b = &a;\n    assert_eq!(a.len(), 2);\n    assert_eq!(&a, b);\n}\n",
    },
    Control {
        name: "the form written in a comment and in a string",
        path: "tests/text.rs",
        base: "#[test]\nfn text() {\n    assert_eq!(make(), 2);\n}\n",
        head: "#[test]\nfn text() {\n    // assert_eq!(y, y);\n    let s = \"assert_eq!(y, y)\";\n    assert_eq!(make(), 2);\n    assert_eq!(s.len(), 16);\n}\n",
    },
];

#[test]
fn no_negative_control_is_reported() {
    let ours = |run: &Run| -> Vec<(String, String, u64)> {
        let mut all = codes(run, "vacuous-tests");
        all.extend(codes(run, "assertion-reduction"));
        all.retain(|c| c.0 == ADDED || c.0 == INTRODUCED);
        all
    };
    let mut wrong = Vec::new();
    let mut names = std::collections::HashSet::new();
    for control in NEGATIVE_CONTROLS {
        assert!(names.insert(control.name), "`{}` twice", control.name);
        // As a new test.
        let r = repo();
        r.write(control.path, control.head);
        r.commit("test: add a test");
        let run = r.check(&[]);
        if !ours(&run).is_empty() || run.code != 0 {
            wrong.push(format!(
                "{} (new): exit {}, {:?}",
                control.name,
                run.code,
                reported(&run, "vacuous-tests")
            ));
        }
        // As a changed test.
        if !control.base.is_empty() {
            let r = repo();
            r.commit_base(control.path, control.base, "test: base");
            r.write(control.path, control.head);
            r.commit("test: change a test");
            let run = r.check(&[]);
            if !ours(&run).is_empty() || run.code != 0 {
                wrong.push(format!(
                    "{} (changed): exit {}, {:?}",
                    control.name,
                    run.code,
                    reported(&run, "assertion-reduction")
                ));
            }
        }
    }
    assert_eq!(wrong, Vec::<String>::new());

    // The reflexivity controls are silent for the comparison beside them, not for the
    // test's name: the same test without that comparison is reported.
    let r = repo();
    r.write(
        "tests/eq.rs",
        "#[test]\nfn test_eq_reflexive() {\n    let a = P::new(1);\n    assert_eq!(a.len(), 1);\n    assert_eq!(a, a);\n}\n",
    );
    r.commit("test: add a test");
    let run = r.check(&[]);
    assert_eq!(
        codes(&run, "vacuous-tests"),
        vec![(ADDED.to_string(), "warning".to_string(), 5)]
    );
}

/// The two gates' own directives lift the findings; no directive was added for them.
#[test]
fn the_gates_directives_lift_the_findings() {
    const BASE: &str = "#[test]\nfn create() {\n    let r = make();\n    assert_eq!(r.f9, 9);\n}\n";
    const HEAD: &str =
        "#[test]\nfn create() {\n    let r = make();\n    assert_eq!(r.f9, 9);\n    assert_eq!(r, r);\n}\n";

    // A new test: `allow-vacuous-test`.
    let r = repo();
    r.write("tests/api.rs", HEAD);
    r.commit("test: add a test");
    let run = r.check_with_pr(
        &[],
        "allow-vacuous-test: create the reflexivity of the comparison is what this test is for\n",
    );
    assert!(codes(&run, "vacuous-tests").is_empty(), "{}", run.stdout);
    let lifted = run.outcome("vacuous-tests")["overrides"].to_string();
    assert!(lifted.contains("allow-vacuous-test"), "{lifted}");
    // Control: a directive naming another test lifts nothing.
    let run = r.check_with_pr(
        &[],
        "allow-vacuous-test: other the reflexivity of the comparison is what this test is for\n",
    );
    assert_eq!(codes(&run, "vacuous-tests").len(), 1, "{}", run.stdout);

    // A changed test: `allow-assertion-drop`.
    let r = repo();
    r.commit_base("tests/api.rs", BASE, "test: base");
    r.write("tests/api.rs", HEAD);
    r.commit("test: change a test");
    let run = r.check_with_pr(
        &[],
        "allow-assertion-drop: create the reflexivity of the comparison is what this test is for\n",
    );
    assert!(
        codes(&run, "assertion-reduction").is_empty(),
        "{}",
        run.stdout
    );
    let lifted = run.outcome("assertion-reduction")["overrides"].to_string();
    assert!(lifted.contains("allow-assertion-drop"), "{lifted}");
    let run = r.check_with_pr(
        &[],
        "allow-assertion-drop: other the reflexivity of the comparison is what this test is for\n",
    );
    assert_eq!(
        codes(&run, "assertion-reduction").len(),
        1,
        "{}",
        run.stdout
    );

    // Every pack: the directive the finding's remediation spells lifts it.
    let mut wrong = Vec::new();
    for lang in langs() {
        let r = repo();
        r.write(lang.path, &lang.source(&[lang.real[0], lang.exact[0]]));
        r.commit("test: add a test");
        let run = r.check(&[]);
        let remediation = run.violations("vacuous-tests")[0]["remediation"]
            .as_str()
            .unwrap()
            .to_string();
        let directive = remediation
            .split('`')
            .find(|part| part.starts_with("allow-vacuous-test: "))
            .unwrap_or_else(|| panic!("{}: no directive in `{remediation}`", lang.name))
            .replace(
                "<reason>",
                "the reflexivity of the comparison is what this test is for",
            );
        let run = r.check_with_pr(&[], &format!("{directive}\n"));
        let got = codes(&run, "vacuous-tests");
        if !got.is_empty() || run.outcome("vacuous-tests")["overrides"] == serde_json::json!([]) {
            wrong.push(format!("{}: `{directive}`: {got:?}", lang.name));
        }
    }
    assert_eq!(wrong, Vec::<String>::new());
}
