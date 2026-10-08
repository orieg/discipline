//! Findings land on the line of what they name, and an input that cannot be read is said
//! (#678): a `ci-integrity` finding is located by the job's own key and the step's own
//! lines, never by the first line that contains the name; `base-tests` reads the report
//! files a command wrote in name order; `archive-contents` stops on a directory it cannot
//! list; `dependency-delta` stops on a manifest that does not parse; and a pytest
//! `python_files` entry that is not a readable glob decides nothing. Each defect has a
//! test that failed before its fix and a control beside it.

mod common;
use common::{Repo, Run, CONFIG_HEAD};

const WF: &str = ".github/workflows/ci.yml";

fn detail(run: &Run) -> String {
    run.json()["could_not_check"]["detail"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn notes(run: &Run, gate: &str) -> String {
    run.outcome(gate)["notes"].to_string()
}

fn all(run: &Run) -> String {
    format!("{}\n{}", run.stdout, run.stderr)
}

/// `(code, line)` of each finding of a gate.
fn located(run: &Run, gate: &str) -> Vec<(String, Option<u64>)> {
    run.violations(gate)
        .iter()
        .map(|v| (v["code"].as_str().unwrap().to_string(), v["line"].as_u64()))
        .collect()
}

/// The 1-based line of the one line of `text` that is exactly `line`.
fn line_of(text: &str, line: &str) -> u64 {
    let hits: Vec<usize> = text
        .lines()
        .enumerate()
        .filter(|(_, l)| *l == line)
        .map(|(i, _)| i + 1)
        .collect();
    assert_eq!(hits.len(), 1, "`{line}` is not one line of:\n{text}");
    hits[0] as u64
}

/// A repository whose base holds `base` at `path` and whose change writes `head` there.
fn changed_file(path: &str, base: &str, head: &str, config: &str) -> Repo {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            (path, base),
            ("discipline.toml", &format!("{CONFIG_HEAD}{config}")),
        ],
        "ci: base",
    );
    repo.write(path, head);
    repo.commit("ci: change");
    repo
}

// ---- ci-integrity: the line of a job ---------------------------------------------

/// Two jobs, the first of which holds the id of the second in its own id and in its
/// `needs`. `first_line` is the line of the first job's key; `timeout` is the second
/// job's `timeout-minutes` line, or nothing.
fn two_jobs(first_line: &str, timeout: &str) -> String {
    format!(
        "name: ci\non: [push]\njobs:\n{first_line}\n    needs: [test]\n    runs-on: ubuntu-latest\n    timeout-minutes: 5\n    steps:\n      - run: cargo clippy\n  test:\n    runs-on: ubuntu-latest\n{timeout}    steps:\n      - run: cargo test\n"
    )
}

/// The finding for job `test` stood on the first line containing `test`: the key of the
/// job `test-lint`.
#[test]
fn a_job_finding_is_on_the_job_s_own_key_line() {
    let head = two_jobs("  test-lint:", "");
    let repo = changed_file(
        WF,
        &two_jobs("  test-lint:", "    timeout-minutes: 10\n"),
        &head,
        "",
    );
    let run = repo.check(&[]);
    assert_eq!(
        located(&run, "ci-integrity"),
        [(
            "ci-integrity/job-timeout-removed".to_string(),
            Some(line_of(&head, "  test:"))
        )],
        "{}",
        all(&run)
    );
}

/// An inline exemption excuses the line it is written on. Written on another job's key,
/// it excused the finding of the job whose id that key contains.
#[test]
fn an_exemption_on_another_job_s_line_does_not_excuse_a_job() {
    let first = "  test-lint: # discipline:allow(ci-integrity)";
    let repo = changed_file(
        WF,
        &two_jobs(first, "    timeout-minutes: 10\n"),
        &two_jobs(first, ""),
        "",
    );
    let run = repo.check(&[]);
    assert_eq!(run.code, 1, "{}", all(&run));
    assert_eq!(
        located(&run, "ci-integrity")
            .iter()
            .map(|(c, _)| c.as_str())
            .collect::<Vec<_>>(),
        ["ci-integrity/job-timeout-removed"],
        "{}",
        all(&run)
    );
    assert_eq!(run.outcome("ci-integrity")["inline_exemptions"], 0);
}

