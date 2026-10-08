//! The same input gives the same report (#675): no message, remediation, or order of
//! findings depends on the iteration order of a hash container. A hash container's order
//! is seeded per process, so each case drives the real binary several times in fresh
//! processes on a fixture with many items, compares the runs with each other, and pins
//! the sorted order.

mod common;
use common::{Repo, Run, CONFIG_HEAD};

/// Fresh processes per case. With twelve items in a hash set, two processes agree on the
/// order by chance about once in 12! tries, so a handful of runs is enough to see an
/// unstable order.
const RUNS: usize = 6;

/// Twelve job names, written here in sorted order.
const JOBS: [&str; 12] = [
    "audit", "bench", "build", "docs", "fmt", "fuzz", "lint", "miri", "msrv", "package", "test",
    "vet",
];

/// A workflow with every job of [`JOBS`] and, for each `(gate, needs)`, a job `gate`
/// that depends on `needs`.
fn workflow(gates: &[(&str, &[&str])]) -> String {
    let mut wf = String::from("name: CI\npermissions: read-all\njobs:\n");
    // Declared in an order that is not the sorted one.
    for job in JOBS.iter().rev() {
        wf.push_str(&format!(
            "  {job}:\n    runs-on: ubuntu-latest\n    timeout-minutes: 5\n    steps:\n      - run: true\n"
        ));
    }
    for (gate, needs) in gates {
        wf.push_str(&format!(
            "  {gate}:\n    needs: [{}]\n    runs-on: ubuntu-latest\n    timeout-minutes: 5\n    steps:\n      - run: true\n",
            needs.join(", ")
        ));
    }
    wf
}

/// One finding as a report shows it.
#[derive(Debug, Clone, PartialEq)]
struct Shown {
    file: String,
    message: String,
    remediation: String,
}

/// What a case reads from one run.
type Read<T> = fn(&Run) -> T;

/// What `read` gives in [`RUNS`] fresh processes of `discipline check`, after checking
/// that every run agrees with the first.
fn stable<T: PartialEq + std::fmt::Debug>(repo: &Repo, base: &str, read: Read<T>) -> T {
    let runs: Vec<T> = (0..RUNS)
        .map(|_| read(&repo.check(&["--base", base])))
        .collect();
    let differing = runs.iter().filter(|r| **r != runs[0]).count();
    assert_eq!(
        differing, 0,
        "{differing} of {RUNS} runs differ from the first:\n{runs:#?}"
    );
    runs.into_iter().next().unwrap()
}

/// The findings of `gate` with `code`, in report order.
fn findings(run: &Run, gate: &str, code: &str) -> Vec<Shown> {
    let text = |v: &serde_json::Value, key: &str| v[key].as_str().unwrap_or_default().to_string();
    run.violations(gate)
        .iter()
        .filter(|v| v["code"] == format!("{gate}/{code}"))
        .map(|v| Shown {
            file: text(v, "file"),
            message: text(v, "message"),
            remediation: text(v, "remediation"),
        })
        .collect()
}

#[test]
fn rollup_needs_incomplete_lists_the_missing_jobs_in_sorted_order() {
    let repo = Repo::new();
    repo.write(
        ".github/workflows/ci.yml",
        &workflow(&[("ci-gate", &["lint"])]),
    );
    repo.commit("ci: a rollup that depends on one job of twelve");

    let found = stable(&repo, "HEAD~1", |run| {
        findings(run, "ci-integrity", "rollup-needs-incomplete")
    });
    let missing: Vec<&str> = JOBS.iter().copied().filter(|j| *j != "lint").collect();
    let listed = format!("{missing:?}");
    assert_eq!(
        found,
        vec![Shown {
            file: ".github/workflows/ci.yml".to_string(),
            message: format!("Rollup job 'ci-gate' is missing dependencies on: {listed}"),
            remediation: format!(
                "Add the missing jobs to 'ci-gate' needs: {listed}, or excuse with allow-gate-weakening: ci-integrity <reason>."
            ),
        }]
    );
}

