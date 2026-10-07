//! What the binary takes from the machine it runs on besides its arguments: the programs
//! it finds through `PATH`, and the git configuration the in-process git library reads.
//!
//! The diff, the base and every blob are read in process, so a different `git` first on
//! `PATH` changes nothing a gate reads. Two things are found through `PATH` by design:
//! the `git` of the CI base fetch (`src/gitctx.rs`), and the program a `command` gate
//! names (`src/guards/command.rs`). The fetch runs in the C locale, so the message the
//! binary quotes from it does not change language with the caller's shell.
//!
//! The in-process library reads the user's git configuration from `HOME` and
//! `XDG_CONFIG_HOME` only: `GIT_CONFIG_GLOBAL`, `GIT_CONFIG_SYSTEM` and
//! `GIT_CONFIG_NOSYSTEM` are settings of the `git` program. The harness points `HOME` at
//! a directory that does not exist and removes `XDG_CONFIG_HOME`, which isolates both.
//! The library's system file (`/etc/gitconfig`) has no variable: nothing a test can set
//! moves it.

mod common;
use common::{Repo, Run, CONFIG_HEAD};
use std::path::{Path, PathBuf};

const WEAKENED_TEST: &str = "#[test]\nfn adds() {\n    let x = 1;\n    assert_eq!(x + 1, 2);\n}\n\n#[test]\nfn orders() {\n    let x = 1;\n    assert!(x < 2);\n}\n";

/// A directory holding an executable `name` that runs `script` under `/bin/sh`.
fn program_dir(name: &str, script: &str) -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

