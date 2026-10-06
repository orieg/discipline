//! A NUL byte in a source file hides nothing from a gate (#621).
//!
//! A grammar's lexer reads the character 0 as the end of the file in the middle of a
//! token: a comment or a string holding a NUL byte ended there, and what followed was
//! read as an error region that could swallow the next declaration. Every pack now
//! parses a copy in which a NUL byte is a control character that ends no token; each
//! language is driven here through its pack and through the real binary.

mod common;
use common::{Repo, Run, CONFIG_HEAD};

use discipline::ast::{default_registry, AssertVocabulary};

/// How one language spells a file of two tests with one assertion each.
struct Sample {
    path: &'static str,
    /// The file; `{X}` stands between the two tests, on a line of its own.
    file: &'static str,
    /// A line comment holding `{N}`.
    comment: &'static str,
    /// A declaration valid between the two tests, with `{N}` inside a string literal.
    string: &'static str,
}

const SAMPLES: &[Sample] = &[
    Sample {
        path: "tests/a.rs",
        file: "#[test]\nfn a() { assert_eq!(1, 1); }\n{X}\n#[test]\nfn b() { assert_eq!(2, 2); }\n",
        comment: "// c{N}c",
        string: "const S: &str = \"p{N}q\";",
    },
    Sample {
        path: "tests/test_a.py",
        file: "def test_a():\n    assert 1 == 1\n{X}\ndef test_b():\n    assert 2 == 2\n",
        comment: "# c{N}c",
        string: "S = \"p{N}q\"",
    },
    Sample {
        path: "src/a.test.js",
        file: "test('a', () => { expect(1).toBe(1); });\n{X}\ntest('b', () => { expect(2).toBe(2); });\n",
        comment: "// c{N}c",
        string: "const s = \"p{N}q\";",
    },
    Sample {
        path: "src/a.test.ts",
        file: "test('a', () => { expect(1).toBe(1); });\n{X}\ntest('b', () => { expect(2).toBe(2); });\n",
        comment: "// c{N}c",
        string: "const s: string = \"p{N}q\";",
    },
    Sample {
        path: "src/a.test.tsx",
        file: "test('a', () => { expect(1).toBe(1); });\n{X}\ntest('b', () => { expect(2).toBe(2); });\n",
        comment: "// c{N}c",
        string: "const s: string = \"p{N}q\";",
    },
    Sample {
        path: "src/test/java/ATest.java",
        file: "class ATest {\n  @Test\n  void a() { assertEquals(1, 1); }\n{X}\n  @Test\n  void b() { assertEquals(2, 2); }\n}\n",
        comment: "// c{N}c",
        string: "String s = \"p{N}q\";",
    },
    Sample {
        path: "pkg/a_test.go",
        file: "package a\nimport \"testing\"\nfunc TestA(t *testing.T) { if 1 != 1 { t.Fatal(\"x\") } }\n{X}\nfunc TestB(t *testing.T) { if 2 != 2 { t.Fatal(\"y\") } }\n",
        comment: "// c{N}c",
        string: "var s = \"p{N}q\"",
    },
    Sample {
        path: "tests/ATest.php",
        file: "<?php\nclass ATest extends TestCase {\n  public function testA() { $this->assertSame(1, 1); }\n{X}\n  public function testB() { $this->assertSame(2, 2); }\n}\n",
        comment: "// c{N}c",
        string: "const S = \"p{N}q\";",
    },
    Sample {
        path: "tests/test_a.c",
        file: "int test_a(void) {\n    assert(1);\n    return 0;\n}\n{X}\nint test_b(void) {\n    assert(2);\n    return 0;\n}\n",
        comment: "// c{N}c",
        string: "const char *s = \"p{N}q\";",
    },
    Sample {
        path: "tests/a_test.cpp",
        file: "TEST(A, One) { EXPECT_EQ(1, 1); }\n{X}\nTEST(A, Two) { EXPECT_EQ(2, 2); }\n",
        comment: "// c{N}c",
        string: "const char *s = \"p{N}q\";",
    },
    Sample {
        path: "tests/ATest.cs",
        file: "public class ATest {\n  [Fact]\n  public void A() { Assert.Equal(1, 1); }\n{X}\n  [Fact]\n  public void B() { Assert.Equal(2, 2); }\n}\n",
        comment: "// c{N}c",
        string: "string s = \"p{N}q\";",
    },
    Sample {
        path: "test/a_test.rb",
        file: "class ATest < Minitest::Test\n  def test_a\n    assert_equal 1, 1\n  end\n{X}\n  def test_b\n    assert_equal 2, 2\n  end\nend\n",
        comment: "# c{N}c",
        string: "S = \"p{N}q\"",
    },
    Sample {
        path: "src/test/kotlin/ATest.kt",
        file: "class ATest {\n    @Test\n    fun a() { assertEquals(1, 1) }\n{X}\n    @Test\n    fun b() { assertEquals(2, 2) }\n}\n",
        comment: "// c{N}c",
        string: "val s = \"p{N}q\"",
    },
    Sample {
        path: "Tests/ATests.swift",
        file: "import XCTest\nclass ATests: XCTestCase {\n    func testA() { XCTAssertEqual(1, 1) }\n{X}\n    func testB() { XCTAssertEqual(2, 2) }\n}\n",
        comment: "// c{N}c",
        string: "let s = \"p{N}q\"",
    },
    Sample {
        path: "src/test/scala/ASpec.scala",
        file: "class ASpec extends AnyFunSuite {\n  test(\"a\") { assert(1 == 1) }\n{X}\n  test(\"b\") { assert(2 == 2) }\n}\n",
        comment: "// c{N}c",
        string: "val s = \"p{N}q\"",
    },
    Sample {
        path: "Tests/ATests.m",
        file: "@implementation ATests\n- (void)testA { XCTAssertEqual(1, 1); }\n{X}\n- (void)testB { XCTAssertEqual(2, 2); }\n@end\n",
        comment: "// c{N}c",
        string: "static NSString *s = @\"p{N}q\";",
    },
];

