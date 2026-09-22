//! End-to-end tests for the adoption path: what a consumer meets when it first
//! records a baseline, ports an existing test floor, or writes its first
//! `version-lockstep` groups from the documentation.

mod common;
use common::*;

use std::collections::BTreeMap;

// ---- baseline: only blocking severities by default ---------------------------

/// One finding per severity, each from a gate whose severity is pinned in the
/// config so the fixture does not depend on built-in defaults:
/// - `time-estimates` at `error`      (docs/legacy.md)
/// - `suppression-delta` at `warning` (src/legacy.rs)
/// - `pii` at `note`                  (docs/contact.md, a LAN address)
const SEVERITY_CONFIG: &str = r#"
[meta]
version = 1
name = "adoption"

[gates.time-estimates]
severity = "error"

[gates.suppression-delta]
severity = "warning"

[gates.pii]
severity = "note"

# Under --whole-tree the harness's own AGENTS.md is an instruction-file finding; the
# fixture pins one finding per severity, so the gate is off here.
[gates.instruction-smuggling]
enabled = false
"#;

/// The debt on `main` with a clean working branch (the brownfield case, reached
/// by `--whole-tree`), or on the working branch itself (reached by the diff).
fn severity_fixture_with_debt_on(branch: &str) -> Repo {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", branch]);
    repo.write("discipline.toml", SEVERITY_CONFIG);
    repo.write("docs/legacy.md", "Ships in 3 weeks.\n");
    repo.write(
        "src/legacy.rs",
        "#[allow(dead_code)]\nfn unused() -> u8 {\n    1\n}\n",
    );
    // Assembled at runtime so this source file does not itself carry the address.
    repo.write(
        "docs/contact.md",
        &format!("host = {}\n", ["192", "168", "4", "7"].join(".")),
    );
    repo.commit("chore: pre-existing debt");
    if branch == "main" {
        repo.git(&["checkout", "-q", "-B", "work"]);
    }
    repo
}

fn severity_fixture() -> Repo {
    severity_fixture_with_debt_on("main")
}

/// Recorded baseline entries, counted per gate.
fn recorded_by_gate(repo: &Repo) -> BTreeMap<String, usize> {
    let text = std::fs::read_to_string(repo.file("discipline-baseline.toml"))
        .expect("baseline file written");
    let parsed: toml::Value = toml::from_str(&text).unwrap();
    let mut by_gate = BTreeMap::new();
    for f in parsed
        .get("findings")
        .and_then(|f| f.as_array())
        .cloned()
        .unwrap_or_default()
    {
        *by_gate
            .entry(f["gate"].as_str().unwrap().to_string())
            .or_insert(0) += 1;
    }
    by_gate
}

#[test]
fn baseline_fixture_carries_one_finding_per_severity() {
    // Guards the fixture itself: if a gate stops firing, the severity tests
    // below would pass vacuously.
    let repo = severity_fixture_with_debt_on("work");
    let run = repo.check(&[]);
    let json = run.json();
    let mut sev: BTreeMap<(String, String), usize> = BTreeMap::new();
    for o in json["outcomes"].as_array().unwrap() {
        for v in o["violations"].as_array().unwrap() {
            *sev.entry((
                o["gate"].as_str().unwrap().to_string(),
                v["severity"].as_str().unwrap().to_string(),
            ))
            .or_insert(0) += 1;
        }
    }
    assert_eq!(
        sev.get(&("time-estimates".into(), "error".into())),
        Some(&1),
        "{sev:?}"
    );
    assert_eq!(
        sev.get(&("suppression-delta".into(), "warning".into())),
        Some(&1),
        "{sev:?}"
    );
    assert_eq!(sev.get(&("pii".into(), "note".into())), Some(&1), "{sev:?}");
}

