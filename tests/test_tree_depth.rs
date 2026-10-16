//! A syntax tree has a depth limit, and the work has a stack that holds it (#667). Most
//! tree walkers call themselves once for each level of the tree, so a source nested
//! deeply enough ended the process with a stack overflow: no report, from any gate.
//!
//! Two things answer that. The binary does its work on a thread with a deep stack
//! (`src/deep_stack.rs`), and a tree deeper than 4,096 levels is refused where it is
//! parsed, before any walker has it, as a file that could not be parsed: exit 2 from
//! the gates that need every changed file's facts, and a note from each gate that reads
//! what it can. The limit is twice the deepest source measured in other projects, and
//! a tree at the limit takes a small part of the deep stack.
//!
//! The limit is counted in nodes from the root of the tree, so the same file gets the
//! same answer on every machine.
//!
//! A test thread has a default stack, which a tree far inside the limit exhausts. So
//! every test here that hands a pack a deeply nested source does it the way the binary
//! does, inside `on_deep_stack`, or through the binary itself; and the tests of what
//! happens without the bound or without the deep stack run in a child process, where
//! a stack overflow is one failed assertion and the suite goes on.

mod common;
use common::{discipline_cmd, isolate_env, Repo, Run, CONFIG_HEAD};
use discipline::ast::{default_registry, AssertVocabulary};
use discipline::deep_stack::on_deep_stack;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The deepest tree read, in levels (`TREE_DEPTH_LIMIT` in `src/ast/source_text.rs`).
const LIMIT: usize = 4_096;

/// Nesting far past the limit, and past what the main thread's own stack let most
/// packs descend before the work had a stack of its own (the Rust pack stopped at
/// 1,280 nested parentheses there, the Python pack at 2,944, the JavaScript pack at
/// 7,680).
const FAR_PAST: usize = 20_000;

/// Far above what a refused run takes.
const TIME_LIMIT: Duration = Duration::from_secs(240);

/// The gates that need the facts of every changed file and stop on one they cannot get.
const NEED_EVERY_FILE: &[&str] = &[
    "assertion-reduction",
    "vacuous-tests",
    "ignored-tests",
    "unsafe-safety-comment",
    "deletion-rationale",
];

/// One test file of a language: where it lives, and its text around the expression
/// `VALUE`.
struct Lang {
    name: &'static str,
    path: &'static str,
    file: &'static str,
    /// Levels of the tree down to the innermost literal when `VALUE` is `1`: the file,
    /// the test, its body, the statement and the literal.
    base: usize,
}

