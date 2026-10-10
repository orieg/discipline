mod common;
use common::CONFIG_HEAD;
use discipline::config::{split_list, DisciplineConfig, Overrides, Severity, GATES};
use std::path::Path;

#[test]
fn dogfood_config_loads_with_every_available_gate_on() {
    let config = DisciplineConfig::load_from_file(Path::new("discipline.toml"))
        .expect("discipline.toml must load under the strict schema");
    assert_eq!(config.meta.name, "discipline");
    for gate in GATES.iter().filter(|g| g.available) {
        if gate.id == "archive-contents"
            || gate.id == "manifest-sync"
            || gate.id == "version-lockstep"
            || gate.id == "scope-confinement"
            || gate.id == "pr-checklist"
            || gate.id == "unsafe-budget"
            || gate.id == "msrv"
            || gate.id == "miri"
            || gate.id == "sanitizers"
            // Opt-in, and new: enabling it here is a change to the protected `discipline.toml`.
            || gate.id == "harness-tampering"
            || gate.id == "gate-command-lint"
            || gate.id == "citation-anchors"
            || gate.id == "mechanism-sections"
        {
            continue;
        }
        let s = config
            .gates
            .settings(gate.id)
            .expect("available gate has settings");
        assert!(s.enabled(), "{} is off in the dogfood config", gate.id);
        assert_eq!(s.severity(), Severity::Error, "{} is not blocking", gate.id);
    }
}

#[test]
fn every_available_gate_has_settings_and_no_planned_gate_does() {
    let config = DisciplineConfig::default_for_repo("t");
    for gate in GATES {
        assert_eq!(
            config.gates.settings(gate.id).is_some(),
            gate.available,
            "{}",
            gate.id
        );
    }
}

#[test]
fn defaults_round_trip_through_toml() {
    let text = toml::to_string_pretty(&DisciplineConfig::default_for_repo("t")).unwrap();
    let back = DisciplineConfig::from_toml_str(&text).unwrap();
    assert!(back.gates.pii.home_paths);
    assert_eq!(back.gates.deletion_rationale.paths, vec!["**"]);
}

#[test]
fn schema_is_strict() {
    let head = CONFIG_HEAD;
    for bad in [
        "[gates.pii]\nlan_ipz = false\n",
        "[gates.no-such-gate]\nenabled = true\n",
        "[gates.fuzz-ratchet]\nenabled = true\n",
        "[gates.reproducible-builds]\nenabled = true\n",
        "[gates.formal-verification]\nenabled = true\n",
        "[nonsense]\na = 1\n",
        "[gates.pii]\nseverity = \"fatal\"\n",
        "[directives]\nunknown_key = true\n",
    ] {
        assert!(
            DisciplineConfig::from_toml_str(&format!("{head}{bad}")).is_err(),
            "accepted: {bad}"
        );
    }
    assert!(DisciplineConfig::from_toml_str("[meta]\nversion = 2\nname = \"t\"\n").is_err());
    assert!(DisciplineConfig::from_toml_str(head).is_ok());
}

#[test]
fn directives_config_defaults_and_overrides() {
    let base = DisciplineConfig::default_for_repo("t");
    assert_eq!(
        base.directives.sources,
        vec!["pr-body", "commits", "merged-pr-body"]
    );
    assert!(!base.directives.allow_hidden);
    assert!(!base.directives.fail_on_overrides);

    let toml = "[meta]\nversion = 1\nname = \"t\"\n[directives]\nsources = [\"pr-body\"]\nallow_hidden = true\nfail_on_overrides = true\n";
    let cfg = DisciplineConfig::from_toml_str(toml).unwrap();
    assert_eq!(cfg.directives.sources, vec!["pr-body"]);
    assert!(cfg.directives.allow_hidden);
    assert!(cfg.directives.fail_on_overrides);

    let overrides = Overrides {
        directive_sources: Some(vec!["commits".into()]),
        fail_on_overrides: Some(false),
        ..Default::default()
    };
    let c = DisciplineConfig::resolve(None, &overrides).unwrap();
    assert_eq!(c.directives.sources, vec!["commits"]);
    assert!(!c.directives.fail_on_overrides);
}

#[test]
fn override_layers_merge_tables_append_lists_and_replace_scalars() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("discipline.toml");
    std::fs::write(
        &path,
        "[meta]\nversion = 1\nname = \"t\"\n[gates.pii]\nhostname_denylist = [\"a\"]\nlan_ips = true\n",
    )
    .unwrap();
    let overrides = Overrides {
        config_override: Some("[gates.pii]\nhostname_denylist = [\"b\"]\nlan_ips = false".into()),
        enable: vec![],
        disable: vec!["time-estimates".into()],
        hostname_denylist: vec!["c".into(), "a".into()],
        ..Default::default()
    };
    let c = DisciplineConfig::resolve(Some(&path), &overrides).unwrap();
    assert_eq!(c.gates.pii.hostname_denylist, vec!["a", "b", "c"]);
    assert!(!c.gates.pii.lan_ips);
    assert!(c.gates.pii.home_paths, "untouched keys keep their defaults");
    assert!(!c.gates.time_estimates.enabled);
    assert!(c.gates.vacuous_tests.enabled);
}