/// The same exemption on the job's own key line excuses it. Before the fix it did not: the
/// line read was the other job's.
#[test]
fn an_exemption_on_the_job_s_own_line_excuses_it() {
    let base = two_jobs("  test-lint:", "    timeout-minutes: 10\n");
    let head = two_jobs("  test-lint:", "")
        .replace("  test:\n", "  test: # discipline:allow(ci-integrity)\n");
    let repo = changed_file(WF, &base, &head, "");
    let run = repo.check(&[]);
    assert!(located(&run, "ci-integrity").is_empty(), "{}", all(&run));
    assert_eq!(run.outcome("ci-integrity")["inline_exemptions"], 1);
}

/// Control: where no other line holds the job's id, the finding is where it always was.
#[test]
fn a_job_finding_keeps_its_line_where_only_the_key_holds_the_name() {
    let wf = |timeout: &str| {
        format!(
            "name: ci\non: [push]\njobs:\n  lint:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo clippy\n  unit:\n    runs-on: ubuntu-latest\n{timeout}    steps:\n      - run: cargo test\n        continue-on-error: true\n"
        )
    };
    let head = wf("");
    let repo = changed_file(WF, &wf("    timeout-minutes: 10\n"), &head, "");
    let run = repo.check(&[]);
    assert_eq!(
        located(&run, "ci-integrity"),
        [(
            "ci-integrity/job-timeout-removed".to_string(),
            Some(line_of(&head, "  unit:"))
        )],
        "{}",
        all(&run)
    );
}

/// A job that newly ignores its failure, written after its steps, one of which has the
/// same key. The finding stood on the first such key after the job's line: the step's.
#[test]
fn a_job_level_key_is_not_a_step_s_key_of_the_same_name() {
    let wf = |job_key: &str| {
        format!(
            "name: ci\non: [push]\njobs:\n  unit:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo test\n        continue-on-error: false\n{job_key}"
        )
    };
    let head = wf("    continue-on-error: true\n");
    let repo = changed_file(WF, &wf(""), &head, "");
    let run = repo.check(&[]);
    assert_eq!(
        located(&run, "ci-integrity"),
        [(
            "ci-integrity/job-failure-masked-continue-on-error".to_string(),
            Some(line_of(&head, "    continue-on-error: true"))
        )],
        "{}",
        all(&run)
    );
}

/// A new step uses an action that an earlier step's script names in its text, in a job
/// that can write. The unpinned reference and the persisted credentials were both
/// reported on that line of the script.
#[test]
fn a_reference_finding_is_on_the_uses_line_of_its_own_step() {
    let wf = |step: &str| {
        format!(
            "name: ci\non: [push]\npermissions:\n  contents: write\njobs:\n  unit:\n    runs-on: ubuntu-latest\n    steps:\n      - run: |\n          echo start\n          echo next is actions/checkout@v4\n{step}      - run: cargo test\n"
        )
    };
    let head = wf("      - uses: actions/checkout@v4\n");
    let repo = changed_file(WF, &wf(""), &head, "");
    let run = repo.check(&[]);
    let uses_line = Some(line_of(&head, "      - uses: actions/checkout@v4"));
    let found = located(&run, "ci-integrity");
    for code in [
        "ci-integrity/unpinned-action",
        "ci-integrity/checkout-persists-credentials",
    ] {
        assert!(
            found.contains(&(code.to_string(), uses_line)),
            "{code}: {found:?}\n{}",
            all(&run)
        );
    }
}