const LANGS: &[Lang] = &[
    Lang {
        name: "rust",
        path: "tests/deep.rs",
        file: "#[test]\nfn t() {\n    let a = VALUE;\n    assert_eq!(a, 1);\n}\n",
        base: 5,
    },
    Lang {
        name: "python",
        path: "tests/test_deep.py",
        file: "def test_t():\n    a = VALUE\n    assert a == 1\n",
        base: 6,
    },
    Lang {
        name: "javascript",
        path: "tests/deep.test.js",
        file: "test('t', () => {\n  const a = VALUE;\n  expect(a).toBe(1);\n});\n",
        base: 9,
    },
    Lang {
        name: "typescript",
        path: "tests/deep.test.ts",
        file: "test('t', () => {\n  const a = VALUE;\n  expect(a).toBe(1);\n});\n",
        base: 9,
    },
    Lang {
        name: "tsx",
        path: "tests/deep.test.tsx",
        file: "test('t', () => {\n  const a = VALUE;\n  expect(a).toBe(1);\n});\n",
        base: 9,
    },
    Lang {
        name: "java",
        path: "src/test/java/DeepTest.java",
        file: "class DeepTest {\n  @Test\n  void t() {\n    int a = VALUE;\n    assertEquals(1, a);\n  }\n}\n",
        base: 8,
    },
    Lang {
        name: "go",
        path: "deep_test.go",
        file: "package m\n\nfunc TestT(t *testing.T) {\n\ta := VALUE\n\tif a != 1 {\n\t\tt.Fatal()\n\t}\n}\n",
        base: 7,
    },
    Lang {
        name: "php",
        path: "tests/DeepTest.php",
        file: "<?php\nclass DeepTest extends TestCase {\n  public function testT() {\n    $a = VALUE;\n    $this->assertEquals(1, $a);\n  }\n}\n",
        base: 8,
    },
    Lang {
        name: "c",
        path: "tests/test_deep.c",
        file: "void test_t(void) {\n  int a = VALUE;\n  assert(a == 1);\n}\n",
        base: 6,
    },
    Lang {
        name: "cpp",
        path: "tests/test_deep.cpp",
        file: "#include <gtest/gtest.h>\n\nTEST(S, T) {\n  int a = VALUE;\n  EXPECT_EQ(a, 1);\n}\n",
        base: 6,
    },
    Lang {
        name: "csharp",
        path: "tests/DeepTests.cs",
        file: "class DeepTests {\n  [Fact]\n  public void T() {\n    var a = VALUE;\n    Assert.Equal(1, a);\n  }\n}\n",
        base: 9,
    },
    Lang {
        name: "ruby",
        path: "test/deep_test.rb",
        file: "class DeepTest < Minitest::Test\n  def test_t\n    a = VALUE\n    assert_equal 1, a\n  end\nend\n",
        base: 7,
    },
    Lang {
        name: "kotlin",
        path: "src/test/kotlin/DeepTest.kt",
        file: "import kotlin.test.Test\nimport kotlin.test.assertEquals\n\nclass DeepTest {\n  @Test\n  fun t() {\n    val a = VALUE\n    assertEquals(1, a)\n  }\n}\n",
        base: 8,
    },
    Lang {
        name: "swift",
        path: "tests/DeepTests.swift",
        file: "class DeepTests: XCTestCase {\n  func testT() {\n    let a = VALUE\n    XCTAssertEqual(a, 1)\n  }\n}\n",
        base: 8,
    },
    Lang {
        name: "scala",
        path: "src/test/scala/DeepTest.scala",
        file: "class DeepTest extends AnyFunSuite {\n  test(\"t\") {\n    val a = VALUE\n    assert(a == 1)\n  }\n}\n",
        base: 7,
    },
    Lang {
        name: "objc",
        path: "tests/DeepTests.m",
        file: "@implementation DeepTests\n- (void)testT {\n  int a = VALUE;\n  XCTAssertEqual(a, 1);\n}\n@end\n",
        base: 8,
    },
];

impl Lang {
    /// The file with its value inside `pairs` pairs of parentheses, each one level of
    /// the tree in every grammar here.
    fn nested(&self, pairs: usize) -> String {
        let value = format!("{}1{}", "(".repeat(pairs), ")".repeat(pairs));
        self.file.replace("VALUE", &value)
    }

    /// The pairs that make the tree exactly `LIMIT` levels deep.
    fn pairs_at_limit(&self) -> usize {
        LIMIT - self.base
    }

    fn refusal(&self, depth: usize) -> String {
        format!(
            "could not parse `{}`: the source nests {depth} levels deep, past the {LIMIT} this tool reads",
            self.path
        )
    }
}

fn lang(name: &str) -> &'static Lang {
    LANGS.iter().find(|l| l.name == name).unwrap()
}

/// What the pack of `lang` makes of the file nested `pairs` deep, read on the deep
/// stack as the binary reads it: the tests it found, or why it has no facts.
fn extract(lang: &Lang, pairs: usize) -> Result<usize, String> {
    on_deep_stack(|| {
        let registry = default_registry();
        let pack = registry
            .find_pack(lang.path)
            .unwrap_or_else(|| panic!("no pack for {}", lang.path));
        pack.extract(lang.path, &lang.nested(pairs), &AssertVocabulary::default())
            .map(|facts| facts.tests.len())
            .map_err(|e| e.to_string())
    })
    .unwrap()
}

/// Every pack reads the deepest nesting the limit allows as it reads any file, and
/// refuses the next one by name, one level apart.
#[test]
fn every_pack_reads_a_tree_at_the_limit_and_refuses_one_level_deeper() {
    for lang in LANGS {
        let at_limit = lang.pairs_at_limit();
        assert_eq!(
            extract(lang, at_limit),
            Ok(1),
            "{}: the file nested to the limit is read, and its test found",
            lang.name
        );
        assert_eq!(
            extract(lang, at_limit + 1),
            Err(lang.refusal(LIMIT + 1)),
            "{}",
            lang.name
        );
        // The shallow file is the same file: one test, no refusal.
        assert_eq!(extract(lang, 1), Ok(1), "{}", lang.name);
    }
}

