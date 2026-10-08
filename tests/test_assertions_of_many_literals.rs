//! One assertion that holds many expected values or bounds is read in work that grows
//! with what is recorded for it (#672).
//!
//! An expected value and a bound are each recorded with the text of the whole assertion
//! they stand in. A fuzzing run found a TypeScript source of 333 nested suites, 24 kB,
//! in which one matcher name was cut short: one `expect` statement then held the 260
//! suites after it. For each of the 262 expected values in it the reader listed every
//! call of the statement again, each call written as its whole text, and compared the
//! new record with every one before it. Counted in instructions, in a build without
//! optimisation, a run on that change was stopped at 4.0e12 with no report; one
//! extraction of the file now takes 8.3e9.
//!
//! The unit tests of `src/ast/expectations.rs` hold the work to what is recorded,
//! counted in steps and not in time. Here the binary is given such sources in a change:
//! it returns with its report, and it still reports the one expected value or bound
//! that the change moved among the many of the statement.
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

/// What `assertion-reduction` reported, as `(code, file, line)`; 0 for no line.
fn reported(run: &Run) -> Vec<(String, String, u64)> {
    run.violations("assertion-reduction")
        .iter()
        .map(|v| {
            (
                v["code"].as_str().unwrap_or("").to_string(),
                v["file"].as_str().unwrap_or("").to_string(),
                v["line"].as_u64().unwrap_or(0),
            )
        })
        .collect()
}

/// `item` written for each of `0..n`, joined by `between`.
fn row(n: usize, item: impl Fn(usize) -> String, between: &str) -> String {
    (0..n).map(item).collect::<Vec<_>>().join(between)
}

/// How many literals each statement below holds.
const LITERALS: usize = 300;

/// The source a fuzzing run found, as the fuzzing target reads it: the first byte picks
/// the path, the rest is the text.
fn the_found_source() -> String {
    let seed = std::fs::read(format!(
        "{}/fuzz/corpus/language_packs/nested_ts_suites_cut_inside_a_matcher",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    String::from_utf8(seed[1..].to_vec()).unwrap()
}

/// A change that adds the source the fuzzing run found is checked by a run that returns
/// with its report: every gate ran, and the file is reported as parsed with errors,
/// which is reported once its facts have been read.
#[test]
fn the_source_a_fuzzing_run_found_is_checked_and_reported() {
    let repo = Repo::new();
    repo.write("tests/m.ts", &the_found_source());
    repo.commit("test: add nested suites");
    let (exit, run) = check_within_limit(&repo);
    assert_eq!(exit, Some(1), "the run returns\n{}", run.stderr);
    assert!(run.json()["could_not_check"].is_null(), "{}", run.stderr);
    assert_eq!(
        reported(&run),
        vec![(
            "assertion-reduction/source-parsed-with-errors".to_string(),
            "tests/m.ts".to_string(),
            0
        )]
    );
}

/// One JavaScript test whose single `expect` statement holds [`LITERALS`] matchers, each
/// on its own line; the matcher at `moved` expects `value`.
fn matchers(moved: usize, value: usize) -> String {
    format!(
        "test('t', () => {{\n  expect([\n{}\n  ]);\n}});\n",
        row(
            LITERALS,
            |i| format!("    a{i}.toBe({})", if i == moved { value } else { i }),
            ",\n"
        )
    )
}

/// One Python test whose single `assert` holds [`LITERALS`] upper bounds, each on its
/// own line; the bound at `moved` is `value`.
fn bounds(moved: usize, value: usize) -> String {
    format!(
        "def test_t():\n    assert (\n{}\n    )\n",
        row(
            LITERALS,
            |i| format!("        a{i} < {}", if i == moved { value } else { i + 1 }),
            " and\n"
        )
    )
}

/// One Go test of [`LITERALS`] testify equalities, each the last argument of the one
/// before and each on its own line; the equality at `moved` expects `value`.
fn nested_equalities(moved: usize, value: usize) -> String {
    format!(
        "package p\n\nimport \"testing\"\n\nfunc TestT(t *testing.T) {{\n{}x{})\n}}\n",
        row(
            LITERALS,
            |i| format!("\tassert.Equal(t, {}, ", if i == moved { value } else { i }),
            "\n"
        ),
        ")".repeat(LITERALS - 1)
    )
}

/// The one literal a change moves among the [`LITERALS`] of a statement is reported, at
/// its line, and nothing else is: each record is still paired with its own on the other
/// side by the text of the statement around it.
#[test]
fn the_one_literal_a_change_moves_in_a_statement_of_many_is_reported() {
    type Source = fn(usize, usize) -> String;
    let cases: [(&str, Source, usize, &str, u64); 3] = [
        (
            "tests/m.test.js",
            matchers,
            9_999,
            "assertion-reduction/expected-value-changed",
            // `test(`, `expect([`, then one matcher for each line.
            3 + 150,
        ),
        (
            "tests/test_m.py",
            bounds,
            9_999,
            "assertion-reduction/assertion-bound-loosened",
            3 + 150,
        ),
        (
            "m_test.go",
            nested_equalities,
            9_999,
            "assertion-reduction/expected-value-changed",
            6 + 150,
        ),
    ];
    for (path, source, value, code, line) in cases {
        let repo = Repo::new();
        repo.commit_base(path, &source(LITERALS, 0), "test: add the test");
        repo.write(path, &source(150, value));
        repo.commit("test: move one literal");
        let (exit, run) = check_within_limit(&repo);
        assert_eq!(exit, Some(1), "{path}: the run returns\n{}", run.stderr);
        assert!(run.json()["could_not_check"].is_null(), "{}", run.stderr);
        assert_eq!(
            reported(&run),
            vec![(code.to_string(), path.to_string(), line)],
            "{path}"
        );
    }
}

/// The same statements with a comment added and no literal moved are no finding: every
/// record of the head is the base's.
#[test]
fn a_statement_of_many_literals_that_keeps_them_is_no_finding() {
    type Source = fn(usize, usize) -> String;
    let cases: [(&str, Source, &str); 3] = [
        ("tests/m.test.js", matchers, "// kept\n"),
        ("tests/test_m.py", bounds, "# kept\n"),
        ("m_test.go", nested_equalities, "// kept\n"),
    ];
    for (path, source, comment) in cases {
        let repo = Repo::new();
        let text = source(LITERALS, 0);
        repo.commit_base(path, &text, "test: add the test");
        repo.write(path, &format!("{text}{comment}"));
        repo.commit("test: add a comment");
        let (exit, run) = check_within_limit(&repo);
        assert!(exit.is_some(), "{path}: the run returns\n{}", run.stderr);
        assert!(run.json()["could_not_check"].is_null(), "{}", run.stderr);
        assert_eq!(reported(&run), Vec::new(), "{path}");
    }
}