#[test]
fn baseline_whole_tree_records_only_blocking_findings_by_default() {
    let repo = severity_fixture();
    let run = repo.run(&["baseline", "--write", "--whole-tree"], &[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);

    let recorded = recorded_by_gate(&repo);
    assert_eq!(
        recorded,
        BTreeMap::from([("time-estimates".to_string(), 1)]),
        "a non-blocking warning or note buys nothing in a baseline:\n{}",
        run.stdout
    );

    // Nothing disappears silently: the skipped findings are counted by severity
    // and the flag that records them is named.
    assert!(
        run.stdout.contains("recorded: 1 error"),
        "missing recorded breakdown:\n{}",
        run.stdout
    );
    assert!(
        run.stdout
            .contains("skipped: 1 warning (suppression-delta: 1), 1 note (pii: 1)"),
        "missing skipped breakdown:\n{}",
        run.stdout
    );
    assert!(run.stdout.contains("--all-severities"), "{}", run.stdout);

    // The dry run reports the same split without writing.
    std::fs::remove_file(repo.file("discipline-baseline.toml")).unwrap();
    let dry = repo.run(&["baseline", "--whole-tree"], &[]);
    assert_eq!(dry.code, 0, "{}{}", dry.stdout, dry.stderr);
    assert!(!repo.file("discipline-baseline.toml").exists());
    assert!(
        dry.stdout
            .contains("Found 1 finding eligible for grandfathering"),
        "{}",
        dry.stdout
    );
    assert!(
        dry.stdout
            .contains("skipped: 1 warning (suppression-delta: 1), 1 note (pii: 1)"),
        "{}",
        dry.stdout
    );
}

#[test]
fn baseline_all_severities_records_warnings_and_notes() {
    let repo = severity_fixture();
    let run = repo.run(
        &["baseline", "--write", "--whole-tree", "--all-severities"],
        &[],
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        recorded_by_gate(&repo),
        BTreeMap::from([
            ("pii".to_string(), 1),
            ("suppression-delta".to_string(), 1),
            ("time-estimates".to_string(), 1),
        ]),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout.contains("recorded: 1 error, 1 warning, 1 note"),
        "{}",
        run.stdout
    );
    assert!(run.stdout.contains("skipped: none"), "{}", run.stdout);
}

#[test]
fn baseline_under_fail_on_warnings_records_warnings_but_not_notes() {
    // Under fail-on-warnings a warning blocks, so it is worth grandfathering;
    // a note never blocks. Both ways of turning it on are honoured: the flag,
    // and the environment variable the CI integrations set.
    for (args, env) in [
        (
            vec!["baseline", "--write", "--whole-tree", "--fail-on-warnings"],
            vec![],
        ),
        (
            vec!["baseline", "--write", "--whole-tree"],
            vec![("DISCIPLINE_FAIL_ON_WARNINGS", "true")],
        ),
    ] {
        let repo = severity_fixture();
        let run = repo.run(&args, &env);
        assert_eq!(
            run.code, 0,
            "{args:?} {env:?}\n{}{}",
            run.stdout, run.stderr
        );
        assert_eq!(
            recorded_by_gate(&repo),
            BTreeMap::from([
                ("suppression-delta".to_string(), 1),
                ("time-estimates".to_string(), 1),
            ]),
            "{args:?} {env:?}\n{}",
            run.stdout
        );
        assert!(
            run.stdout.contains("skipped: 1 note (pii: 1)"),
            "{args:?} {env:?}\n{}",
            run.stdout
        );
    }
}

#[test]
fn baseline_default_is_sufficient_for_check_to_pass_and_keeps_warnings_visible() {
    // The default baseline must still let `check` pass on the unchanged debt:
    // every skipped finding is non-blocking, and it stays reported rather than
    // hidden. Diff mode, so every gate in the fixture is in scope for `check`.
    let repo = severity_fixture_with_debt_on("work");
    let run = repo.run(&["baseline", "--write", "--base", "main"], &[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        recorded_by_gate(&repo),
        BTreeMap::from([("time-estimates".to_string(), 1)]),
        "{}",
        run.stdout
    );
    let check = repo.check_with_pr(
        &[],
        "allow-gate-weakening: baseline grandfathering pre-existing findings for adoption",
    );
    assert_eq!(check.code, 0, "{}{}", check.stdout, check.stderr);
    let json = check.json();
    assert_eq!(json["errors"], 0, "{json}");
    assert_eq!(json["baselined"], 1, "{json}");
    assert_eq!(
        check.violations("suppression-delta").len(),
        1,
        "the skipped warning stays visible"
    );
    assert_eq!(
        check.violations("pii").len(),
        1,
        "the skipped note stays visible"
    );
}

// ---- test-floor: runtime counting basis via test_command ---------------------

/// Stands in for `cargo test -- --list`: five `: test` lines across two
/// binaries, plus the lines a real listing interleaves that must not count.
const LISTING_SCRIPT: &str = "\
#!/bin/sh
echo 'unit::adds: test'
echo 'unit::orders: test'
echo 'unit::bench_insert: benchmark'
echo '3 tests, 1 benchmark'
echo 'integration::round_trip: test'
echo 'integration::rejects_empty: test'
echo 'src/lib.rs - read (line 3): test'
echo '3 tests, 0 benchmarks'
";

fn floor_config(min_tests: Option<usize>) -> String {
    let floor = min_tests
        .map(|n| format!("min_tests = {n}\n"))
        .unwrap_or_default();
    format!(
        r#"
[meta]
version = 1
name = "adoption"

[gates.test-floor]
enabled = true
test_command = "sh scripts/list-tests.sh"
{floor}"#
    )
}

#[test]
fn test_floor_ratchet_compares_the_test_command_count_not_the_static_count() {
    // The static count of this repository is 2 (tests/a.rs holds two #[test]
    // functions); the listing reports 5. Each floor below separates the bases.
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("scripts/list-tests.sh", LISTING_SCRIPT),
            ("discipline.toml", &floor_config(Some(5))),
        ],
        "chore: floor on the runtime listing",
    );

    // Floor 5 passes only on the runtime basis (static 2 < 5 would fire).
    let at_floor = repo.check(&[]);
    assert_eq!(at_floor.code, 0, "{}{}", at_floor.stdout, at_floor.stderr);
    let outcome = at_floor.outcome("test-floor");
    assert_eq!(outcome["examined"], 5, "{outcome}");
    assert!(at_floor.violations("test-floor").is_empty(), "{outcome}");

    // Floor 6 fires, and the message quotes the runtime count.
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("scripts/list-tests.sh", LISTING_SCRIPT),
            ("discipline.toml", &floor_config(Some(6))),
        ],
        "chore: floor above the runtime listing",
    );
    let below = repo.check(&[]);
    assert_eq!(below.code, 1, "{}{}", below.stdout, below.stderr);
    let v = below.violations("test-floor");
    assert_eq!(v.len(), 1, "{v:?}");
    assert_eq!(v[0]["title"], "Test Count Below Floor");
    assert!(
        v[0]["message"]
            .as_str()
            .unwrap()
            .contains("Workspace test count (5) is below the required floor of 6"),
        "{v:?}"
    );
}

