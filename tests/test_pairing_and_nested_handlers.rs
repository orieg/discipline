//! Many expected exceptions rewritten in one test, and handlers many deep, are checked
//! in work that grows with their number (#672).
//!
//! The pairing of the expected exceptions of a test compared each base expectation with
//! each head expectation, again at each step of each search, and the body of a handler
//! was walked once for each handler around it. The unit tests beside each hold the work
//! to their number, counted in steps and not in time
//! (`src/ast/expected_exceptions/pairing.rs`, `src/ast/caught_assertions.rs`), and hold
//! the pairing to what the first pairing written returns. Here the binary is given such
//! sources in a change: it returns with its report, and the report names each finding
//! at its line.
//!
//! The limit on each run is there to end a run that hangs. It measures nothing: it is
//! far above what a run takes on a loaded machine.

mod common;
use common::{discipline_cmd, Repo, Run};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// Far above what a run takes; see the module text.
const TIME_LIMIT: Duration = Duration::from_secs(900);

/// `discipline check --format json --base main`, killed at [`TIME_LIMIT`]. Its output
/// goes to files in the git directory, which no gate reads.
fn checked(repo: &Repo) -> Run {
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
    let run = Run {
        code: status.code().unwrap_or(-1),
        stdout: std::fs::read_to_string(&out).unwrap(),
        stderr: std::fs::read_to_string(&err).unwrap(),
    };
    assert!(status.code().is_some(), "the run returns\n{}", run.stderr);
    assert!(run.json()["could_not_check"].is_null(), "{}", run.stderr);
    run
}

/// The lines of `path` at which `gate` reported `code`.
fn lines_reported(run: &Run, gate: &str, code: &str, path: &str) -> Vec<u64> {
    run.violations(gate)
        .iter()
        .filter(|v| v["code"].as_str() == Some(code) && v["file"].as_str() == Some(path))
        .map(|v| v["line"].as_u64().unwrap_or(0))
        .collect()
}

const REDUCTION: &str = "assertion-reduction";
const CAUGHT: &str = "assertion-reduction/assertion-failure-caught";
const WIDENED: &str = "assertion-reduction/expected-exception-widened";

/// How many of each construct a test below holds.
const NUMBER: usize = 40;

/// `line` written for each of `0..n`.
fn lines(n: usize, line: impl Fn(usize) -> String) -> String {
    (0..n).map(line).collect()
}

/// A Python test of [`NUMBER`] `pytest.raises` blocks: block `i` guards `call(i)`, with
/// its own message when `messages`, and the blocks at `wider` accept `Exception`.
fn python_raises(call: &str, messages: bool, wider: &[usize]) -> String {
    format!(
        "import pytest\n\n\ndef test_t():\n{}",
        lines(NUMBER, |i| {
            let class = if wider.contains(&i) {
                "Exception"
            } else {
                "ValueError"
            };
            let matching = if messages {
                format!(", match=\"message {i}\"")
            } else {
                String::new()
            };
            format!("    with pytest.raises({class}{matching}):\n        {call}({i})\n")
        })
    )
}

/// A Python test of [`NUMBER`] expected exceptions, each rewritten (the call it guards
/// is renamed): the two that also accept a wider class are reported, each at its line,
/// and no other. With none wider, nothing is reported. The same holds when each block
/// expects a message of its own.
#[test]
fn python_expected_exceptions_each_rewritten_are_reported_where_they_accept_more() {
    let path = "tests/test_m.py";
    for messages in [false, true] {
        for (wider, want) in [
            (
                vec![6, NUMBER - 1],
                vec![5 + 2 * 6, 5 + 2 * (NUMBER as u64 - 1)],
            ),
            (Vec::new(), Vec::new()),
        ] {
            let repo = Repo::new();
            repo.commit_base(
                path,
                &python_raises("f", messages, &[]),
                "test: add a test of expected exceptions",
            );
            repo.write(path, &python_raises("g", messages, &wider));
            repo.commit("test: call another function in each block");
            let run = checked(&repo);
            assert_eq!(
                lines_reported(&run, REDUCTION, WIDENED, path),
                want,
                "messages: {messages}, wider: {wider:?}"
            );
        }
    }
}

