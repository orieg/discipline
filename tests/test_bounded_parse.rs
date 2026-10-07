//! A parse has a budget (#640). The parser library's error recovery does not finish on
//! some texts: the Rust grammar stays at one position of
//! `fuzz/corpus/language_packs/timeout_rust_error_recovery` for as long as it is left to
//! run. A change that adds such a file gets an answer that names the file: exit 2 from
//! the gates that need every changed file's facts, and a note from each gate that reads
//! what it can. The twin of the file that the grammar does finish is the control.
//!
//! Every run here has a limit of its own, so a parse with no budget fails one test and
//! does not stop the suite. The limit is not the bound under test, which is counted in
//! parser steps and is the same on every machine.

mod common;
use common::{discipline_cmd, Repo, Run, CONFIG_HEAD};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// Far above what a run cut at its budget takes.
const LIMIT: Duration = Duration::from_secs(240);

/// The shortest text found that the Rust grammar does not finish, and the same text with
/// a character that starts a name where the first has one that starts no token: the
/// grammar reads the second to its end, with errors.
const UNFINISHED: &str = "(>\u{fffd}t(0(.t();}";
const FINISHED: &str = "(>\u{e9}t(0(.t();}";
const UNFINISHED_WHY: &str =
    "the parser did not finish within its budget of 259 steps for 15 bytes";

/// The gates that need the facts of every changed file and stop on one they cannot get.
const NEED_EVERY_FILE: &[&str] = &[
    "assertion-reduction",
    "vacuous-tests",
    "ignored-tests",
    "unsafe-safety-comment",
    "deletion-rationale",
];

/// The source of the fuzz seed: its first byte selects the pack in the fuzz target.
fn seed_source() -> String {
    let seed = std::fs::read(format!(
        "{}/fuzz/corpus/language_packs/timeout_rust_error_recovery",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    String::from_utf8_lossy(&seed[1..]).into_owned()
}

/// `discipline check --format json --base main`, killed at [`LIMIT`]. Its output goes to
/// files in the git directory, which no gate reads.
fn check_within_limit(repo: &Repo) -> Run {
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
        if started.elapsed() > LIMIT {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("`discipline check` did not return: a parse has no budget");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    Run {
        code: status.code().unwrap_or(-1),
        stdout: std::fs::read_to_string(&out).unwrap(),
        stderr: std::fs::read_to_string(&err).unwrap(),
    }
}

fn notes(run: &Run, gate: &str) -> Vec<String> {
    run.outcome(gate)["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_change_adding_the_fuzz_seed_is_refused_by_name() {
    let source = seed_source();
    assert_eq!(source.len(), 272);
    let repo = Repo::new();
    repo.write("src/stuck.rs", &source);
    repo.commit("feat: add a module");
    let run = check_within_limit(&repo);
    assert_eq!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    let (reason, gate) = run.could_not_check();
    assert_eq!(reason, "gate");
    assert_eq!(gate.as_deref(), Some("assertion-reduction"));
    let detail = run.json()["could_not_check"]["detail"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        detail,
        "gate `assertion-reduction` could not run: could not parse `src/stuck.rs`: \
         the parser did not finish within its budget of 324 steps for 272 bytes"
    );
    assert!(run.stderr.contains(&detail), "{}", run.stderr);
    // No gate reported an outcome, so nothing reads as a pass.
    assert_eq!(run.json()["outcomes"].as_array().unwrap().len(), 0);
}

/// The control: the same change with the twin the grammar finishes is checked, not
/// refused. The file has syntax errors, which the gates report in their own way.
#[test]
fn the_twin_the_grammar_finishes_is_checked() {
    let repo = Repo::new();
    repo.write("src/twin.rs", FINISHED);
    repo.commit("feat: add a module");
    let run = check_within_limit(&repo);
    assert_ne!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    assert!(run.json()["could_not_check"].is_null(), "{}", run.stdout);
    assert!(!run.stdout.contains("did not finish"), "{}", run.stdout);
    assert!(!run.json()["outcomes"].as_array().unwrap().is_empty());

    // And with the character that starts no token it is refused, so the two differ by
    // the parse alone.
    let repo = Repo::new();
    repo.write("src/twin.rs", UNFINISHED);
    repo.commit("feat: add a module");
    let run = check_within_limit(&repo);
    assert_eq!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    let detail = run.json()["could_not_check"]["detail"].to_string();
    assert!(
        detail.contains(&format!("could not parse `src/twin.rs`: {UNFINISHED_WHY}")),
        "{detail}"
    );
}

/// With the gates that stop on such a file switched off, every other gate that reads
/// source says it could not parse the file: none of them passes it in silence.
#[test]
fn each_gate_that_reads_what_it_can_notes_the_file() {
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
    repo.write("src/stuck.rs", UNFINISHED);
    repo.write("tests/prop.rs", UNFINISHED);
    repo.commit("feat: add a module");
    let run = check_within_limit(&repo);
    assert!(run.json()["could_not_check"].is_null(), "{}", run.stdout);

    let could_not = |path: &str| format!("could not parse `{path}`: {UNFINISHED_WHY}");
    for gate in ["stub-bodies", "error-swallowing", "suppression-delta"] {
        for path in ["src/stuck.rs", "tests/prop.rs"] {
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
            could_not("src/stuck.rs")
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
            could_not("src/stuck.rs")
        )),
        "{:?}",
        notes(&run, "pii")
    );
    let floor = notes(&run, "test-floor");
    assert!(
        floor.iter().any(|n| n
            .starts_with("head: 2 file(s) could not be read, or parse with errors")
            && n.contains("src/stuck.rs")
            && n.contains("tests/prop.rs")),
        "{floor:?}"
    );
}