/// `(tests read, assertions of each, whether the grammar reported an error)`.
fn read(path: &str, src: &str) -> (usize, Vec<usize>, bool) {
    let reg = default_registry();
    let pack = reg
        .find_pack(path)
        .unwrap_or_else(|| panic!("no pack for {path}"));
    let facts = pack
        .extract(path, src, &AssertVocabulary::default())
        .unwrap_or_else(|e| panic!("{path}: {e}"));
    (
        facts.tests.len(),
        facts.tests.iter().map(|t| t.total_asserts).collect(),
        facts.has_parse_errors,
    )
}

/// A NUL byte inside a comment or a string literal is a character of that comment or
/// string in every pack: both tests are read, and the grammar reports no error. Control:
/// the same file without the line reads the same.
#[test]
fn a_nul_byte_in_a_comment_or_a_string_ends_neither() {
    let mut wrong = Vec::new();
    for s in SAMPLES {
        let clean = read(s.path, &s.file.replace("{X}", ""));
        assert_eq!(clean, (2, vec![1, 1], false), "{}: control", s.path);
        for (name, line) in [("comment", s.comment), ("string", s.string)] {
            let got = read(s.path, &s.file.replace("{X}", &line.replace("{N}", "\0")));
            if got != clean {
                wrong.push(format!("{} {name}: {got:?}", s.path));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// A NUL byte outside a comment and a string is not code in any of these languages: the
/// grammar reports an error, so the file is named as parsed with errors. It is read as
/// any other stray control character is, and not as the end of the file: what a pack
/// reads is what it reads with the byte 2 in that place, at the start of the file too.
#[test]
fn a_nul_byte_between_declarations_is_an_error_like_any_stray_character() {
    let mut wrong = Vec::new();
    for s in SAMPLES {
        for at_start in [false, true] {
            let with = |stray: &str| {
                if at_start {
                    format!("{stray}{}", s.file.replace("{X}", ""))
                } else {
                    s.file.replace("{X}", stray)
                }
            };
            let got = read(s.path, &with("\0"));
            let other = read(s.path, &with("\u{2}"));
            // PHP reads what precedes `<?php` as text, whatever it is.
            let text_before_php = at_start && s.path.ends_with(".php");
            if got != other || got.2 == text_before_php {
                wrong.push(format!(
                    "{} at_start={at_start}: {got:?}, byte 2: {other:?}",
                    s.path
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Line numbers and byte offsets are the file's own: the test after the NUL byte is
/// reported on the line it is written on.
#[test]
fn a_test_after_a_nul_byte_keeps_its_line() {
    let reg = default_registry();
    for s in SAMPLES {
        let pack = reg.find_pack(s.path).unwrap();
        let lines = |src: &str| -> Vec<usize> {
            pack.extract(s.path, src, &AssertVocabulary::default())
                .unwrap()
                .tests
                .iter()
                .map(|t| t.line)
                .collect()
        };
        let plain = lines(&s.file.replace("{X}", &s.comment.replace("{N}", "x")));
        let nul = lines(&s.file.replace("{X}", &s.comment.replace("{N}", "\0")));
        assert_eq!(plain.len(), 2, "{}", s.path);
        assert_eq!(nul, plain, "{}", s.path);
    }
}

fn change(path: &str, base: &str, head: &str) -> Run {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", CONFIG_HEAD);
    repo.write(path, base);
    repo.commit("test: base");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write(path, head);
    repo.commit("refactor: change");
    repo.check(&["--base", "main"])
}

/// Through the binary: a test that follows a comment holding a NUL byte loses its only
/// assertion, and the run reports that test. Control: with a change that keeps the
/// assertion the same file passes, so the NUL byte alone is not what fails the run.
#[test]
fn a_test_weakened_after_a_nul_byte_is_reported() {
    for (ext, check, kept) in [
        (".java", "assertEquals(2, f());", "assertEquals(2, g());"),
        (".cs", "Assert.Equal(2, F());", "Assert.Equal(2, G());"),
        (".rb", "    assert_equal 2, f\n", "    assert_equal 2, g\n"),
    ] {
        let s = SAMPLES.iter().find(|s| s.path.ends_with(ext)).unwrap();
        let base = s
            .file
            .replace("{X}", &s.comment.replace("{N}", "\0"))
            .replace("2, 2", if ext == ".rb" { "2, f" } else { "2, f()" })
            .replace("f())", if ext == ".cs" { "F())" } else { "f())" });
        assert!(base.contains(check) && base.contains('\0'), "{}", s.path);

        let run = change(s.path, &base, &base.replace(check, ""));
        let out = run.json().to_string();
        assert_eq!(run.code, 1, "{}: {}", s.path, run.stdout);
        assert!(
            out.contains("Assertion Count Decreased In Existing Test"),
            "{}: {out}",
            s.path
        );
        assert!(!out.contains("Parsed With Errors"), "{}: {out}", s.path);

        let control = change(s.path, &base, &base.replace(check, kept));
        assert_eq!(control.code, 0, "{}: {}", s.path, control.stdout);
    }
}

/// The fuzz seeds for a NUL byte (`fuzz/corpus/language_packs/*_nul_*`), read as the
/// fuzz target reads them: the first byte selects the pack and the path, the rest is the
/// source.
#[test]
fn the_nul_fuzz_seeds_hide_nothing() {
    let seed = |name: &str| -> String {
        let path = format!(
            "{}/fuzz/corpus/language_packs/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(bytes[1..].contains(&0), "{name} holds no NUL byte");
        String::from_utf8_lossy(&bytes[1..]).into_owned()
    };
    let java = seed("java_nul_in_comment");
    assert_eq!(read("tests/M.java", &java), (2, vec![1, 1], false));
    let kotlin = seed("kotlin_nul_at_start");
    let got = read("tests/m.kt", &kotlin);
    assert_eq!(got, read("tests/m.kt", &kotlin.replace('\0', "\u{2}")));
    assert!(got.2);
    let swift = seed("swift_nul_after_endif");
    assert_eq!(read("tests/m.swift", &swift), (1, vec![1], true));
}

/// Through the binary, outside a test file: a Kotlin source already holds a NUL byte in
/// a comment, and the change adds an empty `catch` after it. The grammar used to end the
/// comment at the byte and lose the rest of the class, so the handler was never seen and
/// the run passed with a warning. Control: the same change with a handler that rethrows
/// passes.
#[test]
fn a_handler_emptied_after_a_nul_byte_is_reported() {
    let file = |body: &str| {
        format!(
            "class A {{\n    fun a(): Int {{ return 1 }}\n// c\0c\n    fun b() {{ {body} }}\n}}\n"
        )
    };
    let path = "src/main/kotlin/A.kt";
    let run = change(
        path,
        &file("save()"),
        &file("try { save() } catch (e: Exception) { }"),
    );
    assert_eq!(run.code, 1, "{}", run.stdout);
    let found = run.violations("error-swallowing");
    assert_eq!(found.len(), 1, "{}", run.stdout);
    assert_eq!(found[0]["title"], "Empty Error Handler Added");
    assert_eq!(found[0]["line"], 4);
    assert!(!run.json().to_string().contains("Parsed With Errors"));

    let control = change(
        path,
        &file("save()"),
        &file("try { save() } catch (e: Exception) { throw e }"),
    );
    assert_eq!(control.code, 0, "{}", control.stdout);
}