/// A file of two tests in one of the languages whose handlers are asked whether their
/// body fails. Each test holds one assertion in a `try` whose handler holds
/// [`NUMBER`] more handlers one inside another, with a call in each `try`. In the
/// first test nothing in any handler fails; in the second the innermost one rethrows,
/// in a function when `in_function`.
struct Nested {
    path: &'static str,
    before: &'static str,
    /// What begins the test `name`.
    test: fn(&str) -> String,
    end_of_test: &'static str,
    after: &'static str,
    asserting: &'static str,
    calling: &'static str,
    rethrow: &'static str,
    in_function: &'static str,
}

impl Nested {
    fn source(&self, in_function: bool) -> String {
        let handlers = |innermost: &str| {
            format!(
                "{}{}{innermost}{}",
                self.asserting,
                self.calling.repeat(NUMBER),
                "}\n".repeat(NUMBER + 1)
            )
        };
        format!(
            "{}{}{}{}{}{}{}{}",
            self.before,
            (self.test)("swallowed"),
            handlers(""),
            self.end_of_test,
            (self.test)("rethrown"),
            handlers(if in_function {
                self.in_function
            } else {
                self.rethrow
            }),
            self.end_of_test,
            self.after
        )
    }

    /// The lines of the two assertions.
    fn assertions(&self) -> (u64, u64) {
        let first = self.before.lines().count() as u64 + 2;
        // The test's first line, the assertion, the handlers, their ends, the end of
        // the test, and the first line of the second test.
        (first, first + 2 * (NUMBER as u64 + 1) + 2)
    }

    /// The first test's assertion is reported as caught. The second test's is reported
    /// when the rethrow is written in a function, which the handler does not run, and
    /// not when the innermost handler rethrows: that fails every handler around it.
    fn check(&self) {
        let (swallowed, rethrown) = self.assertions();
        for (in_function, want) in [(false, vec![swallowed]), (true, vec![swallowed, rethrown])] {
            let repo = Repo::new();
            repo.write(self.path, &self.source(in_function));
            repo.commit("test: add tests with handlers");
            let run = checked(&repo);
            assert_eq!(
                lines_reported(&run, REDUCTION, CAUGHT, self.path),
                want,
                "{}, in a function: {in_function}",
                self.path
            );
        }
    }
}

#[test]
fn javascript_handlers_one_inside_another_swallow_unless_one_of_them_rethrows() {
    Nested {
        path: "tests/m.test.js",
        before: "",
        test: |name| format!("test('{name}', () => {{\n"),
        end_of_test: "});\n",
        after: "",
        asserting: "try { expect(f(0)).toBe(0); } catch (e) {\n",
        calling: "try { g(); } catch (e) {\n",
        rethrow: "throw e;\n",
        in_function: "const h = () => { throw e; };\n",
    }
    .check();
}

#[test]
fn java_handlers_one_inside_another_swallow_unless_one_of_them_rethrows() {
    Nested {
        path: "src/test/java/MTest.java",
        before: "class MTest {\n",
        test: |name| format!("@Test void {name}() {{\n"),
        end_of_test: "}\n",
        after: "}\n",
        asserting: "try { assertEquals(0, f(0)); } catch (AssertionError e) {\n",
        calling: "try { g(); } catch (AssertionError e) {\n",
        rethrow: "throw e;\n",
        in_function: "Runnable h = () -> { throw e; };\n",
    }
    .check();
}

#[test]
fn kotlin_handlers_one_inside_another_swallow_unless_one_of_them_rethrows() {
    Nested {
        path: "src/test/kotlin/MTest.kt",
        before: "class MTest {\n",
        test: |name| format!("@Test fun {name}() {{\n"),
        end_of_test: "}\n",
        after: "}\n",
        asserting: "try { assertEquals(0, f(0)) } catch (e: AssertionError) {\n",
        calling: "try { g() } catch (e: AssertionError) {\n",
        rethrow: "throw e\n",
        in_function: "val h = { throw e }\n",
    }
    .check();
}

#[test]
fn csharp_handlers_one_inside_another_swallow_unless_one_of_them_rethrows() {
    Nested {
        path: "tests/MTests.cs",
        before: "class MTests {\n",
        test: |name| format!("[Fact] public void {name}() {{\n"),
        end_of_test: "}\n",
        after: "}\n",
        asserting: "try { Assert.Equal(0, F(0)); } catch (Exception e) {\n",
        calling: "try { G(); } catch (Exception e) {\n",
        rethrow: "throw;\n",
        in_function: "Action h = () => { throw e; };\n",
    }
    .check();
}