#[test]
fn split_list_accepts_commas_spaces_and_newlines() {
    assert_eq!(split_list("a, b\nc  d,,"), vec!["a", "b", "c", "d"]);
    assert!(split_list(" \n").is_empty());
}

#[test]
fn test_asymmetric_list_merging_reset() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("discipline.toml");
    std::fs::write(
        &path,
        r#"[meta]
version = 1
name = "t"

[gates.assertion-reduction]
exempt_paths = ["tests/legacy/**"]
assert_helper_fns = ["helper1"]
extra_assert_macros = ["macro1"]

[gates.deletion-rationale]
paths = ["src/**"]

[gates.time-estimates]
include = ["**/*.md"]
extra_patterns = ["pattern1"]

[gates.pii]
hostname_denylist = ["host1.local"]
allow_patterns = ["allow1"]
allowed_users = ["user1"]
"#,
    )
    .unwrap();

    // 1. Shorter-is-stricter lists clear when __reset__ or { reset = true } is supplied.
    // 2. Longer-is-stricter lists ignore reset and stay append-only.
    let overrides = Overrides {
        config_override: Some(
            r#"[gates.assertion-reduction]
exempt_paths = ["__reset__", "tests/isolated/**"]
assert_helper_fns = ["__reset__"]
extra_assert_macros = ["__reset__", "macro2"]

[gates.deletion-rationale]
paths = ["__reset__", "docs/**"]

[gates.time-estimates]
include = ["__reset__", "**/*.txt"]
extra_patterns = ["__reset__", "pattern2"]

[gates.pii]
hostname_denylist = ["__reset__", "host2.local"]
allow_patterns = { reset = true, items = ["allow2"] }
allowed_users = ["__reset__", "alice"]

[directives]
sources = ["__reset__", "commits"]
"#
            .into(),
        ),
        ..Default::default()
    };

    let c = DisciplineConfig::resolve(Some(&path), &overrides).unwrap();

    // Loosening lists (shorter is stricter): reset honored
    assert_eq!(
        c.gates.assertion_reduction.exempt_paths,
        vec!["tests/isolated/**"]
    );
    assert_eq!(
        c.gates.assertion_reduction.assert_helper_fns,
        Vec::<String>::new()
    );
    assert_eq!(
        c.gates.assertion_reduction.extra_assert_macros,
        vec!["macro2"]
    );
    assert_eq!(c.gates.pii.allow_patterns, vec!["allow2"]);
    assert_eq!(c.gates.pii.allowed_users, vec!["alice"]);
    assert_eq!(c.directives.sources, vec!["commits"]);

    // Tightening lists (longer is stricter): reset ignored, strictly append-only
    assert_eq!(
        c.gates.pii.hostname_denylist,
        vec!["host1.local", "host2.local"]
    );
    assert_eq!(
        c.gates.time_estimates.extra_patterns,
        vec!["pattern1", "pattern2"]
    );
    assert_eq!(c.gates.deletion_rationale.paths, vec!["src/**", "docs/**"]);
    assert_eq!(c.gates.time_estimates.include, vec!["**/*.md", "**/*.txt"]);
}

#[test]
fn schema_does_not_drift() {
    let committed = std::fs::read_to_string("discipline.schema.json")
        .expect("discipline.schema.json must exist at repo root");
    let committed_val: serde_json::Value =
        serde_json::from_str(&committed).expect("discipline.schema.json must be valid JSON");
    let generated = discipline::schema::generate_schema();
    assert_eq!(
        committed_val, generated,
        "committed discipline.schema.json does not match discipline::schema::generate_schema(); run `discipline docs --write` to update"
    );

    // Assert that every available gate has a property in the schema, and no planned gate does
    let gates_props = generated["properties"]["gates"]["properties"]
        .as_object()
        .expect("gates properties must be an object");
    for g in GATES {
        assert_eq!(
            gates_props.contains_key(g.id),
            g.available,
            "gate {} availability mismatch in schema",
            g.id
        );
    }
}

#[test]
fn test_init_starter_template_is_valid() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("discipline.toml");
    let starter = r#"# discipline.toml — configuration for Discipline CI gatekeeper.
#
# Schema version 1. By default, every available gate is enabled at severity = "error".
# You only need to specify settings that differ from the defaults.
# Run `discipline gates` to view the effective status of all gates.

[meta]
version = 1
name = "my-test-proj"
# description = "Brief description of the project"

# [directives]
# sources = ["pr-body", "commits", "merged-pr-body"]
# allow_hidden = false
# fail_on_overrides = false

# Gate customizations (examples):
# [gates.assertion-reduction]
# severity = "error"
# exempt_paths = ["tests/legacy/**"]

# [gates.pii]
# allowed_users = ["runner", "user", "username"]
# hostname_denylist = ["internal.corp"]

