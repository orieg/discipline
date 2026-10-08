//! Sources that hold many of one thing are checked in work that grows with their number
//! (#672): many calls that catch what a test expects, many results bound and not looked
//! at, a chain of many promise calls, closures many deep, many tests of one file, many
//! methods of one class, many findings in one file.
//!
//! Each reader read its function, its file or its findings again for each of them. The
//! unit tests beside each reader hold the work to their number, counted in steps and not
//! in time (`src/ast/in_number.rs`, and the tests of `src/ast/caught_assertions.rs`,
//! `src/ast/expected_exceptions.rs`, `src/ast/javascript.rs`, `src/ast/reach.rs` and
//! `src/baseline.rs`). Here the binary is given such sources in a change: it returns
//! with its report, and the report names each finding at its line.
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

/// The run of a change that returned with a report of every gate.
fn checked(repo: &Repo) -> Run {
    let (exit, run) = check_within_limit(repo);
    assert!(exit.is_some(), "the run returns\n{}", run.stderr);
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
const CASES: &str = "assertion-reduction/test-cases-reduced";

/// How many of each construct a test below holds.
const NUMBER: usize = 40;

/// `line` written for each of `0..n`.
fn lines(n: usize, line: impl Fn(usize) -> String) -> String {
    (0..n).map(line).collect()
}

/// A Java test of [`NUMBER`] `catchThrowable` calls, each bound to its own name and
/// asserted to be of a class: the calls at `wider` accept `RuntimeException`.
fn java_catches(wider: &[usize]) -> String {
    format!(
        "class MTest {{\n    @Test\n    void t() {{\n{}    }}\n}}\n",
        lines(NUMBER, |i| {
            let class = if wider.contains(&i) {
                "RuntimeException"
            } else {
                "IllegalArgumentException"
            };
            format!(
                "        Throwable t{i} = catchThrowable(() -> f({i}));\n        assertThat(t{i}).isInstanceOf({class}.class);\n"
            )
        })
    )
}

/// A Java method of [`NUMBER`] `catchThrowable` calls is read with the assertion on each
/// caught value: the two whose assertion accepts a wider class are reported, each at the
/// line of its call, and no other.
#[test]
fn java_catch_throwable_calls_are_each_compared_by_their_own_assertion() {
    let path = "src/test/java/MTest.java";
    let repo = Repo::new();
    repo.commit_base(
        path,
        &java_catches(&[]),
        "test: add a test of caught values",
    );
    repo.write(path, &java_catches(&[6, NUMBER - 1]));
    repo.commit("test: accept a wider class twice");
    let run = checked(&repo);
    assert_eq!(
        lines_reported(&run, REDUCTION, WIDENED, path),
        vec![4 + 2 * 6, 4 + 2 * (NUMBER as u64 - 1)]
    );
}

/// A Kotlin test of [`NUMBER`] `runCatching` results bound and not used: each assertion
/// is reported as caught, at its line. With each result used on the next line, none is.
#[test]
fn kotlin_run_catching_results_are_reported_unless_they_are_used() {
    let path = "src/test/kotlin/MTest.kt";
    for (used, want) in [
        (false, (4..4 + NUMBER as u64).collect::<Vec<_>>()),
        (true, Vec::new()),
    ] {
        let repo = Repo::new();
        repo.write(
            path,
            &format!(
                "class MTest {{\n    @Test\n    fun t() {{\n{}    }}\n}}\n",
                lines(NUMBER, |i| format!(
                    "        val r{i} = runCatching {{ assertEquals(1, f({i})) }}{}\n",
                    if used {
                        format!("; check(r{i})")
                    } else {
                        String::new()
                    }
                ))
            ),
        );
        repo.commit("test: add a test of results");
        let run = checked(&repo);
        assert_eq!(
            lines_reported(&run, REDUCTION, CAUGHT, path),
            want,
            "used: {used}"
        );
    }
}

/// A JavaScript test of one promise chain of [`NUMBER`] `.then().catch()` links: each
/// assertion is reported as caught, once, at its line. A source file of the same change
/// holds a chain of ten times as many calls, which is read too.
#[test]
fn javascript_chains_of_calls_are_read_and_their_caught_assertions_reported() {
    let path = "tests/m.test.js";
    let repo = Repo::new();
    repo.write(
        path,
        &format!(
            "test('t', () => {{\n  return p\n{}  ;\n}});\n",
            lines(NUMBER, |i| format!(
                "    .then(() => {{ expect(a).toBe({i}); }}).catch(() => {{}})\n"
            ))
        ),
    );
    repo.write(
        "src/m.js",
        &format!(
            "function run() {{\n  return x{};\n}}\n",
            ".a()".repeat(10 * NUMBER)
        ),
    );
    repo.commit("test: add a chain of promise calls");
    let run = checked(&repo);
    assert_eq!(
        lines_reported(&run, REDUCTION, CAUGHT, path),
        (3..3 + NUMBER as u64).collect::<Vec<_>>()
    );
}

/// A Rust test of [`NUMBER`] `catch_unwind` closures one inside another, with nothing
/// looking at any result: the assertion of each is reported as caught, once.
#[test]
fn rust_unwind_closures_one_inside_another_are_each_reported_once() {
    let path = "tests/unwind.rs";
    let repo = Repo::new();
    repo.write(
        path,
        &format!(
            "#[test]\nfn t() {{\n{}{}}}\n",
            lines(NUMBER, |i| format!(
                "let r{i} = std::panic::catch_unwind(|| {{ assert_eq!(f({i}), {i});\n"
            )),
            "});\n".repeat(NUMBER)
        ),
    );
    repo.commit("test: add unwind closures");
    let run = checked(&repo);
    assert_eq!(
        lines_reported(&run, REDUCTION, CAUGHT, path),
        (3..3 + NUMBER as u64).collect::<Vec<_>>()
    );
}

/// A Python file of [`NUMBER`] tests that each swallow their assertion, under a class of
/// as many methods one of which a second class overrides: each assertion is reported as
/// caught, at its line, in its own test.
#[test]
fn python_tests_and_methods_in_number_are_read_and_each_caught_assertion_reported() {
    let path = "tests/test_m.py";
    let repo = Repo::new();
    repo.write(
        path,
        &format!(
            "class Base:\n{}class Impl(Base):\n    def m0(self):\n        return 1\n{}",
            lines(NUMBER, |i| format!(
                "    def m{i}(self):\n        return {i}\n"
            )),
            lines(NUMBER, |i| format!(
                "def test_{i}():\n    try:\n        assert f({i}) == {i}\n    except AssertionError:\n        pass\n"
            ))
        ),
    );
    repo.commit("test: add tests that swallow");
    let run = checked(&repo);
    let first = 1 + 2 * NUMBER as u64 + 3 + 3;
    assert_eq!(
        lines_reported(&run, REDUCTION, CAUGHT, path),
        (0..NUMBER as u64)
            .map(|i| first + 5 * i)
            .collect::<Vec<_>>()
    );
}

/// A Go test file of [`NUMBER`] tests, each ranging over a package-level table of its
/// own: `rows(i)` is the number of rows of the table of test `i`.
fn go_tables(rows: impl Fn(usize) -> usize) -> String {
    format!(
        "package p\n\n{}{}",
        lines(NUMBER, |i| format!(
            "var table{i} = []struct{{ a int }}{{{}}}\n",
            (0..rows(i))
                .map(|r| format!("{{{r}}}"))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        lines(NUMBER, |i| {
            format!(
            "func Test{i}(t *testing.T) {{\n\tfor _, v := range table{i} {{\n\t\tassert.Equal(t, {i}, f(v))\n\t}}\n}}\n"
        )
        })
    )
}

/// A Go file of [`NUMBER`] tests is read with the package-level table each test names:
/// the two tests whose table lost a row are reported, and no other.
#[test]
fn go_tests_of_one_file_each_count_the_rows_of_their_own_table() {
    let path = "m_test.go";
    let repo = Repo::new();
    repo.commit_base(path, &go_tables(|_| 3), "test: add table tests");
    repo.write(
        path,
        &go_tables(|i| if i == 5 || i == NUMBER - 1 { 2 } else { 3 }),
    );
    repo.commit("test: drop a row of two tables");
    let run = checked(&repo);
    let first = 3 + NUMBER as u64;
    assert_eq!(
        lines_reported(&run, REDUCTION, CASES, path),
        vec![first + 5 * 5, first + 5 * (NUMBER as u64 - 1)]
    );
}

/// A source file that gains ten times [`NUMBER`] empty handlers, all written the same:
/// each is reported at its line, and each finding has a fingerprint of its own.
#[test]
fn findings_in_number_in_one_file_are_each_reported_with_their_own_fingerprint() {
    let (path, handlers) = ("src/m.py", 10 * NUMBER);
    let repo = Repo::new();
    repo.write(
        path,
        &format!(
            "def run():\n{}",
            lines(handlers, |_| {
                "    try:\n        f()\n    except Exception:\n        pass\n".to_string()
            })
        ),
    );
    repo.commit("feat: add handlers");
    let run = checked(&repo);
    let found = run.violations("error-swallowing");
    assert_eq!(
        found
            .iter()
            .map(|v| v["line"].as_u64().unwrap_or(0))
            .collect::<Vec<_>>(),
        (0..handlers as u64).map(|i| 4 + 4 * i).collect::<Vec<_>>()
    );
    let fingerprints: std::collections::HashSet<&str> = found
        .iter()
        .map(|v| v["fingerprint"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(fingerprints.len(), handlers);
}
