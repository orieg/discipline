use discipline::config::{split_list, DisciplineConfig, Overrides, Severity, GATES};
use std::path::Path;

#[test]
fn dogfood_config_loads_with_every_available_gate_on() {
    let config = DisciplineConfig::load_from_file(Path::new("discipline.toml"))
        .expect("discipline.toml must load under the strict schema");
    assert_eq!(config.meta.name, "discipline");
    for gate in GATES.iter().filter(|g| g.available) {
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
        "[gates.miri]\nenabled = true\n",
        "[gates.golden-output]\nenabled = true\n",
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
