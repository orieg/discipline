//! A file renamed out of test scope is production code from then on: its handlers are
//! judged like any other production file's, whatever path it had on the base side (#517).

mod common;
use common::Repo;

const HEAD: &str = common::CONFIG_HEAD;
const HELPER: &str = "import os\n\n\ndef load(path):\n    data = open(path).read()\n    lines = data.splitlines()\n    cleaned = [line.strip() for line in lines if line]\n    return cleaned\n\n\ndef size(path):\n    return os.path.getsize(path)\n";
const SWALLOW: &str =
    "\n\ndef remove(path):\n    try:\n        os.remove(path)\n    except OSError:\n        pass\n";

fn renamed_with_a_swallowed_error(config: &str, from: &str, to: &str) -> common::Run {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", config);
    repo.write(from, HELPER);
    repo.commit("chore: base");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.git(&["mv", from, to]);
    repo.write(to, &format!("{HELPER}{SWALLOW}"));
    repo.commit("refactor: move the helper");
    let status = repo.git_output(&["diff", "-M", "--name-status", "main", "HEAD"]);
    assert!(status.starts_with('R'), "fixture is not a rename: {status}");
    repo.check(&[])
}

#[test]
fn a_test_file_renamed_to_a_production_path_is_judged_as_production() {
    let run = renamed_with_a_swallowed_error(HEAD, "tests/helper.py", "src/helper.py");
    let found = run.violations("error-swallowing");
    assert_eq!(found.len(), 1, "{}", run.stdout);
    assert_eq!(found[0]["title"], "Empty Error Handler Added");
    assert_eq!(found[0]["file"], "src/helper.py");
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
}

/// The same through a declared test-scope glob rather than a conventional test path.
#[test]
fn a_file_renamed_out_of_a_declared_test_glob_is_judged_as_production() {
    let config = format!("{HEAD}[tests]\npaths = [\"qa/**\"]\n");
    let run = renamed_with_a_swallowed_error(&config, "qa/helper.py", "src/helper.py");
    let found = run.violations("error-swallowing");
    assert_eq!(found.len(), 1, "{}", run.stdout);
    assert_eq!(found[0]["file"], "src/helper.py");
}

/// Control: a rename that stays inside test scope is still test code.
#[test]
fn a_rename_inside_test_scope_stays_test_code() {
    let run = renamed_with_a_swallowed_error(HEAD, "tests/helper.py", "tests/support.py");
    assert!(
        run.violations("error-swallowing").is_empty(),
        "{}",
        run.stdout
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
}