# [gates.time-estimates]
# allow_patterns = ['^timeout: \d+']
"#;
    std::fs::write(&path, starter).unwrap();
    let cfg = DisciplineConfig::load_from_file(&path).unwrap();
    assert_eq!(cfg.meta.name, "my-test-proj");
    assert_eq!(cfg.meta.version, 1);
    for g in GATES.iter().filter(|g| g.available) {
        if g.id == "issue-link"
            || g.id == "provenance-tags"
            || g.id == "commit-provenance"
            || g.id == "archive-contents"
            || g.id == "manifest-sync"
            || g.id == "version-lockstep"
            || g.id == "scope-confinement"
            || g.id == "harness-tampering"
            || g.id == "ratified-paths"
            || g.id == "review-threads"
            || g.id == "pr-checklist"
            || g.id == "unsafe-budget"
            || g.id == "msrv"
            || g.id == "miri"
            || g.id == "sanitizers"
            || g.id == "gate-command-lint"
            || g.id == "citation-anchors"
            || g.id == "mechanism-sections"
        {
            assert!(
                !cfg.gates.settings(g.id).unwrap().enabled(),
                "{} should be opt-in (disabled by default)",
                g.id
            );
        } else {
            assert!(
                cfg.gates.settings(g.id).unwrap().enabled(),
                "{} should be enabled by default",
                g.id
            );
        }
    }
}

#[test]
fn test_init_starter_detects_runners_and_generates_valid_config() {
    use discipline::init::{generate_starter, TestRunner};

    // 1. Rust runner detection and snippet
    let rust_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        rust_dir.path().join("Cargo.toml"),
        "[package]\nname = \"demo\"\n",
    )
    .unwrap();
    let runner = TestRunner::detect(rust_dir.path()).unwrap();
    assert_eq!(runner, TestRunner::Cargo);
    let starter_rust = generate_starter("demo-rust", rust_dir.path());
    assert!(starter_rust.contains("cargo nextest run --profile ci"));
    assert!(starter_rust.contains("target/nextest/ci/junit.xml"));
    assert!(starter_rust.contains("preset = \"cargo-mutants\""));
    let cfg = DisciplineConfig::from_toml_str(&starter_rust).unwrap();
    assert_eq!(cfg.meta.name, "demo-rust");

    // 2. Python runner detection and snippet
    let py_dir = tempfile::tempdir().unwrap();
    std::fs::write(py_dir.path().join("pytest.ini"), "[pytest]\n").unwrap();
    let runner = TestRunner::detect(py_dir.path()).unwrap();
    assert_eq!(runner, TestRunner::Pytest);
    let starter_py = generate_starter("demo-py", py_dir.path());
    assert!(starter_py.contains("pytest --junitxml=reports/junit.xml"));
    assert!(starter_py.contains("reports/junit.xml"));
    assert!(starter_py.contains("preset = \"mutmut\""));
    let cfg_py = DisciplineConfig::from_toml_str(&starter_py).unwrap();
    assert_eq!(cfg_py.meta.name, "demo-py");

    // 3. Go runner detection and snippet (no diff-scoped mutation preset)
    let go_dir = tempfile::tempdir().unwrap();
    std::fs::write(go_dir.path().join("go.mod"), "module demo\n").unwrap();
    let runner = TestRunner::detect(go_dir.path()).unwrap();
    assert_eq!(runner, TestRunner::Go);
    let starter_go = generate_starter("demo-go", go_dir.path());
    assert!(starter_go.contains("gotestsum --junitfile reports/junit.xml"));
    assert!(!starter_go.contains("preset ="));

    // 4. JS/TS Vitest and Jest runner detection
    let vitest_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        vitest_dir.path().join("vitest.config.ts"),
        "export default {};\n",
    )
    .unwrap();
    let runner = TestRunner::detect(vitest_dir.path()).unwrap();
    assert_eq!(runner, TestRunner::Vitest);
    let starter_vitest = generate_starter("demo-vitest", vitest_dir.path());
    assert!(starter_vitest.contains("vitest run --reporter=junit"));
    assert!(starter_vitest.contains("preset = \"stryker\""));

    let jest_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        jest_dir.path().join("jest.config.js"),
        "module.exports = {};\n",
    )
    .unwrap();
    let runner = TestRunner::detect(jest_dir.path()).unwrap();
    assert_eq!(runner, TestRunner::Jest);
    let starter_jest = generate_starter("demo-jest", jest_dir.path());
    assert!(starter_jest.contains("npm test -- --reporters=jest-junit"));
    assert!(starter_jest.contains("preset = \"stryker\""));

    // 5. Maven and Gradle runner detection
    let mvn_dir = tempfile::tempdir().unwrap();
    std::fs::write(mvn_dir.path().join("pom.xml"), "<project></project>\n").unwrap();
    assert_eq!(
        TestRunner::detect(mvn_dir.path()).unwrap(),
        TestRunner::Maven
    );
    let starter_mvn = generate_starter("demo-mvn", mvn_dir.path());
    assert!(starter_mvn.contains("preset = \"pit\""));

    let gradle_dir = tempfile::tempdir().unwrap();
    std::fs::write(gradle_dir.path().join("build.gradle"), "// gradle\n").unwrap();
    assert_eq!(
        TestRunner::detect(gradle_dir.path()).unwrap(),
        TestRunner::Gradle
    );
    let starter_gradle = generate_starter("demo-gradle", gradle_dir.path());
    assert!(starter_gradle.contains("preset = \"pit\""));

    // 6. Uncommenting the test-floor block produces a valid test_command, test_report and min_tests in config
    let uncommented = starter_py
        .replace("# [gates.test-floor]", "[gates.test-floor]")
        .replace("# test_command =", "test_command =")
        .replace("# test_report =", "test_report =")
        .replace("# min_tests =", "min_tests =");
    let cfg_uncommented = DisciplineConfig::from_toml_str(&uncommented).unwrap();
    let tf = cfg_uncommented.gates.test_floor;
    assert_eq!(
        tf.test_command.as_deref(),
        Some("pytest --junitxml=reports/junit.xml")
    );
    assert_eq!(tf.test_report.as_deref(), Some("reports/junit.xml"));
    assert_eq!(tf.min_tests, Some(1));

    // 7. Uncommenting the mutation preset block produces valid [[gates.command.commands]]
    let uncommented_mut = starter_rust
        .replace("# [[gates.command.commands]]", "[[gates.command.commands]]")
        .replace("# name = \"mutation\"", "name = \"mutation\"")
        .replace("# preset = \"cargo-mutants\"", "preset = \"cargo-mutants\"");
    let cfg_mut = DisciplineConfig::from_toml_str(&uncommented_mut).unwrap();
    assert_eq!(cfg_mut.gates.command.commands.len(), 1);
    assert_eq!(cfg_mut.gates.command.commands[0].name, "mutation");
    assert_eq!(
        cfg_mut.gates.command.commands[0].preset.as_deref(),
        Some("cargo-mutants")
    );
}