/// The rollup job's finding stood on the first line containing its name.
#[test]
fn a_rollup_finding_is_on_the_rollup_job_s_own_key_line() {
    let wf = |needs: &str| {
        format!(
            "name: ci\non: [push]\njobs:\n  pre-gate:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo fmt --check\n  unit:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo test\n  gate:\n    needs: [{needs}]\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo ok\n"
        )
    };
    let head = wf("pre-gate");
    let repo = changed_file(
        WF,
        &wf("pre-gate, unit"),
        &head,
        "[gates.ci-integrity]\nrollup_job = \"gate\"\n",
    );
    let run = repo.check(&[]);
    let found = located(&run, "ci-integrity");
    assert!(!found.is_empty(), "{}", all(&run));
    for (code, line) in &found {
        assert_eq!(
            *line,
            Some(line_of(&head, "  gate:")),
            "{code}: {}",
            all(&run)
        );
    }
}

// ---- ci-integrity: the line of a step --------------------------------------------

/// The second job's second step runs a command that is also a line of an earlier step's
/// script, and a step between them already ignored its failure on the base side.
fn masked_step(mask: &str) -> String {
    format!(
        "name: ci\non: [push]\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - name: Build\n        run: |\n          cargo build\n          cargo test\n      - name: Flaky\n        run: ./flaky.sh\n        continue-on-error: true\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo start\n      - run: cargo test\n{mask}"
    )
}

/// A step was located at the first line of the file containing its script's first line,
/// and its `continue-on-error` at the next such key after that: here another step's.
#[test]
fn a_step_finding_is_on_a_line_of_the_step_itself() {
    let head = masked_step("        continue-on-error: true # the new one\n");
    let repo = changed_file(WF, &masked_step(""), &head, "");
    let run = repo.check(&[]);
    assert_eq!(
        located(&run, "ci-integrity"),
        [(
            "ci-integrity/step-failure-masked-continue-on-error".to_string(),
            Some(line_of(
                &head,
                "        continue-on-error: true # the new one"
            ))
        )],
        "{}",
        all(&run)
    );
}

/// Control: a step whose script is no other line of the file is reported where it was.
#[test]
fn a_step_finding_keeps_its_line_where_its_script_is_written_once() {
    let wf = |mask: &str| {
        format!(
            "name: ci\non: [push]\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo build\n  unit:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo test\n{mask}"
        )
    };
    let head = wf("        continue-on-error: true\n");
    let repo = changed_file(WF, &wf(""), &head, "");
    let run = repo.check(&[]);
    assert_eq!(
        located(&run, "ci-integrity"),
        [(
            "ci-integrity/step-failure-masked-continue-on-error".to_string(),
            Some(line_of(&head, "        continue-on-error: true"))
        )],
        "{}",
        all(&run)
    );
}

/// An exemption on a line of another step's script excused the whole step that runs the
/// same command.
#[test]
fn an_exemption_inside_another_step_s_script_does_not_excuse_a_step() {
    let with_comment = |mask: &str| {
        masked_step(mask).replace(
            "          cargo test\n",
            "          cargo test # discipline:allow(ci-integrity)\n",
        )
    };
    let repo = changed_file(
        WF,
        &with_comment(""),
        &with_comment("        continue-on-error: true\n"),
        "",
    );
    let run = repo.check(&[]);
    assert_eq!(
        located(&run, "ci-integrity")
            .iter()
            .map(|(c, _)| c.as_str())
            .collect::<Vec<_>>(),
        ["ci-integrity/step-failure-masked-continue-on-error"],
        "{}",
        all(&run)
    );
}

/// The exemption on the step's own line excuses it. Before the fix it did not: the line
/// read was a line of the other step's script.
#[test]
fn an_exemption_on_the_step_s_own_line_excuses_it() {
    let head = masked_step("        continue-on-error: true\n").replace(
        "      - run: cargo test\n",
        "      - run: cargo test # discipline:allow(ci-integrity)\n",
    );
    let repo = changed_file(WF, &masked_step(""), &head, "");
    let run = repo.check(&[]);
    assert!(located(&run, "ci-integrity").is_empty(), "{}", all(&run));
}

