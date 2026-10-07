//! A small, deeply nested source is read in work that grows with its size (#669).
//!
//! The parser library keeps no link from a node to its parent: `Node::parent` descends
//! from the root for each answer, and the sibling methods call it first. A walker that
//! climbed from each nested function to the root with it did work cubic in the nesting,
//! and the call counter read the whole text under each nested call again to blank its
//! strings. Counted in instructions, in a build without optimisation, one extraction of
//! 256 nested Rust functions took 5.8e10 and of 512 took 4.7e11; 1,024 chained Java
//! calls and 512 nested Go calls took their square. Neither the parse budget nor the
//! depth limit bounds that, so one file of a few kilobytes in a change kept a run from
//! finishing, and a run that does not finish reports nothing.
//!
//! The walkers now ask `ast::ancestry::Ancestry`, which answers from the chain of nodes
//! it holds, and the call counter blanks the text of a file once. The unit tests of
//! `src/ast/ancestry.rs` hold the work to the size of the source, counted in steps and
//! not in time. Here the binary is given the same sources in a change: it returns, and
//! it reports the finding that stands at the bottom of the nesting, which it reaches
//! only by reading the file to the end.
//!
//! The limit on each run is there to end a run that hangs. It measures nothing: it is
//! far above what a run takes on a loaded machine.

mod common;
use common::{discipline_cmd, Repo, Run};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// Far above what a run takes; see the module text.
const TIME_LIMIT: Duration = Duration::from_secs(600);

/// `open` written `n` times, `core`, then `close` written `n` times.
fn nest(n: usize, open: impl Fn(usize) -> String, core: &str, close: &str) -> String {
    let mut out: String = (0..n).map(open).collect();
    out.push_str(core);
    out.push_str(&close.repeat(n));
    out
}

/// One nested source: where the file lives, its text, and the line of the test with no
/// assertion that the nesting holds.
struct Nested {
    name: &'static str,
    path: &'static str,
    text: String,
    line: usize,
}

fn nested_sources() -> Vec<Nested> {
    vec![
        Nested {
            name: "256 nested Rust functions",
            path: "tests/deep.rs",
            text: nest(
                256,
                |i| format!("fn f{i}() {{\n"),
                "#[test]\nfn hollow() {}\n",
                "}\n",
            ),
            line: 258,
        },
        Nested {
            name: "256 nested JavaScript functions",
            path: "tests/deep.test.js",
            text: nest(
                256,
                |i| format!("function f{i}() {{\n"),
                "test('hollow', () => {});\n",
                "}\n",
            ),
            line: 257,
        },
        Nested {
            name: "256 nested TypeScript suites",
            path: "tests/suites.test.ts",
            text: nest(
                256,
                |i| format!("describe('d{i}', () => {{\n"),
                "it('hollow', () => {});\n",
                "});\n",
            ),
            line: 257,
        },
        Nested {
            name: "1,024 chained Java calls",
            path: "src/test/java/DeepTest.java",
            text: format!(
                "class DeepTest {{\n  @Test\n  void hollow() {{\n    x{};\n  }}\n}}\n",
                ".a()".repeat(1_024)
            ),
            line: 2,
        },
        Nested {
            name: "512 nested Go calls",
            path: "deep_test.go",
            text: format!(
                "package m\n\nfunc TestHollow(t *testing.T) {{\n\tx := {}\n\t_ = x\n}}\n",
                nest(512, |_| "g(".to_string(), "1", ")")
            ),
            line: 3,
        },
    ]
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

/// The vacuous tests a run reported, as `(file, line)`.
fn vacuous(run: &Run) -> Vec<(String, u64)> {
    run.violations("vacuous-tests")
        .iter()
        .map(|v| {
            (
                v["file"].as_str().unwrap_or("").to_string(),
                v["line"].as_u64().unwrap_or(0),
            )
        })
        .collect()
}

/// A change that adds one deeply nested file is checked by a run that returns with its
/// report, and the report names the test with no assertion at the bottom of the nesting,
/// by its file and line. Each source is the one whose cost the issue measured.
#[test]
fn a_change_of_one_deeply_nested_file_is_checked_and_its_finding_reported() {
    for nested in nested_sources() {
        let repo = Repo::new();
        repo.write(nested.path, &nested.text);
        repo.commit("test: add a nested test file");
        let (exit, run) = check_within_limit(&repo);
        assert_eq!(
            exit,
            Some(1),
            "{}: the run returns with a finding\n{}",
            nested.name,
            run.stderr
        );
        assert!(
            run.json()["could_not_check"].is_null(),
            "{}: every gate ran\n{}",
            nested.name,
            run.stderr
        );
        assert_eq!(
            vacuous(&run),
            vec![(nested.path.to_string(), nested.line as u64)],
            "{}",
            nested.name
        );
    }
}

/// The same files without the nesting are read the same way: the finding is the test's,
/// not the nesting's. One level of each source, and the same test at its bottom.
#[test]
fn the_same_test_without_the_nesting_is_the_same_finding() {
    let shallow = [
        (
            "tests/deep.rs",
            "fn f0() {\n#[test]\nfn hollow() {}\n}\n".to_string(),
            3,
        ),
        (
            "tests/deep.test.js",
            "function f0() {\ntest('hollow', () => {});\n}\n".to_string(),
            2,
        ),
        (
            "src/test/java/DeepTest.java",
            "class DeepTest {\n  @Test\n  void hollow() {\n    x.a();\n  }\n}\n".to_string(),
            2,
        ),
        (
            "deep_test.go",
            "package m\n\nfunc TestHollow(t *testing.T) {\n\tx := g(1)\n\t_ = x\n}\n".to_string(),
            3,
        ),
    ];
    for (path, text, line) in shallow {
        let repo = Repo::new();
        repo.write(path, &text);
        repo.commit("test: add a test file");
        let (exit, run) = check_within_limit(&repo);
        assert_eq!(exit, Some(1), "{path}\n{}", run.stderr);
        assert_eq!(vacuous(&run), vec![(path.to_string(), line)], "{path}");
    }
}

/// A nested file with a real test in it is no finding: the run that reads it to the end
/// passes the gate that reports a test with no assertion.
#[test]
fn a_nested_file_with_an_asserting_test_passes() {
    let repo = Repo::new();
    repo.write(
        "tests/deep.rs",
        &nest(
            256,
            |i| format!("fn f{i}() {{\n"),
            "#[test]\nfn holds() {\n    let x = 1;\n    assert_eq!(x + 1, 2);\n}\n",
            "}\n",
        ),
    );
    repo.commit("test: add a nested test file");
    let (exit, run) = check_within_limit(&repo);
    assert!(exit.is_some(), "the run returns\n{}", run.stderr);
    assert!(run.json()["could_not_check"].is_null(), "{}", run.stderr);
    assert_eq!(vacuous(&run), Vec::<(String, u64)>::new());
}