#[test]
fn test_empty_test_report_env_vars_parse_without_error() {
    use clap::Parser;
    use discipline::cli::Cli;

    // Test reports env vars can be empty strings in CI (e.g. from GitHub Action inputs defaulting to '')
    std::env::set_var("DISCIPLINE_TEST_BASE_REPORT", "");
    std::env::set_var("DISCIPLINE_TEST_HEAD_REPORT", "");
    std::env::set_var("DISCIPLINE_TEST_REPORT", "");

    let cli = Cli::try_parse_from(["discipline", "check", "--base", "HEAD"])
        .expect("empty test report env vars must not trigger clap required value errors");

    if let discipline::cli::Commands::Check(args) = cli.command {
        assert_eq!(args.test_base_report, Some(std::path::PathBuf::from("")));
        assert_eq!(args.test_head_report, Some(std::path::PathBuf::from("")));
        assert_eq!(args.test_report, Some(std::path::PathBuf::from("")));
    } else {
        panic!("expected check command");
    }

    std::env::remove_var("DISCIPLINE_TEST_BASE_REPORT");
    std::env::remove_var("DISCIPLINE_TEST_HEAD_REPORT");
    std::env::remove_var("DISCIPLINE_TEST_REPORT");
}

#[test]
fn toml_deserialization_errors_include_spans() {
    // 1. Syntax error with line and column span
    let bad_syntax = "[meta]\nversion = 1\nname = \"test\n";
    let err = DisciplineConfig::from_toml_str(bad_syntax).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("at line") || msg.contains("line"),
        "error message should contain line info: {msg}"
    );

    // 2. Schema validation error with line and column span
    let bad_field =
        "[meta]\nversion = 1\nname = \"test\"\n\n[gates.pii]\nunknown_property = true\n";
    let err = DisciplineConfig::from_toml_str(bad_field).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("at line 6, column 1") || msg.contains("line 6"),
        "schema error should report exact line 6: {msg}"
    );

    // 3. File loading schema validation error with file path, line, and column span
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("discipline.toml");
    std::fs::write(&path, bad_field).unwrap();
    let err = DisciplineConfig::load_from_file(&path).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains(&format!("{}:6:1", path.display())) || msg.contains("line 6"),
        "file error should report file path and line 6: {msg}"
    );
}

#[test]
fn schema_json_def_references_resolve() {
    let generated = discipline::schema::generate_schema();
    let defs = generated["$defs"]
        .as_object()
        .expect("$defs must be an object");
    let gates_props = generated["properties"]["gates"]["properties"]
        .as_object()
        .expect("gates properties must be an object");

    for (gate_id, gate_val) in gates_props {
        if let Some(all_of) = gate_val["allOf"].as_array() {
            for item in all_of {
                if let Some(ref_str) = item["$ref"].as_str() {
                    let def_name = ref_str.strip_prefix("#/$defs/").expect("must be #/$defs/");
                    assert!(
                        defs.contains_key(def_name),
                        "gate {gate_id} references missing def {def_name}"
                    );
                }
            }
        }
    }
}