/// The work of [`the_deep_stack_holds_a_tree_at_the_limit_for_every_pack`], which runs
/// this test in a child process whose environment names one language; with no language
/// named it has nothing to do. The file nested to the limit is read on the deep stack,
/// and the one nested far past it is refused by name.
#[test]
fn deep_stack_probe() {
    let Ok(name) = std::env::var("TREE_DEPTH_PROBE_LANG") else {
        return;
    };
    let lang = lang(&name);
    let outcome = (
        extract(lang, FAR_PAST),
        extract(lang, lang.pairs_at_limit()),
    );
    assert_eq!(
        outcome,
        (Err(lang.refusal(FAR_PAST + lang.base)), Ok(1)),
        "{name}"
    );
    println!("deep-stack probe of {name}: refused and read");
}

/// `on_deep_stack` holds what the limit lets through: each pack reads a file nested to
/// the limit on it, and refuses one nested 20,000 deep before a walker has it. Each
/// pack runs in a child process. Were the limit raised past what the stack holds, or
/// the bound gone, the walker would descend until the stack ended and the child would
/// die; that is one failed assertion here.
#[test]
fn the_deep_stack_holds_a_tree_at_the_limit_for_every_pack() {
    let mut failed = Vec::new();
    for lang in LANGS {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        isolate_env(&mut cmd);
        let out = cmd
            .args(["--exact", "deep_stack_probe", "--nocapture"])
            .env("TREE_DEPTH_PROBE_LANG", lang.name)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let reported = stdout.contains(&format!(
            "deep-stack probe of {}: refused and read",
            lang.name
        ));
        if !out.status.success() || !reported {
            failed.push(format!(
                "{}: {} ({})",
                lang.name,
                out.status,
                String::from_utf8_lossy(&out.stderr)
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
            ));
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");
}

/// The binary does its work on the deep stack: for every pack, a change that adds a
/// file nested to the limit is checked by a process that exits with its report, and
/// one nested 20,000 deep is refused by name. A tree at the limit is several times
/// deeper than the main thread's own stack lets most packs descend, so a binary that
/// did the work there dies on the first of the two, by a signal and with no report:
/// `None` for its exit code, which is what this test fails on.
#[test]
fn the_binary_survives_a_tree_at_the_limit_and_one_far_past_it_for_every_pack() {
    let mut failed = Vec::new();
    for lang in LANGS {
        let repo = Repo::new();
        repo.write(lang.path, &lang.nested(lang.pairs_at_limit()));
        repo.commit("test: add a test file");
        let (exit, run) = check_within_limit(&repo);
        let checked = exit.is_some()
            && exit != Some(2)
            && serde_json::from_str::<serde_json::Value>(&run.stdout)
                .is_ok_and(|report| report["could_not_check"].is_null());
        if !checked {
            failed.push(format!(
                "{} at the limit: exit {exit:?} ({})",
                lang.name,
                run.stderr.lines().last().unwrap_or("")
            ));
        }

        let repo = Repo::new();
        repo.write(lang.path, &lang.nested(FAR_PAST));
        repo.commit("test: add a test file");
        let (exit, run) = check_within_limit(&repo);
        let refused = exit == Some(2) && run.stderr.contains(&lang.refusal(FAR_PAST + lang.base));
        if !refused {
            failed.push(format!(
                "{} far past the limit: exit {exit:?} ({})",
                lang.name,
                run.stderr.lines().last().unwrap_or("")
            ));
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");
}

/// `discipline check --format json --base main`, killed at [`TIME_LIMIT`]. Its output
/// goes to files in the git directory, which no gate reads. `None` for the exit code of
/// a process a signal ended.
fn check_within_limit(repo: &Repo) -> (Option<i32>, Run) {
    let out = repo.file(".git/check.out");
    let err = repo.file(".git/check.err");
    let mut cmd = discipline_cmd(repo.path());
    cmd.args(["check", "--format", "json", "--base", "main"])
        .env("PR_TITLE", "chore: test PR (#101)")
        .env("PR_BODY", "no-issue: a test change")
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(&out).unwrap())
        .stderr(std::fs::File::create(&err).unwrap());
    let mut child = cmd.spawn().unwrap();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > TIME_LIMIT {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("`discipline check` did not return");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    (
        status.code(),
        Run {
            code: status.code().unwrap_or(-1),
            stdout: std::fs::read_to_string(&out).unwrap(),
            stderr: std::fs::read_to_string(&err).unwrap(),
        },
    )
}

fn notes(run: &Run, gate: &str) -> Vec<String> {
    run.outcome(gate)["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().to_string())
        .collect()
}

/// A change that adds a file nested far past the limit gets an answer that names the
/// file, from a process that exits: it is not ended by a signal.
fn a_change_nested_far_past_the_limit_is_refused_by_name(name: &str) {
    let lang = lang(name);
    let repo = Repo::new();
    repo.write(lang.path, &lang.nested(FAR_PAST));
    repo.commit("test: add a test file");
    let (exit, run) = check_within_limit(&repo);
    assert_eq!(
        exit,
        Some(2),
        "{name}: the process must exit, with 2\n{}\n{}",
        run.stdout,
        run.stderr
    );
    let (reason, gate) = run.could_not_check();
    assert_eq!(reason, "gate");
    assert_eq!(gate.as_deref(), Some("assertion-reduction"));
    let detail = run.json()["could_not_check"]["detail"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        detail,
        format!(
            "gate `assertion-reduction` could not run: {}",
            lang.refusal(FAR_PAST + lang.base)
        )
    );
    assert!(run.stderr.contains(&detail), "{}", run.stderr);
    assert!(
        !run.stderr.contains("overflowed its stack"),
        "{}",
        run.stderr
    );
    // No gate reported an outcome, so nothing reads as a pass.
    assert_eq!(run.json()["outcomes"].as_array().unwrap().len(), 0);

    // The control: the same change with the file nested to the limit is checked, and
    // one level deeper it is refused, so the two differ by one level of the tree.
    let repo = Repo::new();
    repo.write(lang.path, &lang.nested(lang.pairs_at_limit()));
    repo.commit("test: add a test file");
    let (exit, run) = check_within_limit(&repo);
    assert_eq!(exit, Some(0), "{name}\n{}\n{}", run.stdout, run.stderr);
    assert!(run.json()["could_not_check"].is_null(), "{}", run.stdout);
    assert!(!run.stdout.contains("levels deep"), "{}", run.stdout);
    assert!(!run.json()["outcomes"].as_array().unwrap().is_empty());

    let repo = Repo::new();
    repo.write(lang.path, &lang.nested(lang.pairs_at_limit() + 1));
    repo.commit("test: add a test file");
    let (exit, run) = check_within_limit(&repo);
    assert_eq!(exit, Some(2), "{name}\n{}\n{}", run.stdout, run.stderr);
    let detail = run.json()["could_not_check"]["detail"].to_string();
    assert!(detail.contains(&lang.refusal(LIMIT + 1)), "{detail}");
}

#[test]
fn a_rust_change_nested_far_past_the_limit_is_refused_by_name() {
    a_change_nested_far_past_the_limit_is_refused_by_name("rust");
}

#[test]
fn a_python_change_nested_far_past_the_limit_is_refused_by_name() {
    a_change_nested_far_past_the_limit_is_refused_by_name("python");
}

#[test]
fn a_javascript_change_nested_far_past_the_limit_is_refused_by_name() {
    a_change_nested_far_past_the_limit_is_refused_by_name("javascript");
}

#[test]
fn a_c_change_nested_far_past_the_limit_is_refused_by_name() {
    a_change_nested_far_past_the_limit_is_refused_by_name("c");
}

#[test]
fn a_go_change_nested_far_past_the_limit_is_refused_by_name() {
    a_change_nested_far_past_the_limit_is_refused_by_name("go");
}

/// With the gates that stop on such a file switched off, every other gate that reads
/// source says it could not parse the file: none of them passes it in silence, and none
/// descends it.
#[test]
fn each_gate_that_reads_what_it_can_notes_the_file() {
    let rust = lang("rust");
    let deep = rust.nested(FAR_PAST);
    let why = format!(
        "the source nests {} levels deep, past the {LIMIT} this tool reads",
        FAR_PAST + rust.base
    );
    let mut config = format!("{CONFIG_HEAD}[tests]\nfunctions = [\"self_test\"]\n");
    for gate in NEED_EVERY_FILE {
        config.push_str(&format!("[gates.{gate}]\nenabled = false\n"));
    }
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", &config);
    repo.write(
        "tests/prop.rs",
        "fn f() { let c = ProptestConfig { cases: 5000, ..Default::default() }; }\n",
    );
    repo.commit("chore: configuration and a property test");
    repo.git(&["checkout", "-q", "work"]);
    repo.git(&["merge", "-q", "--ff-only", "main"]);
    repo.write("src/deep.rs", &deep);
    repo.write("tests/prop.rs", &deep);
    repo.commit("feat: add a module");
    let (exit, run) = check_within_limit(&repo);
    assert!(exit.is_some(), "the process must exit\n{}", run.stderr);
    assert!(run.json()["could_not_check"].is_null(), "{}", run.stdout);

    let could_not = |path: &str| format!("could not parse `{path}`: {why}");
    for gate in ["stub-bodies", "error-swallowing", "suppression-delta"] {
        for path in ["src/deep.rs", "tests/prop.rs"] {
            let expected = format!(
                "`{path}`: not analysed, the head side does not parse ({})",
                could_not(path)
            );
            assert!(
                notes(&run, gate).contains(&expected),
                "{gate}: {:?}",
                notes(&run, gate)
            );
        }
    }
    assert!(
        notes(&run, "instruction-smuggling").contains(&format!(
            "{}, so its comments and strings were not read for instruction phrases",
            could_not("src/deep.rs")
        )),
        "{:?}",
        notes(&run, "instruction-smuggling")
    );
    assert!(
        notes(&run, "test-budget").contains(&format!(
            "{}, so its test budgets were not compared",
            could_not("tests/prop.rs")
        )),
        "{:?}",
        notes(&run, "test-budget")
    );
    assert!(
        notes(&run, "pii").contains(&format!(
            "{}, so no line of it is read as inside a declared test function",
            could_not("src/deep.rs")
        )),
        "{:?}",
        notes(&run, "pii")
    );
    let floor = notes(&run, "test-floor");
    assert!(
        floor.iter().any(|n| n
            .starts_with("head: 2 file(s) could not be read, or parse with errors")
            && n.contains("src/deep.rs")
            && n.contains("tests/prop.rs")),
        "{floor:?}"
    );
}

/// One run of the gates over a change that adds `text` at `path`: the exit code, the
/// titles of the findings, and the detail of a run that could not check.
fn added(path: &str, text: &str) -> (Option<i32>, Vec<String>, Option<String>) {
    let repo = Repo::new();
    repo.write(path, text);
    repo.commit("test: add a test");
    let (exit, run) = check_within_limit(&repo);
    let report = run.json();
    let titles = report["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|o| o["violations"].as_array().unwrap().clone())
        .map(|v| v["title"].as_str().unwrap().to_string())
        .collect();
    let detail = report["could_not_check"]["detail"]
        .as_str()
        .map(str::to_string);
    (exit, titles, detail)
}

/// A part of a file that a pack parses again on its own is held to the limit as the
/// file is, and a part past it refuses the file. A Rust macro's arguments are a flat
/// run of tokens in the file's tree, so the file passes the depth check whatever they
/// nest to when read as an expression: `assert!(!!..!true)` with 4,200 `!` would be
/// counted as an assertion nobody judged, and a test that asserts a constant would pass
/// as one that checks something. With 100 `!` the argument is read and the test named.
#[test]
fn a_rust_macro_argument_nested_past_the_limit_refuses_the_file() {
    let test = |nots: usize| {
        format!(
            "#[test]\nfn constant() {{\n    assert!({}true);\n}}\n",
            "!".repeat(nots)
        )
    };
    let (exit, titles, detail) = added("tests/constant.rs", &test(100));
    assert_eq!(exit, Some(1));
    assert_eq!(titles, ["Vacuous Test Added"]);
    assert_eq!(detail, None);

    let (exit, titles, detail) = added("tests/constant.rs", &test(4_200));
    assert_eq!(exit, Some(2), "{titles:?}");
    assert!(titles.is_empty(), "{titles:?}");
    assert_eq!(
        detail.as_deref(),
        Some(
            "gate `assertion-reduction` could not run: could not parse `tests/constant.rs`: \
             in a part of it parsed on its own, the source nests 4207 levels deep, past the \
             4096 this tool reads"
        )
    );
}

/// The same for a Python skip condition written as a string, which is one node of the
/// file's tree: `skipif("((..True..))")` is an unconditional skip at any nesting. Nested
/// past the limit the condition has no tree and the file is refused, where it would
/// have been counted as a skip under a condition nobody read.
#[test]
fn a_python_skip_condition_nested_past_the_limit_refuses_the_file() {
    let test = |pairs: usize| {
        format!(
            "import pytest\n\n@pytest.mark.skipif(\"{}True{}\")\ndef test_t():\n    assert compute() == 3\n",
            "(".repeat(pairs),
            ")".repeat(pairs)
        )
    };
    let (exit, titles, detail) = added("tests/test_skip.py", &test(100));
    assert_eq!(exit, Some(1));
    assert_eq!(titles, ["Ignored Test Added"]);
    assert_eq!(detail, None);

    let (exit, titles, detail) = added("tests/test_skip.py", &test(4_200));
    assert_eq!(exit, Some(2), "{titles:?}");
    assert!(titles.is_empty(), "{titles:?}");
    assert_eq!(
        detail.as_deref(),
        Some(
            "gate `assertion-reduction` could not run: could not parse `tests/test_skip.py`: \
             in a part of it parsed on its own, the source nests 4203 levels deep, past the \
             4096 this tool reads"
        )
    );
}

/// The body of a Rust property test is parsed again as a function. One nested past the
/// limit is a body the grammar could not read: the file is reported as parsed with
/// errors and its test as one that checks nothing, not counted as far as it went.
#[test]
fn a_rust_property_body_nested_past_the_limit_is_reported_as_unread() {
    let test = |nots: usize| {
        format!(
            "use proptest::prelude::*;\n\nproptest! {{\n    #[test]\n    fn holds(x in 0..10u8) {{\n        let _y = {}true;\n        prop_assert!(x < 10);\n    }}\n}}\n",
            "!".repeat(nots)
        )
    };
    let (exit, titles, detail) = added("tests/property.rs", &test(100));
    assert_eq!((exit, detail), (Some(0), None));
    assert!(titles.is_empty(), "{titles:?}");

    let (exit, titles, detail) = added("tests/property.rs", &test(4_200));
    assert_eq!((exit, detail), (Some(1), None));
    assert_eq!(
        titles,
        ["Source File Parsed With Errors", "Vacuous Test Added"]
    );
}

/// The fuzz seeds of this defect: the first byte selects the pack and the path in the
/// `language_packs` target, the rest is a file nested past the limit. Each is refused
/// by name by the pack the byte selects.
#[test]
fn the_fuzz_seeds_nested_past_the_limit_are_refused_by_name() {
    // The selector byte: the index of the path in the target's list, and bit 5 for a
    // path under `tests/`.
    for (seed, path, selector) in [
        ("rust_nested_past_the_depth_limit", "tests/lib.rs", 0x20u8),
        ("python_nested_past_the_depth_limit", "tests/m.py", 0x21),
        ("js_nested_past_the_depth_limit", "tests/m.js", 0x22),
        ("c_nested_past_the_depth_limit", "tests/m.c", 0x28),
    ] {
        let bytes = std::fs::read(format!(
            "{}/fuzz/corpus/language_packs/{seed}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        assert_eq!(bytes[0], selector, "{seed}");
        let source = String::from_utf8(bytes[1..].to_vec()).unwrap();
        let refused = on_deep_stack(|| {
            let registry = default_registry();
            let pack = registry.find_pack(path).unwrap();
            pack.extract(path, &source, &AssertVocabulary::default())
                .map(|_| ())
                .unwrap_err()
                .to_string()
        })
        .unwrap();
        assert!(
            refused.starts_with(&format!("could not parse `{path}`: the source nests "))
                && refused.ends_with(&format!(" levels deep, past the {LIMIT} this tool reads")),
            "{seed}: {refused}"
        );
    }
}