// ---- ci-integrity: a trigger, a GitLab job, a discipline pin ---------------------

/// The new trigger was located at the first line containing its name: a comment.
#[test]
fn a_trigger_finding_is_on_the_trigger_not_on_a_comment_that_names_it() {
    let wf = |on: &str| {
        format!(
            "# This workflow must never run on pull_request_target.\nname: ci\n{on}permissions: read-all\njobs:\n  unit:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo test\n"
        )
    };
    let head = wf("on:\n  push:\n  pull_request_target:\n");
    let repo = changed_file(WF, &wf("on:\n  push:\n"), &head, "");
    let run = repo.check(&[]);
    let found = located(&run, "ci-integrity");
    assert!(
        found.contains(&(
            "ci-integrity/pull-request-target-trigger".to_string(),
            Some(line_of(&head, "  pull_request_target:"))
        )),
        "{}",
        all(&run)
    );
}

/// A GitLab job is a top-level key; its finding stood on the first line containing
/// `<name>:`, here the job `unit-test`.
#[test]
fn a_gitlab_job_finding_is_on_the_job_s_own_key_line() {
    let ci = |extra: &str| {
        format!(
            "unit-test:\n  script:\n    - make unit\ntest:\n  script:\n    - make test\n{extra}"
        )
    };
    let head = ci("  allow_failure: true\n");
    let repo = changed_file(".gitlab-ci.yml", &ci(""), &head, "");
    let run = repo.check(&[]);
    let found = located(&run, "ci-integrity");
    assert_eq!(found.len(), 1, "{}", all(&run));
    assert_eq!(found[0].1, Some(line_of(&head, "test:")), "{}", all(&run));
}

/// Two steps run the discipline action. The second moves to a ref that the first one's
/// ref contains as text; its finding stood on the first step's line.
#[test]
fn a_discipline_pin_finding_is_on_the_line_holding_that_value() {
    let wf = |second: &str| {
        format!(
            "name: ci\non: [push]\njobs:\n  a:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: orieg/discipline@v0.17.2\n  b:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: orieg/discipline@{second}\n"
        )
    };
    let head = wf("v0.17");
    let repo = changed_file(
        WF,
        &wf("v0.17.2"),
        &head,
        "[gates.ci-integrity]\nfirst_party_action_prefixes = [\"orieg/\"]\n",
    );
    let run = repo.check(&[]);
    let found = located(&run, "ci-integrity");
    assert!(
        found.contains(&(
            "ci-integrity/discipline-version-changed".to_string(),
            Some(line_of(&head, "      - uses: orieg/discipline@v0.17"))
        )),
        "{}",
        all(&run)
    );
}

// ---- command: which report file `base-tests` reads -------------------------------

const BASE_TESTS: &str = "[gates.command]\npreset = \"base-tests\"\ncommand = \"sh out.sh\"\n";
const FAILED: &str = "<testsuite><testcase name=\"adds\" classname=\"a\"><failure message=\"boom\"/></testcase></testsuite>";
const PASSED: &str = "<testsuite><testcase name=\"adds\" classname=\"a\"/></testsuite>";

/// A script that prints nothing and writes `names` as report files in the order given,
/// the first in name order holding `first` and the others `rest`.
fn report_script(names: &[&str], first: &str, rest: &str) -> String {
    let lowest = names.iter().min().unwrap();
    names
        .iter()
        .map(|n| {
            let body = if n == lowest { first } else { rest };
            format!("printf '%s' '{body}' > {n}\n")
        })
        .collect()
}