/// `PATH` with `dir` first.
fn path_with_first(dir: &Path) -> String {
    format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

/// A stand-in `git` that records that it ran, in `marker`, and answers every command with
/// success and text no git prints.
fn recording_git(marker: &Path) -> tempfile::TempDir {
    program_dir(
        "git",
        &format!(
            "echo ran >> '{}'\necho 'not git output'\nexit 0",
            marker.display()
        ),
    )
}

/// The parts of a JSON report a gate decides.
fn verdict(run: &Run) -> (i32, serde_json::Value, serde_json::Value) {
    let report = run.json();
    (run.code, report["outcomes"].clone(), report["base"].clone())
}

/// A change that weakens a test, renames a file and leaves one edit staged: what the
/// diff, the rename detection and the index read.
fn changed_repo() -> Repo {
    let repo = Repo::new();
    repo.write("tests/a.rs", WEAKENED_TEST);
    repo.git(&["mv", "docs/plan.md", "docs/moved.md"]);
    repo.commit("test: weaken and move");
    repo.write("src/extra.rs", "pub fn extra() -> u8 {\n    1\n}\n");
    repo.git(&["add", "src/extra.rs"]);
    repo
}

/// Killed mutant: none in the product (it reads the repository in process). The control
/// is the marker: `the_ci_base_fetch_finds_git_through_path_and_runs_it_in_the_c_locale`
/// shows the same stand-in is found when the binary does start `git`.
#[cfg(unix)]
#[test]
fn a_different_git_first_on_path_does_not_change_what_the_gates_read() {
    let repo = changed_repo();
    let outside = tempfile::tempdir().unwrap();
    let marker = outside.path().join("git-ran");
    let fake = recording_git(&marker);
    let path = path_with_first(fake.path());
    for args in [
        vec!["check", "--format", "json", "--base", "main"],
        vec!["check", "--format", "json", "--staged"],
        vec!["doctor", "--local-only", "--format", "json"],
    ] {
        let plain = repo.run(&args, &[]);
        let with_fake = repo.run(&args, &[("PATH", path.as_str())]);
        if args[0] == "check" {
            assert_eq!(verdict(&with_fake), verdict(&plain), "{args:?}");
        } else {
            assert_eq!(
                (with_fake.code, &with_fake.stdout),
                (plain.code, &plain.stdout),
                "{args:?}"
            );
        }
    }
    // The change is seen at all: the weakened test is reported against `main`.
    let against_main = repo.run(
        &["check", "--format", "json", "--base", "main"],
        &[("PATH", path.as_str())],
    );
    assert_eq!(
        against_main.titles("assertion-reduction").len(),
        1,
        "{}",
        against_main.stdout
    );
    assert!(
        !marker.exists(),
        "the stand-in git was started: {:?}",
        std::fs::read_to_string(&marker)
    );
}

/// The one place the binary starts `git`: the base fetch of a CI run. It takes the first
/// `git` on `PATH`, and gives it `LC_ALL=C` and no `LANGUAGE`, whatever the caller's
/// locale, so the diagnostic it quotes reads the same everywhere.
/// Killed mutants: the `LC_ALL` set on the fetch in `src/gitctx.rs` removed, and the
/// `LANGUAGE` removal dropped.
#[cfg(unix)]
#[test]
fn the_ci_base_fetch_finds_git_through_path_and_runs_it_in_the_c_locale() {
    let repo = Repo::new();
    let fake = program_dir(
        "git",
        "echo \"fake git: LC_ALL=${LC_ALL-unset} LANGUAGE=${LANGUAGE-unset}\" >&2\nexit 1",
    );
    let path = path_with_first(fake.path());
    let fetch = |locale: &[(&str, &str)]| -> String {
        let mut env = vec![
            ("CI", "true"),
            // The stand-in is what runs: nothing leaves the machine.
            ("DISCIPLINE_NO_NETWORK", "0"),
            ("PATH", path.as_str()),
        ];
        env.extend_from_slice(locale);
        let run = repo.run(&["check", "--base", "origin/no-such-branch"], &env);
        assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
        run.stderr
    };
    for locale in [
        &[][..],
        &[("LC_ALL", "fr_FR.UTF-8"), ("LANGUAGE", "fr")][..],
        &[("LC_MESSAGES", "de_DE.UTF-8"), ("LANG", "de_DE.UTF-8")][..],
    ] {
        let said = fetch(locale);
        assert!(
            said.contains("fake git: LC_ALL=C LANGUAGE=unset"),
            "{locale:?}: {said}"
        );
    }
}

/// A `command` gate's program is found through `PATH`: by design, the gate runs the
/// project's own tool. With the tool's directory on `PATH` it runs; without it the run
/// could not check and says the tool is missing.
#[cfg(unix)]
#[test]
fn a_command_gate_program_is_found_through_path() {
    let repo = Repo::new();
    repo.commit_base(
        "discipline.toml",
        &format!("{CONFIG_HEAD}\n[gates.command]\ncommand = \"project_tool_for_path_test\"\n"),
        "ci: configure the command gate",
    );
    let tool = program_dir("project_tool_for_path_test", "touch tool-ran\nexit 0");
    let path = path_with_first(tool.path());

    let missing = repo.check(&[]);
    assert_eq!(
        missing.could_not_check(),
        ("tool-missing".to_string(), Some("command".to_string())),
        "{}",
        missing.stderr
    );
    assert!(!repo.file("tool-ran").exists());

    let found = repo.run(
        &["check", "--format", "json", "--base", "main"],
        &[("PATH", path.as_str())],
    );
    assert_eq!(found.code, 0, "{}{}", found.stdout, found.stderr);
    assert!(repo.file("tool-ran").exists(), "{}", found.stdout);
}

/// A home directory whose git configuration cannot be parsed, as `~/.gitconfig` and as
/// `git/config` under an XDG configuration directory. A reader that opens it fails.
fn home_with_unreadable_git_config() -> (tempfile::TempDir, PathBuf) {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join(".gitconfig");
    std::fs::write(&config, "broken [\n").unwrap();
    std::fs::create_dir_all(home.path().join("git")).unwrap();
    std::fs::write(home.path().join("git/config"), "broken [\n").unwrap();
    (home, config)
}

/// The in-process git library reads the user's configuration from `HOME` and
/// `XDG_CONFIG_HOME`, and a file it cannot parse there is a run that could not check
/// (exit 2), never a pass. It does not read `GIT_CONFIG_GLOBAL` or `GIT_CONFIG_SYSTEM`,
/// which only the `git` program honours.
#[test]
fn the_git_library_reads_user_configuration_from_home_and_xdg_only() {
    let repo = changed_repo();
    let (home, config) = home_with_unreadable_git_config();
    let dir = home.path().to_str().unwrap();
    let file = config.to_str().unwrap();
    let args = ["check", "--format", "json", "--base", "main"];
    let isolated = repo.run(&args, &[]);
    assert_eq!(isolated.code, 1, "{}{}", isolated.stdout, isolated.stderr);

    for name in ["HOME", "XDG_CONFIG_HOME"] {
        let run = repo.run(&args, &[(name, dir)]);
        assert_eq!(run.code, 2, "{name}: {}{}", run.stdout, run.stderr);
        assert!(
            run.stderr.contains("failed to parse config file"),
            "{name}: {}",
            run.stderr
        );
    }
    for name in ["GIT_CONFIG_GLOBAL", "GIT_CONFIG_SYSTEM"] {
        let run = repo.run(&args, &[(name, file)]);
        assert_eq!(verdict(&run), verdict(&isolated), "{name}: {}", run.stderr);
    }
}

/// User configuration that changes how `git` itself reports a change (`diff.renames`,
/// `core.quotepath`, line-ending conversion, an ignore file, file modes) does not change
/// what the gates read: the rename is still a rename, and the weakened test is reported.
#[test]
fn user_git_configuration_does_not_change_what_the_gates_read() {
    let repo = changed_repo();
    // A working-tree file with CRLF endings that drops an assertion, and a path that
    // `core.quotepath` would escape.
    repo.write(
        "tests/caf\u{e9}.rs",
        "#[test]\nfn accent() {\n    let x = 1;\n    assert_eq!(x + 1, 2);\n}\n",
    );
    repo.commit("test: a path outside ASCII");
    repo.write(
        "tests/a.rs",
        "#[test]\r\nfn adds() {\r\n    let x = 1;\r\n}\r\n\r\n#[test]\r\nfn orders() {\r\n    let x = 1;\r\n    assert!(x < 2);\r\n}\r\n",
    );
    let args = ["check", "--format", "json", "--base", "main"];
    let isolated = repo.run(&args, &[]);
    assert_eq!(isolated.code, 1, "{}{}", isolated.stdout, isolated.stderr);
    assert_eq!(
        isolated.titles("assertion-reduction").len(),
        1,
        "{}",
        isolated.stdout
    );
    let ignore = tempfile::tempdir().unwrap();
    std::fs::write(ignore.path().join("ignore"), "*.rs\n*.md\n").unwrap();
    let excludes = format!(
        "[core]\n\texcludesFile = {}\n",
        ignore.path().join("ignore").display()
    );
    for config in [
        "[diff]\n\trenames = false\n",
        "[core]\n\tquotepath = false\n",
        "[core]\n\tquotepath = true\n",
        "[core]\n\tautocrlf = true\n",
        "[core]\n\tautocrlf = input\n\tsafecrlf = true\n",
        "[core]\n\teol = crlf\n",
        "[core]\n\tfilemode = false\n\tignorecase = true\n\tsymlinks = false\n",
        "[core]\n\tprecomposeunicode = false\n",
        "[diff]\n\talgorithm = patience\n",
        "[safe]\n\tdirectory = /nonexistent/elsewhere\n",
        excludes.as_str(),
    ] {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(".gitconfig"), config).unwrap();
        let run = repo.run(&args, &[("HOME", home.path().to_str().unwrap())]);
        assert_eq!(
            verdict(&run),
            verdict(&isolated),
            "{config}: {}",
            run.stderr
        );
    }
}