#[test]
fn test_floor_test_command_without_an_explicit_floor_fails_closed() {
    // With no floor configured the ratchet would compare the runtime count at
    // HEAD against a STATIC count of the base ref: two different bases. That
    // comparison is refused rather than silently passing or failing.
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("scripts/list-tests.sh", LISTING_SCRIPT),
            ("discipline.toml", &floor_config(None)),
        ],
        "chore: runtime listing without a floor",
    );
    let run = repo.check(&[]);
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert!(
        run.stderr.contains("test_command") && run.stderr.contains("min_tests"),
        "the error must name the missing key:\n{}",
        run.stderr
    );
}

// ---- version-lockstep: the documented multi-ecosystem example ----------------

const GATES_MD: &str = include_str!("../docs/GATES.md");
const EXAMPLE_BEGIN: &str = "<!-- version-lockstep-example:begin -->";
const EXAMPLE_END: &str = "<!-- version-lockstep-example:end -->";

/// The `toml` block between the example markers in docs/GATES.md, verbatim.
fn documented_lockstep_config() -> String {
    let start = GATES_MD
        .find(EXAMPLE_BEGIN)
        .expect("docs/GATES.md is missing the version-lockstep example begin marker");
    let rest = &GATES_MD[start + EXAMPLE_BEGIN.len()..];
    let end = rest
        .find(EXAMPLE_END)
        .expect("docs/GATES.md is missing the version-lockstep example end marker");
    let block = &rest[..end];
    let body = block
        .split_once("```toml\n")
        .expect("example must be a ```toml fence")
        .1;
    let body = body
        .rsplit_once("```")
        .expect("example fence must be closed")
        .0;
    body.to_string()
}