fn base_tests_run(script: &str) -> Run {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("discipline.toml", &format!("{CONFIG_HEAD}{BASE_TESTS}")),
            ("out.sh", script),
        ],
        "ci: base configuration",
    );
    repo.write(
        "docs/plan.md",
        "# Plan\n\nPhase 1, Phase 2, then Phase 3.\n",
    );
    repo.commit("docs: extend the plan");
    repo.check(&[])
}

/// Sets of report file names, each written in two orders. A directory listing gives them
/// in an order the file system chooses, so before the fix at least one of these runs read
/// a report other than the first in name order.
const REPORT_SETS: &[&[&str]] = &[
    &["a.xml", "b.xml", "c.xml", "d.xml", "e.xml", "f.xml"],
    &["f.xml", "e.xml", "d.xml", "c.xml", "b.xml", "a.xml"],
    &[
        "junit.xml",
        "report.xml",
        "results.xml",
        "test.xml",
        "unit.xml",
    ],
    &[
        "unit.xml",
        "test.xml",
        "results.xml",
        "report.xml",
        "junit.xml",
    ],
    &[
        "r1.xml", "r2.xml", "r3.xml", "r4.xml", "r5.xml", "r6.xml", "r7.xml",
    ],
    &[
        "r7.xml", "r6.xml", "r5.xml", "r4.xml", "r3.xml", "r2.xml", "r1.xml",
    ],
];

/// With several report files and no report on stdout, the one read is the first in name
/// order that holds cases: here the only one with a failed case.
#[test]
fn base_tests_reads_the_first_report_file_in_name_order() {
    for names in REPORT_SETS {
        let run = base_tests_run(&report_script(names, FAILED, PASSED));
        assert_eq!(
            located(&run, "command")
                .iter()
                .map(|(c, _)| c.as_str())
                .collect::<Vec<_>>(),
            ["command/base-test-failed"],
            "{names:?}: {}",
            all(&run)
        );
    }
}

/// The same files with the passing report first in name order pass, whatever order they
/// were written in. This failed before the fix too: the order was the file system's.
#[test]
fn base_tests_does_not_read_a_later_report_file() {
    for names in REPORT_SETS {
        let run = base_tests_run(&report_script(names, PASSED, FAILED));
        assert!(
            located(&run, "command").is_empty(),
            "{names:?}: {}",
            all(&run)
        );
        assert!(
            notes(&run, "command").contains("1 base tests passed"),
            "{names:?}: {}",
            all(&run)
        );
    }
}

/// Control: one report file is read whatever it is called, and a report on stdout is
/// read before any file.
#[test]
fn base_tests_reads_a_single_report_file_and_prefers_the_output() {
    let run = base_tests_run(&report_script(&["only.xml"], FAILED, PASSED));
    assert_eq!(located(&run, "command").len(), 1, "{}", all(&run));
    let printed = format!("printf '%s' '{FAILED}' > a.xml\nprintf '%s\\n' '{PASSED}'\n");
    let run = base_tests_run(&printed);
    assert!(located(&run, "command").is_empty(), "{}", all(&run));
}

// ---- archive-contents: a directory that cannot be listed -------------------------

#[cfg(unix)]
mod unreadable_directory {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn tar_with(name: &str) -> Vec<u8> {
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        header[100..108].copy_from_slice(b"0000644\0");
        header[108..116].copy_from_slice(b"0000000\0");
        header[116..124].copy_from_slice(b"0000000\0");
        header[124..136].copy_from_slice(b"00000000000\0");
        header[136..148].copy_from_slice(b"00000000000\0");
        header[148..156].copy_from_slice(b"        ");
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let sum: u32 = header.iter().map(|b| u32::from(*b)).sum();
        header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        let mut out = header.to_vec();
        out.extend_from_slice(&[0u8; 1024]);
        out
    }