/// One test at a time changes this process's environment.
static AMBIENT: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The harness keeps the parent's `HOME` and `XDG_CONFIG_HOME` from the binary, so the
/// in-process library reads no configuration of whoever runs the tests; and its `git`
/// runs with `GIT_CONFIG_NOSYSTEM=1`, so a system file is not read either.
/// Killed mutants, in `common::isolate_env`: `HOME` not set, `XDG_CONFIG_HOME` not
/// removed, and `GIT_CONFIG_NOSYSTEM` not set.
#[test]
fn the_harness_isolates_git_configuration_for_the_library_and_the_program() {
    let _one_at_a_time = AMBIENT.lock().unwrap_or_else(|e| e.into_inner());
    let repo = changed_repo();
    let (home, _) = home_with_unreadable_git_config();
    let args = ["check", "--format", "json", "--base", "main"];
    for name in ["HOME", "XDG_CONFIG_HOME"] {
        let before = std::env::var_os(name);
        std::env::set_var(name, home.path());
        let run = repo.run(&args, &[]);
        match before {
            Some(v) => std::env::set_var(name, v),
            None => std::env::remove_var(name),
        }
        assert_eq!(run.code, 1, "{name}: {}{}", run.stdout, run.stderr);
    }

    // The `git` program: a system file named by the test itself is still not read,
    // because the harness asks git to skip the system level.
    let system = tempfile::tempdir().unwrap();
    let file = system.path().join("gitconfig");
    std::fs::write(&file, "[user]\n\tname = from-the-system-file\n").unwrap();
    let out = common::git_command()
        .args(["config", "--get", "user.name"])
        .env("GIT_CONFIG_SYSTEM", &file)
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert_eq!(
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).trim()
        ),
        (Some(1), ""),
        "the harness git read a system configuration file"
    );
}

/// A script a test runs gets the same isolation as the binary: no token, CI marker, home
/// or temporary directory of the parent process.
/// Killed mutants: `isolate_env` not applied by `common::script_command`, and `TMPDIR`
/// taken out of the harness's list of removed variables.
#[cfg(unix)]
#[test]
fn a_script_spawn_inherits_no_token_marker_home_or_temporary_directory() {
    let _one_at_a_time = AMBIENT.lock().unwrap_or_else(|e| e.into_inner());
    // The directory this process already uses, named explicitly: other tests of this
    // file create their fixtures in it while this one runs.
    let tmp = std::env::temp_dir();
    let set = [
        ("GH_TOKEN", "parent-process-token"),
        ("CI", "true"),
        ("GITHUB_ACTIONS", "true"),
        ("TMPDIR", tmp.to_str().unwrap()),
        ("HOME", "/nonexistent/parent-home"),
    ];
    let before: Vec<_> = set.iter().map(|(k, _)| std::env::var_os(k)).collect();
    for (k, v) in set {
        std::env::set_var(k, v);
    }
    let out = common::script_command("sh")
        .args([
            "-c",
            "echo \"${GH_TOKEN-unset} ${CI-unset} ${GITHUB_ACTIONS-unset} ${TMPDIR-unset} $HOME\"",
        ])
        .output();
    for ((k, _), v) in set.iter().zip(before) {
        match v {
            Some(v) => std::env::set_var(k, v),
            None => std::env::remove_var(k),
        }
    }
    assert_eq!(
        String::from_utf8_lossy(&out.unwrap().stdout).trim(),
        format!("unset unset unset unset {}", common::NO_HOME)
    );
}
