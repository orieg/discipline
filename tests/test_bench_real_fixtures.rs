//! The benchmark outputs under `tests/fixtures/bench/` that real tools wrote (see the
//! README there) are read by `bench-regression` as they are: Go's `go test -bench` text,
//! Google Benchmark's JSON with its `context` block, and pytest-benchmark's JSON with its
//! machine and commit information. The inline inputs of the other bench tests are short
//! hand-written documents; these are the formats as the tools emit them.

mod common;
use common::Repo;

const GO_BASE: &str = include_str!("fixtures/bench/go/base.txt");
const GO_HEAD: &str = include_str!("fixtures/bench/go/head.txt");
const GOOGLE_BASE: &str = include_str!("fixtures/bench/google/base.json");
const GOOGLE_HEAD: &str = include_str!("fixtures/bench/google/head.json");
const PYTEST_BASE: &str = include_str!("fixtures/bench/pytest/base.json");
const PYTEST_HEAD: &str = include_str!("fixtures/bench/pytest/head.json");

/// The exit code, and the title and message of each finding `bench-regression` reports for `head` at `path` over `base`.
fn bench_findings(path: &str, base: &str, head: &str) -> (i32, Vec<String>) {
    let repo = Repo::new();
    repo.commit_base(path, base, "bench: baseline");
    repo.write(path, head);
    let run = repo.check(&["--suite", "bench"]);
    assert!(
        run.code == 0 || run.code == 1,
        "{path}: {}{}",
        run.stdout,
        run.stderr
    );
    let findings = run
        .violations("bench-regression")
        .iter()
        .map(|v| format!("{}: {}", v["title"].as_str().unwrap(), v["message"]))
        .collect();
    (run.code, findings)
}

/// Each tool's two real runs compare without a finding, and the benchmark each file
/// names is the one the gate tracks: under another name in the head, the base's
/// benchmark is reported as removed, by the name the tool printed.
/// Killed mutant: a fixture's benchmark renamed in the base file only.
#[test]
fn real_go_google_and_pytest_outputs_are_read_under_the_names_the_tools_print() {
    for (path, base, head, name, other) in [
        (
            "benchmarks/go.txt",
            GO_BASE,
            GO_HEAD,
            "BenchmarkSearch",
            "BenchmarkLookup",
        ),
        (
            "build/benchmarks.json",
            GOOGLE_BASE,
            GOOGLE_HEAD,
            "BM_StringCreation",
            "BM_StringBuild",
        ),
        (
            "reports/pytest_bench.json",
            PYTEST_BASE,
            PYTEST_HEAD,
            "test_serialize",
            "test_encode",
        ),
    ] {
        assert!(base.contains(name) && head.contains(name), "{path}");
        assert_eq!(bench_findings(path, base, head), (0, Vec::new()), "{path}");
        // A warning at the default severity: the run still exits 0.
        let (_, findings) = bench_findings(path, base, &head.replace(name, other));
        let removed: Vec<&String> = findings
            .iter()
            .filter(|f| f.starts_with("Benchmark Removed: "))
            .collect();
        assert_eq!(removed.len(), 1, "{path}: {findings:?}");
        assert!(removed[0].contains(name), "{path}: {findings:?}");
    }
}