    /// A repository with one archive under `dist/`, a directory `locked` that cannot be
    /// listed, and `archive_path` set to `glob`. `None` when the process can list the
    /// directory anyway (it runs as a user permissions do not bind).
    fn repo_with_locked_directory(glob: &str) -> Option<Repo> {
        let repo = Repo::new();
        repo.write(
            "discipline.toml",
            &format!(
                "{CONFIG_HEAD}[gates.archive-contents]\nenabled = true\narchive_path = \"{glob}\"\nrequired_paths = [\"README\"]\n"
            ),
        );
        std::fs::create_dir_all(repo.file("dist")).unwrap();
        std::fs::write(repo.file("dist/pkg.tar"), tar_with("README")).unwrap();
        repo.commit("chore: policy and archive");
        std::fs::create_dir_all(repo.file("locked")).unwrap();
        std::fs::write(repo.file("locked/other.tar"), tar_with("README")).unwrap();
        std::fs::set_permissions(repo.file("locked"), std::fs::Permissions::from_mode(0o000))
            .unwrap();
        if std::fs::read_dir(repo.file("locked")).is_ok() {
            unlock(&repo);
            return None;
        }
        Some(repo)
    }

    fn unlock(repo: &Repo) {
        std::fs::set_permissions(repo.file("locked"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    }

    /// The pattern can match a file below the directory that cannot be listed: a second
    /// archive there would make the path ambiguous. The directory was skipped in silence.
    #[test]
    fn a_directory_that_cannot_be_listed_stops_the_run() {
        let Some(repo) = repo_with_locked_directory("**/*.tar") else {
            return;
        };
        let run = repo.check(&[]);
        unlock(&repo);
        assert_eq!(run.code, 2, "{}", all(&run));
        assert_eq!(
            run.could_not_check(),
            ("gate".to_string(), Some("archive-contents".to_string()))
        );
        let d = detail(&run);
        assert!(
            d.contains("`locked`") && d.contains("could not be listed"),
            "{d}"
        );
    }

    /// Control: a pattern that names another directory cannot match below the locked one,
    /// and the run reads the archive it names.
    #[test]
    fn a_directory_the_pattern_cannot_reach_does_not_stop_the_run() {
        let Some(repo) = repo_with_locked_directory("dist/*.tar") else {
            return;
        };
        let run = repo.check(&[]);
        unlock(&repo);
        assert_eq!(run.code, 0, "{}", all(&run));
        assert_eq!(run.outcome("archive-contents")["examined"], 1);
    }
}

// ---- dependency-delta: a manifest that does not parse ----------------------------

/// `(manifest, base text, head text that adds a dependency and does not parse, format)`.
const BROKEN_MANIFESTS: &[(&str, &str, &str, &str)] = &[
    (
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nSENTINEL_VALUE = \"*\"\n[dependencies\n",
        "TOML",
    ),
    (
        "pyproject.toml",
        "[project]\nname = \"t\"\nversion = \"0.1.0\"\ndependencies = []\n",
        "[project]\nname = \"t\"\nversion = \"0.1.0\"\ndependencies = [\"SENTINEL_VALUE\"\n",
        "TOML",
    ),
    (
        "package.json",
        "{\"name\": \"t\", \"dependencies\": {}}\n",
        "{\"name\": \"t\", \"dependencies\": {\"SENTINEL_VALUE\": \"*\"},}\n",
        "JSON",
    ),
    (
        "composer.json",
        "{\"name\": \"t/t\", \"require\": {}}\n",
        "{\"name\": \"t/t\", \"require\": {\"SENTINEL_VALUE/x\": \"*\"},}\n",
        "JSON",
    ),
];

/// A change that adds a wildcard dependency and breaks the manifest's syntax passed: a
/// manifest that does not parse was read as one with no dependencies.
#[test]
fn a_head_manifest_that_does_not_parse_stops_the_run() {
    for (path, base, head, format) in BROKEN_MANIFESTS {
        let repo = changed_file(path, base, head, "");
        let run = repo.check(&[]);
        assert_eq!(run.code, 2, "{path}: {}", all(&run));
        let (_, gate) = run.could_not_check();
        assert_eq!(gate.as_deref(), Some("dependency-delta"), "{path}");
        let d = detail(&run);
        assert!(
            d.contains(&format!("`{path}` does not parse as {format} (line "))
                && d.contains("its dependencies could not be read"),
            "{path}: {d}"
        );
        // Location only: a parser's message can quote the file.
        assert!(
            !all(&run).contains("SENTINEL_VALUE"),
            "{path}: {}",
            all(&run)
        );
    }
}

/// A base manifest that does not parse gives nothing to compare with; the change that
/// repairs it is not stopped, and the note says what was not compared.
#[test]
fn a_base_manifest_that_does_not_parse_is_a_note() {
    for (path, good, broken, format) in BROKEN_MANIFESTS {
        let repo = changed_file(path, broken, good, "");
        let run = repo.check(&[]);
        // Another gate may report the base side of the same file; this one stops nothing.
        assert!(
            run.json()["could_not_check"].is_null(),
            "{path}: {}",
            all(&run)
        );
        assert!(located(&run, "dependency-delta").is_empty(), "{path}");
        let n = notes(&run, "dependency-delta");
        assert!(
            n.contains(&format!("`{path}` does not parse as {format} (line "))
                && n.contains("on the base side"),
            "{path}: {n}"
        );
        assert!(
            !all(&run).contains("SENTINEL_VALUE"),
            "{path}: {}",
            all(&run)
        );
    }
}

/// Control: the same additions in manifests that parse are reported.
#[test]
fn a_manifest_that_parses_still_reports_a_wildcard_dependency() {
    for (path, base, head) in [
        (
            "Cargo.toml",
            BROKEN_MANIFESTS[0].1,
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nleftpad = \"*\"\n",
        ),
        (
            "package.json",
            BROKEN_MANIFESTS[2].1,
            "{\"name\": \"t\", \"dependencies\": {\"leftpad\": \"*\"}}\n",
        ),
    ] {
        let repo = changed_file(path, base, head, "");
        let run = repo.check(&[]);
        assert_eq!(run.code, 1, "{path}: {}", all(&run));
        assert!(!located(&run, "dependency-delta").is_empty(), "{path}");
    }
}

// ---- runner collection: a `python_files` entry that is not a readable glob -------

const PY_TEST: &str = "def test_a():\n    assert 1 + 1 == 2\n";

/// A repository whose pytest configuration sets `python_files` to `patterns` and whose
/// change adds a test file named `t[a.py`.
fn pytest_repo(patterns: &str) -> Repo {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            (
                "pytest.ini",
                &format!("[pytest]\npython_files = {patterns}\n"),
            ),
            ("checks/test_base.py", PY_TEST),
        ],
        "test: base",
    );
    repo.write("checks/t[a.py", PY_TEST);
    repo.commit("test: add");
    repo
}

/// `t[a` is not a glob the collection model can compile. It was matched as plain text
/// inside the file name, so the file was read as collected by pytest.
#[test]
fn an_unreadable_python_files_entry_leaves_collection_not_determined() {
    let run = pytest_repo("t[a").check(&[]);
    assert_eq!(run.code, 0, "{}", all(&run));
    let n = notes(&run, "test-floor");
    assert!(
        n.contains("head: runner collection unknown (a `python_files` entry of the pytest configuration is not a glob that can be read)")
            && n.contains("2 file(s) is counted"),
        "{n}"
    );
    // The entry itself is not quoted.
    assert!(!n.contains("t[a"), "{n}");
}

/// Control: with an entry that compiles and matches, the file is collected and no note
/// speaks of an undetermined collection.
#[test]
fn a_readable_python_files_entry_decides_collection() {
    let run = pytest_repo("t*.py").check(&[]);
    assert_eq!(run.code, 0, "{}", all(&run));
    let n = notes(&run, "test-floor");
    assert!(!n.contains("`python_files`"), "{n}");
}