#[test]
fn rollup_needs_removed_findings_come_in_sorted_order() {
    let repo = Repo::new();
    // Three gate jobs, so the order of the jobs and the order of each job's dropped
    // dependencies both show.
    let all: &[&str] = &JOBS;
    repo.write(
        ".github/workflows/ci.yml",
        &workflow(&[
            ("ci-gate", all),
            ("deploy-gate", all),
            ("release-gate", all),
        ]),
    );
    repo.commit("ci: three gate jobs that depend on every job");
    repo.write(
        ".github/workflows/ci.yml",
        &workflow(&[
            ("ci-gate", &["lint"]),
            ("deploy-gate", &["lint"]),
            ("release-gate", &["lint"]),
        ]),
    );
    repo.commit("ci: the gate jobs keep one dependency");

    let found = stable(&repo, "HEAD~1", |run| {
        findings(run, "ci-integrity", "rollup-needs-removed")
    });
    let messages: Vec<String> = found.into_iter().map(|f| f.message).collect();
    let expected: Vec<String> = ["ci-gate", "deploy-gate", "release-gate"]
        .iter()
        .flat_map(|gate| {
            JOBS.iter().filter(|j| **j != "lint").map(move |job| {
                format!("Rollup job '{gate}' dropped dependency on '{job}' present in base.")
            })
        })
        .collect();
    assert_eq!(messages, expected);
}

/// A deleted workflow whose verification steps are in jobs the change added is a move,
/// and the note names the deleted jobs.
#[test]
fn the_note_of_a_moved_workflow_lists_its_jobs_in_sorted_order() {
    let repo = Repo::new();
    // The jobs move into a workflow the base side already has: a new file with the same
    // jobs would be read as a rename of the deleted one.
    let jobs = |prefix: &str, names: &[&str]| {
        let mut wf = String::from("name: CI\npermissions: read-all\njobs:\n");
        for job in names.iter().rev() {
            wf.push_str(&format!(
                "  {prefix}{job}:\n    runs-on: ubuntu-latest\n    timeout-minutes: 5\n    steps:\n      - run: cargo test --test {job}\n"
            ));
        }
        wf
    };
    repo.write(".github/workflows/old.yml", &jobs("test-", &JOBS));
    repo.write(".github/workflows/ci.yml", &jobs("suite-", &["site"]));
    repo.commit("ci: twelve verification jobs in a workflow of their own");
    repo.remove(".github/workflows/old.yml");
    let mut moved = vec!["site"];
    moved.extend(JOBS);
    repo.write(".github/workflows/ci.yml", &jobs("suite-", &moved));
    repo.commit("ci: the jobs move to the other workflow");

    let notes = stable(&repo, "HEAD~1", |run| {
        run.outcome("ci-integrity")["notes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n.as_str())
            .filter(|n| n.contains("treated as a move"))
            .map(str::to_string)
            .collect::<Vec<_>>()
    });
    let listed: Vec<String> = JOBS.iter().map(|j| format!("test-{j}")).collect();
    assert_eq!(notes.len(), 1, "{notes:#?}");
    assert!(
        notes[0].contains(&format!("jobs ({})", listed.join(", "))),
        "{}",
        notes[0]
    );
}

#[test]
fn fuzz_target_removed_findings_come_in_sorted_order() {
    let repo = Repo::new();
    let manifest = |targets: &[&str]| {
        let mut m = String::from("[package]\nname = \"fuzz\"\nversion = \"0.0.0\"\n");
        for target in targets.iter().rev() {
            m.push_str(&format!("\n[[bin]]\nname = \"{target}\"\n"));
        }
        m
    };
    repo.write("fuzz/Cargo.toml", &manifest(&JOBS));
    repo.commit("test: twelve fuzz targets");
    repo.write("fuzz/Cargo.toml", &manifest(&["lint"]));
    repo.commit("test: one fuzz target is left");

    let found = stable(&repo, "HEAD~1", |run| {
        findings(run, "test-budget", "fuzz-target-removed")
    });
    let messages: Vec<String> = found.into_iter().map(|f| f.message).collect();
    let expected: Vec<String> = JOBS
        .iter()
        .filter(|t| **t != "lint")
        .map(|t| {
            format!(
                "Fuzz target `{t}` was removed from fuzz harness `fuzz/Cargo.toml` without an explicit override."
            )
        })
        .collect();
    assert_eq!(messages, expected);
}

#[test]
fn seed_corpus_decreased_findings_come_in_sorted_order() {
    let repo = Repo::new();
    for dir in JOBS.iter().rev() {
        repo.write(&format!("corpus/{dir}/seed-1"), "one\n");
        repo.write(&format!("corpus/{dir}/seed-2"), "two\n");
    }
    repo.commit("test: twelve seed corpus directories");
    for dir in JOBS {
        repo.remove(&format!("corpus/{dir}/seed-1"));
    }
    repo.commit("test: each corpus loses a seed");

    let found = stable(&repo, "HEAD~1", |run| {
        findings(run, "test-budget", "seed-corpus-decreased")
    });
    let files: Vec<String> = found.into_iter().map(|f| f.file).collect();
    let expected: Vec<String> = JOBS.iter().map(|d| format!("corpus/{d}")).collect();
    assert_eq!(files, expected);
}

/// A module that several test targets declared, and none declares any more, is reported
/// at one of them: the same one on every run. The module walk takes the roots in path
/// order and the last of them first, so that one is named.
#[test]
fn a_module_several_test_targets_dropped_is_reported_at_the_same_one_each_run() {
    const TESTS_3: &str = "#[test]\nfn one() {\n    assert_eq!(1, 1);\n}\n\n#[test]\nfn two() {\n    assert_eq!(2, 2);\n}\n\n#[test]\nfn three() {\n    assert_eq!(3, 3);\n}\n";
    let target = |job: &str, with_mod: bool| {
        format!(
            "{}#[test]\nfn {job}_runs() {{\n    assert_eq!(1, 1);\n}}\n",
            if with_mod { "mod shared;\n\n" } else { "" }
        )
    };
    let repo = Repo::new();
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("tests/shared/mod.rs", TESTS_3);
    for job in JOBS {
        repo.write(&format!("tests/{job}.rs"), &target(job, true));
    }
    repo.commit("test: twelve targets that declare one module");
    for job in JOBS {
        repo.write(&format!("tests/{job}.rs"), &target(job, false));
    }
    // As many tests arrive as leave the run, so the count does not show the move.
    repo.write("tests/new.rs", TESTS_3);
    repo.commit("test: no target declares the module");

    let found = stable(&repo, "HEAD~1", |run| {
        findings(run, "test-floor", "tests-moved-out-of-default-run")
    });
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].file, "tests/vet.rs");
    assert!(
        found[0]
            .message
            .contains("The `mod` declaration no longer in `tests/vet.rs`"),
        "{}",
        found[0].message
    );
}