#[test]
fn test_all_directives_documented_in_configuration_md() {
    let doc =
        std::fs::read_to_string("docs/CONFIGURATION.md").expect("docs/CONFIGURATION.md must exist");

    let mut table_directives = std::collections::BTreeSet::new();
    let mut in_table = false;
    for line in doc.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("| Directive | Lifts | Subject |") {
            in_table = true;
            continue;
        }
        if in_table {
            if !trimmed.starts_with('|') || trimmed.is_empty() {
                break;
            }
            if trimmed.starts_with("|---|") {
                continue;
            }
            let parts: Vec<&str> = trimmed.split('|').collect();
            if parts.len() >= 2 {
                let cell = parts[1].trim();
                for entry in cell.split('/') {
                    let cleaned = entry.trim().trim_matches('`').trim_end_matches(':').trim();
                    if !cleaned.is_empty() {
                        table_directives.insert(cleaned.to_string());
                    }
                }
            }
        }
    }

    let code_directives: std::collections::BTreeSet<String> =
        discipline::tokens::ALL_DIRECTIVE_NAMES
            .iter()
            .map(|s| s.to_string())
            .collect();

    // 1. Every code directive must be in the documentation table
    for dir in &code_directives {
        assert!(
            table_directives.contains(dir),
            "Directive '{dir}' known to src/tokens.rs is missing from the directive table in docs/CONFIGURATION.md"
        );
    }

    // 2. Every documented table directive must be known to src/tokens.rs
    for dir in &table_directives {
        assert!(
            code_directives.contains(dir),
            "Directive '{dir}' documented in docs/CONFIGURATION.md table is unknown to src/tokens.rs"
        );
    }

    // 3. Exact set equality
    assert_eq!(
        table_directives, code_directives,
        "Mismatch between documented directive table and src/tokens.rs::ALL_DIRECTIVE_NAMES"
    );
}

#[test]
fn config_meta_mode_round_trips_and_defaults_to_enforcing() {
    let default_cfg = DisciplineConfig::default_for_repo("test-repo");
    assert_eq!(
        default_cfg.meta.mode,
        discipline::config::RunMode::Enforcing
    );

    let toml_advisory = r#"
[meta]
version = 1
name = "test-repo"
mode = "advisory"
"#;
    let cfg = DisciplineConfig::from_toml_str(toml_advisory).expect("advisory mode must parse");
    assert_eq!(cfg.meta.mode, discipline::config::RunMode::Advisory);

    let serialized = toml::to_string_pretty(&cfg).unwrap();
    let back = DisciplineConfig::from_toml_str(&serialized).unwrap();
    assert_eq!(back.meta.mode, discipline::config::RunMode::Advisory);
}

/// Compatibility contract (docs/ARCHITECTURE.md §3.1): the built-in default
/// enablement and severity of every gate. Changing a default fails this test
/// until the snapshot is updated deliberately, so the change is visible in
/// review. Within a major version a default may only become STRICTER without a
/// release-note entry in docs/ROADMAP.md ("Default Changes").
const DEFAULTS_SNAPSHOT: &[(&str, bool, Severity)] = &[
    ("agents-md", true, Severity::Warning),
    ("assertion-reduction", true, Severity::Error),
    ("vacuous-tests", true, Severity::Error),
    ("ignored-tests", true, Severity::Error),
    ("unsafe-safety-comment", true, Severity::Error),
    ("deletion-rationale", true, Severity::Error),
    ("scope-confinement", false, Severity::Error),
    ("harness-tampering", false, Severity::Error),
    ("suppression-delta", true, Severity::Warning),
    ("time-estimates", true, Severity::Warning),
    ("pii", true, Severity::Error),
    ("agent-scratch", true, Severity::Error),
    ("shell-secrets", true, Severity::Error),
    ("issue-link", false, Severity::Error),
    ("ratified-paths", false, Severity::Error),
    ("review-threads", false, Severity::Error),
    ("commit-provenance", false, Severity::Error),
    ("citation-metadata", true, Severity::Error),
    ("provenance-tags", false, Severity::Error),
    ("pr-checklist", false, Severity::Error),
    ("config-integrity", true, Severity::Error),
    ("sandbox-config", true, Severity::Error),
    ("toolchain-config", true, Severity::Error),
    ("stub-bodies", true, Severity::Error),
    ("error-swallowing", true, Severity::Error),
    ("instruction-smuggling", true, Severity::Error),
    ("build-hooks", true, Severity::Error),
    ("golden-output", true, Severity::Error),
    ("dependency-delta", true, Severity::Error),
    ("test-budget", true, Severity::Error),
    ("ci-integrity", true, Severity::Error),
    ("ci-skip-set", true, Severity::Error),
    ("test-floor", true, Severity::Error),
    ("archive-contents", false, Severity::Error),
    ("manifest-sync", false, Severity::Error),
    ("version-lockstep", false, Severity::Error),
    ("command", true, Severity::Error),
    ("sanitizers", false, Severity::Error),
    ("miri", false, Severity::Error),
    ("unsafe-budget", false, Severity::Error),
    ("msrv", false, Severity::Error),
    ("bench-regression", true, Severity::Warning),
    ("gate-command-lint", false, Severity::Error),
    ("citation-anchors", false, Severity::Error),
    ("mechanism-sections", false, Severity::Error),
];