/// Every file the documented example names, with the project version at `V`
/// and decoy versions (parent POM, dependencies, toolchain floors, split
/// version macros) that a loosely anchored regex would capture instead.
fn lockstep_files(v: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "Cargo.toml",
            format!(
                "[package]\nname = \"example-lib\"\nrust-version = \"1.75\"\nversion = \"{v}\"\nedition = \"2021\"\n\n[dependencies]\nserde = {{ version = \"1.0.200\" }}\n"
            ),
        ),
        (
            "package.json",
            format!(
                "{{\n  \"name\": \"example-lib\",\n  \"version\": \"{v}\",\n  \"dependencies\": {{\n    \"left-pad\": \"1.3.0\"\n  }},\n  \"engines\": {{ \"node\": \">=18\" }}\n}}\n"
            ),
        ),
        (
            "dotnet/ExampleLib.csproj",
            format!(
                "<Project Sdk=\"Microsoft.NET.Sdk\">\n  <ItemGroup>\n    <PackageReference Include=\"Newtonsoft.Json\" Version=\"13.0.3\" />\n  </ItemGroup>\n  <PropertyGroup>\n    <TargetFramework>net8.0</TargetFramework>\n    <Version>{v}</Version>\n  </PropertyGroup>\n</Project>\n"
            ),
        ),
        (
            "java/pom.xml",
            format!(
                "<project>\n  <modelVersion>4.0.0</modelVersion>\n  <parent>\n    <groupId>org.example</groupId>\n    <artifactId>example-parent</artifactId>\n    <version>7.0.0</version>\n  </parent>\n  <artifactId>example-lib</artifactId>\n  <version>{v}</version>\n  <dependencies>\n    <dependency>\n      <groupId>junit</groupId>\n      <artifactId>junit</artifactId>\n      <version>4.13.2</version>\n    </dependency>\n  </dependencies>\n</project>\n"
            ),
        ),
        (
            "ruby/example-lib.gemspec",
            format!(
                "Gem::Specification.new do |spec|\n  spec.name = \"example-lib\"\n  spec.required_ruby_version = \">= 3.0\"\n  spec.version = \"{v}\"\n  spec.add_dependency \"rake\", \"~> 13.0\"\nend\n"
            ),
        ),
        (
            "include/example_lib.h",
            format!(
                "#ifndef EXAMPLE_LIB_H\n#define EXAMPLE_LIB_H\n#define EXAMPLE_VERSION_MAJOR 1\n#define EXAMPLE_VERSION \"{v}\"\n#endif\n"
            ),
        ),
    ]
}

#[test]
fn version_lockstep_documented_example_passes_in_sync_and_names_each_drifted_file() {
    let config = format!(
        "[meta]\nversion = 1\nname = \"example-lib\"\n\n{}",
        documented_lockstep_config()
    );

    // (a) every ecosystem agrees: pass, and all six sources were read.
    let repo = Repo::new();
    let mut base: Vec<(&str, String)> = lockstep_files("1.4.2");
    base.push(("discipline.toml", config.clone()));
    let base_refs: Vec<(&str, &str)> = base.iter().map(|(p, c)| (*p, c.as_str())).collect();
    repo.commit_base_files(&base_refs, "chore: example-lib 1.4.2 across ecosystems");
    let ok = repo.check(&[]);
    assert_eq!(ok.code, 0, "{}{}", ok.stdout, ok.stderr);
    let outcome = ok.outcome("version-lockstep");
    assert_eq!(outcome["enabled"], true, "{outcome}");
    assert_eq!(outcome["examined"], 6, "{outcome}");

    // (b) drift each file in turn: the gate fires and names THAT file. Doing
    // it per file proves every documented regex reads the project's own
    // version rather than a decoy.
    for (drifted, _) in lockstep_files("1.4.2") {
        let repo = Repo::new();
        repo.commit_base_files(&base_refs, "chore: example-lib 1.4.2 across ecosystems");
        let content = lockstep_files("1.4.3")
            .into_iter()
            .find(|(p, _)| *p == drifted)
            .unwrap()
            .1;
        repo.write(drifted, &content);
        repo.commit(&format!("chore: bump {drifted} alone"));

        let run = repo.check(&[]);
        assert_eq!(run.code, 1, "{drifted}\n{}{}", run.stdout, run.stderr);
        let v = run.violations("version-lockstep");
        assert_eq!(v.len(), 1, "{drifted}: {v:?}");
        assert_eq!(v[0]["title"], "Version Declaration Lockstep Mismatch");
        assert_eq!(
            v[0]["file"], drifted,
            "the finding must point at the drifted file: {v:?}"
        );
        let msg = v[0]["message"].as_str().unwrap();
        assert!(
            msg.contains(&format!("`{drifted}` declares `1.4.3`")),
            "{drifted}: {msg}"
        );
    }
}
