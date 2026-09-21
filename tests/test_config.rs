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
    let head = "[meta]\nversion = 1\nname = \"t\"\n";
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
    assert_eq!(base.directives.sources, vec!["pr-body", "commits"]);
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
        "committed discipline.schema.json does not match discipline::schema::generate_schema(); run `cargo run -- schema > discipline.schema.json`"
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
# sources = ["pr-body", "commits"]
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
            || g.id == "archive-contents"
            || g.id == "manifest-sync"
            || g.id == "version-lockstep"
            || g.id == "scope-confinement"
            || g.id == "pr-checklist"
            || g.id == "unsafe-budget"
            || g.id == "msrv"
            || g.id == "miri"
            || g.id == "sanitizers"
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
fn schema_json_file_in_sync_with_code() {
    let committed = std::fs::read_to_string("discipline.schema.json")
        .expect("discipline.schema.json must exist");
    let generated = serde_json::to_string_pretty(&discipline::schema::generate_schema())
        .expect("must serialize generated schema")
        + "\n";

    assert_eq!(
        committed, generated,
        "discipline.schema.json is out of sync with Rust Serde models; run `cargo run -- docs --write` to update"
    );
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
    ("suppression-delta", true, Severity::Warning),
    ("time-estimates", true, Severity::Warning),
    ("pii", true, Severity::Error),
    ("agent-scratch", true, Severity::Error),
    ("shell-secrets", true, Severity::Error),
    ("issue-link", false, Severity::Error),
    ("provenance-tags", false, Severity::Error),
    ("pr-checklist", false, Severity::Error),
    ("config-integrity", true, Severity::Error),
    ("golden-output", true, Severity::Error),
    ("dependency-delta", true, Severity::Error),
    ("test-budget", true, Severity::Error),
    ("ci-integrity", true, Severity::Error),
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