#[test]
fn default_enablement_and_severity_match_snapshot() {
    let defaults = DisciplineConfig::default_for_repo("t");
    let available: Vec<&str> = GATES.iter().filter(|g| g.available).map(|g| g.id).collect();
    for id in &available {
        assert!(
            DEFAULTS_SNAPSHOT.iter().any(|(s, _, _)| s == id),
            "{id} has no entry in DEFAULTS_SNAPSHOT; add it deliberately"
        );
    }
    let mut drift = Vec::new();
    for (id, enabled, severity) in DEFAULTS_SNAPSHOT {
        assert!(
            available.contains(id),
            "{id} is in DEFAULTS_SNAPSHOT but is not an available gate"
        );
        let s = defaults.gates.settings(id).expect("available gate");
        if s.enabled() != *enabled || s.severity() != *severity {
            drift.push(format!(
                "{id}: snapshot ({enabled}, {severity}) != compiled ({}, {})",
                s.enabled(),
                s.severity()
            ));
        }
    }
    assert!(
        drift.is_empty(),
        "default enablement/severity changed; update DEFAULTS_SNAPSHOT and record the change \
         in docs/ROADMAP.md \"Default Changes\": {drift:#?}"
    );
}

#[test]
fn schema_severity_enum_matches_accepted_severities() {
    let schema = discipline::schema::generate_schema();
    let values: Vec<String> = schema["$defs"]["Severity"]["enum"]
        .as_array()
        .expect("Severity enum")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    for v in &values {
        let body = format!("[meta]\nversion = 1\nname = \"t\"\n[gates.pii]\nseverity = \"{v}\"\n");
        let parsed = DisciplineConfig::from_toml_str(&body);
        assert!(
            parsed.is_ok(),
            "schema advertises severity {v:?} but the config parser rejects it: {:?}",
            parsed.err()
        );
    }
    for sev in [Severity::Error, Severity::Warning, Severity::Note] {
        assert!(
            values.contains(&sev.to_string()),
            "config accepts severity {sev} but the schema does not list it: {values:?}"
        );
    }
}

/// `capped_at_warning` lowers an error to a warning and leaves the two lower severities
/// as they are: a gate uses it for a finding that must not fail the run on its own.
#[test]
fn capped_at_warning_lowers_only_an_error() {
    assert_eq!(Severity::Error.capped_at_warning(), Severity::Warning);
    assert_eq!(Severity::Warning.capped_at_warning(), Severity::Warning);
    assert_eq!(Severity::Note.capped_at_warning(), Severity::Note);
}

/// Names the harness sets for every spawn instead of removing them.
const SET_BY_HARNESS: &[&str] = &["DISCIPLINE_NO_NETWORK", "COPILOT_HOME", "HOME"];

/// Every `.rs` file under `src`, with its text.
fn binary_sources() -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut stack = vec![std::path::PathBuf::from("src")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                found.push((path.to_string_lossy().replace('\\', "/"), text));
            }
        }
    }
    found.sort();
    found
}