/// A file that is the root of a test target in two packages, one of which sets
/// `harness = false`, is described the same way on every run: as the target whose `main`
/// is the whole test run. Two packages give two orders only, so this case takes more
/// runs than the others to show an unstable order.
#[test]
fn a_test_target_of_two_packages_is_described_the_same_way_each_run() {
    let repo = Repo::new();
    repo.write(
        "discipline.toml",
        &format!("{CONFIG_HEAD}\n[gates.harness-tampering]\nenabled = true\n"),
    );
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"outer\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[test]]\nname = \"own\"\npath = \"inner/checks/own.rs\"\nharness = false\n",
    );
    repo.write(
        "inner/Cargo.toml",
        "[package]\nname = \"inner\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[test]]\nname = \"own\"\npath = \"checks/own.rs\"\n",
    );
    repo.commit("test: one file is a test target of two packages");
    repo.write(
        "inner/checks/own.rs",
        "fn main() {\n    std::process::exit(0);\n}\n",
    );
    repo.commit("test: the target's main exits with zero");

    let messages: Vec<Vec<String>> = (0..2 * RUNS)
        .map(|_| {
            findings(
                &repo.check(&["--base", "HEAD~1"]),
                "harness-tampering",
                "harness-exits-zero",
            )
            .into_iter()
            .map(|f| f.message)
            .collect()
        })
        .collect();
    for run in &messages {
        assert_eq!(run.len(), 1, "{messages:#?}");
        assert!(
            run[0].contains("the root of a Cargo test target with `harness = false`"),
            "{messages:#?}"
        );
    }
}