/// Every environment variable the binary reads must be isolated by the test harness, or a
/// developer's shell (or a CI runner's own variables) could decide a test's verdict.
/// This one finds a name by its family prefix, wherever the literal stands (a constant, a
/// table, an argument); a bare `CI` is a name too (#572: the underscore the expression
/// required after the prefix hid it).
#[test]
fn test_harness_isolates_every_environment_variable_the_binary_reads() {
    let family = regex::Regex::new(
        r#""(CI|(?:DISCIPLINE|GITHUB|GITEA|FORGEJO|GITLAB|CI|PR|GH|DOCS)_[A-Z0-9_]+)""#,
    )
    .unwrap();
    // Read when the binary is compiled (`option_env!`), not when it runs: the release
    // pipeline sets it, and no run of the binary sees it.
    let compile_time = ["DISCIPLINE_RELEASE_SHA"];
    let mut missing = std::collections::BTreeSet::new();
    for (path, text) in binary_sources() {
        for c in family.captures_iter(&text) {
            let name = &c[1];
            if !common::ISOLATED_ENV_VARS.contains(&name)
                && !SET_BY_HARNESS.contains(&name)
                && !compile_time.contains(&name)
            {
                missing.insert(format!("{name} ({path})"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "add to tests/common/mod.rs ISOLATED_ENV_VARS: {missing:?}"
    );
}

/// #572: a name outside the families (`HOME`, `NO_COLOR`, `USERPROFILE`) was invisible to
/// the check above. This one finds a literal name by the call that reads it: `var(..)`,
/// `var_os(..)`, the `env(..)` / `get_env(..)` closures the readers are handed, and an
/// argument's `env = ".."` attribute.
#[test]
fn test_harness_isolates_names_read_without_a_family_prefix() {
    let read = regex::Regex::new(
        r#"\b(?:var|var_os|env|get_env)\)?\(\s*"([A-Za-z_][A-Za-z0-9_]*)"\s*\)|\benv\s*=\s*"([A-Za-z_][A-Za-z0-9_]*)""#,
    )
    .unwrap();
    // Inside source text the Rust pack's own unit tests parse: no run reads it.
    let fixture_text = ["SKIP_SLOW"];
    let mut missing = std::collections::BTreeSet::new();
    for (path, text) in binary_sources() {
        for c in read.captures_iter(&text) {
            let name = c.get(1).or(c.get(2)).unwrap().as_str();
            if !common::ISOLATED_ENV_VARS.contains(&name)
                && !SET_BY_HARNESS.contains(&name)
                && !fixture_text.contains(&name)
            {
                missing.insert(format!("{name} ({path})"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "add to tests/common/mod.rs ISOLATED_ENV_VARS, or set it in isolate_env: {missing:?}"
    );
}

/// Reads whose name is not a literal at the call, and sweeps of the whole environment:
/// file, the argument as written, and how the harness covers the names it can take. The
/// two checks above cannot see through these, so a new one fails here until it is read
/// and recorded.
const REVIEWED_DYNAMIC_READS: &[(&str, &str, &str)] = &[
    ("src/forge.rs", "k", ENV_CLOSURE),
    ("src/gitctx.rs", "k", ENV_CLOSURE),
    ("src/main.rs", "k", ENV_CLOSURE),
    (
        "src/guards/mod.rs",
        "k",
        "the CI markers of `pull_request_for`: a literal table (`CI`, `GITHUB_ACTIONS`, ...) the family check sees",
    ),
    (
        "src/guards/ci_skip_set.rs",
        "var",
        "the `GITHUB_FIELDS` table: literals of the GITHUB_ family, which is also swept by prefix",
    ),
    ("src/guards/ci_skip_set.rs", "CONTEXT_ENV", NAMED_CONSTANT),
    (
        "src/guards/instruction_smuggling.rs",
        "crate::hook::HOOK_RUN_ENV",
        NAMED_CONSTANT,
    ),
    ("src/guards/mod.rs", "REPLAY_CASE_ENV", NAMED_CONSTANT),
    ("src/cli.rs", "HOSTNAME_DENYLIST_ENV", NAMED_CONSTANT),
    ("src/cli.rs", "TERM_DENYLIST_ENV", NAMED_CONSTANT),
    (
        "src/guards/command.rs",
        "&env_key",
        "`DISCIPLINE_COMMAND_<GATE>`, built from the gate's name: in no list, removed by the DISCIPLINE_ prefix sweep",
    ),
    (
        "src/replay.rs",
        "vars()",
        "names to remove from the environment of the check a replay starts; nothing is read from them",
    ),
];

const ENV_CLOSURE: &str = "handed to a reader as its `env` / `get_env` closure; every name the reader asks for is a literal the two checks above see";
const NAMED_CONSTANT: &str =
    "a constant whose value is a DISCIPLINE_ literal the family check sees";

/// #572: a name built at run time (`DISCIPLINE_COMMAND_<GATE>`) or handed to a closure is
/// in no list; each such read site is pinned with the reason the harness covers it.
#[test]
fn every_environment_read_by_a_computed_name_is_reviewed() {
    let dynamic = regex::Regex::new(
        r#"\benv::(var|var_os)\(\s*([^"\\\s)][^)]*)\)|\benv::(vars|vars_os)\(\)"#,
    )
    .unwrap();
    let mut found = std::collections::BTreeSet::new();
    for (path, text) in binary_sources() {
        for c in dynamic.captures_iter(&text) {
            let argument = match c.get(2) {
                Some(a) => a.as_str().trim().to_string(),
                None => format!("{}()", &c[3]),
            };
            found.insert((path.clone(), argument));
        }
    }
    let reviewed: std::collections::BTreeSet<(String, String)> = REVIEWED_DYNAMIC_READS
        .iter()
        .map(|(path, argument, _)| (path.to_string(), argument.to_string()))
        .collect();
    let unreviewed: Vec<_> = found.difference(&reviewed).collect();
    let stale: Vec<_> = reviewed.difference(&found).collect();
    assert!(
        unreviewed.is_empty() && stale.is_empty(),
        "record in REVIEWED_DYNAMIC_READS how the harness isolates each: {unreviewed:#?}\nentries that no longer match: {stale:#?}"
    );
}

#[test]
fn extra_assert_macros_drop_a_trailing_bang_at_load() {
    let cfg = discipline::config::DisciplineConfig::from_toml_str(
        "[meta]\nversion = 1\nname = \"t\"\n[gates.assertion-reduction]\nextra_assert_macros = [\"assert_matches!\", \"plain\"]\n[gates.vacuous-tests]\nextra_assert_macros = [\" check_sorted! \"]\n",
    )
    .unwrap();
    assert_eq!(
        cfg.gates.assertion_reduction.extra_assert_macros,
        ["assert_matches", "plain"]
    );
    assert_eq!(
        cfg.gates.vacuous_tests.extra_assert_macros,
        ["check_sorted"]
    );
}

/// The rename mechanism under a synthetic alias table: no key is renamed yet, so the real
/// table is empty and this is the only way to exercise the path 1.0 promises.
#[test]
fn a_renamed_key_is_read_under_its_old_name_with_a_deprecation_note() {
    use discipline::config::KeyAlias;
    const ALIASES: &[KeyAlias] = &[
        KeyAlias {
            old: "gates.vacuous-tests.min_assertions",
            new: "min_assertions_per_test",
        },
        KeyAlias {
            old: "gates.*.exempt",
            new: "exempt_paths",
        },
    ];
    let head = CONFIG_HEAD;

    // The old name alone: read as the new one, with one note per rewrite.
    let cfg = DisciplineConfig::from_toml_str_with_aliases(
        &format!("{head}[gates.vacuous-tests]\nmin_assertions = 2\n[gates.pii]\nexempt = [\"a/**\"]\n[gates.agents-md]\nexempt = [\"b/**\"]\n"),
        ALIASES,
    )
    .unwrap();
    assert_eq!(cfg.gates.vacuous_tests.min_assertions_per_test, Some(2));
    assert_eq!(cfg.gates.pii.exempt_paths, ["a/**"]);
    assert_eq!(cfg.gates.agents_md.exempt_paths, ["b/**"]);
    assert_eq!(cfg.deprecations.len(), 3, "{:?}", cfg.deprecations);
    assert!(cfg.deprecations.iter().any(|n| n.starts_with(
        "`gates.vacuous-tests.min_assertions` is deprecated: it is read as `gates.vacuous-tests.min_assertions_per_test`"
    )));

    // Both names in one table: an error, never a silent pick.
    let err = DisciplineConfig::from_toml_str_with_aliases(
        &format!("{head}[gates.vacuous-tests]\nmin_assertions = 2\nmin_assertions_per_test = 3\n"),
        ALIASES,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("are both set"), "{err}");

    // The new name alone: no note.
    let cfg = DisciplineConfig::from_toml_str_with_aliases(
        &format!("{head}[gates.vacuous-tests]\nmin_assertions_per_test = 3\n"),
        ALIASES,
    )
    .unwrap();
    assert!(cfg.deprecations.is_empty());

    // Without the alias the old name is an unknown key, as for any typo.
    assert!(DisciplineConfig::from_toml_str_with_aliases(
        &format!("{head}[gates.vacuous-tests]\nmin_assertions = 2\n"),
        &[],
    )
    .is_err());
}

/// An empty `review_trailer` is the v0.17 spelling of `require_agent_review = false`: it
/// still loads, switches the rule off and leaves a deprecation note. Refusing it made a
/// base configuration written for v0.17 unloadable (#518).
/// Killed mutant: the empty-name rewrite removed from configuration loading.
#[test]
fn an_empty_review_trailer_reads_as_require_agent_review_false_with_a_deprecation() {
    let head = common::CONFIG_HEAD;
    for empty in ["\"\"", "\"  \""] {
        let cfg = DisciplineConfig::from_toml_str(&format!(
            "{head}[gates.commit-provenance]\nreview_trailer = {empty}\n"
        ))
        .unwrap();
        assert!(!cfg.gates.commit_provenance.require_agent_review);
        assert_eq!(cfg.gates.commit_provenance.review_trailer, "Reviewed-by");
        assert_eq!(cfg.deprecations.len(), 1, "{:?}", cfg.deprecations);
        assert!(
            cfg.deprecations[0].contains("require_agent_review = false"),
            "{:?}",
            cfg.deprecations
        );
    }
    // Both spellings at once, disagreeing, is an error naming both keys.
    let err = DisciplineConfig::from_toml_str(&format!(
        "{head}[gates.commit-provenance]\nreview_trailer = \"\"\nrequire_agent_review = true\n"
    ))
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("review_trailer") && err.contains("require_agent_review"),
        "{err}"
    );
    // Agreeing is the same as the new key alone, still noted.
    let both = DisciplineConfig::from_toml_str(&format!(
        "{head}[gates.commit-provenance]\nreview_trailer = \"\"\nrequire_agent_review = false\n"
    ))
    .unwrap();
    assert!(!both.gates.commit_provenance.require_agent_review);
    assert_eq!(both.deprecations.len(), 1);
    let defaults = DisciplineConfig::from_toml_str(head).unwrap();
    assert!(defaults.gates.commit_provenance.require_agent_review);
    assert_eq!(
        defaults.gates.commit_provenance.review_trailer,
        "Reviewed-by"
    );
    let off = DisciplineConfig::from_toml_str(&format!(
        "{head}[gates.commit-provenance]\nrequire_agent_review = false\n"
    ))
    .unwrap();
    assert!(!off.gates.commit_provenance.require_agent_review);
}
