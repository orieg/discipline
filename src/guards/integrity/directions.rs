//! Which way each configuration key loosens: the tables `config-integrity` judges a
//! changed `discipline.toml` by.

/// How a change to one option moves the bar. Option names are judged the same way in
/// every gate table, so a name must keep one meaning across gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// List: a gained entry is looser.
    Grown,
    /// List: a lost entry is looser. An edited entry counts as lost, unless the option is
    /// in [`ENTRY_SHAPES`] and the edit only tightens the entry.
    Shrunk,
    /// Allow-list: a gained entry is looser, and so is emptying it (an empty allow-list
    /// switches the restriction off).
    Allowlist,
    /// Number: a larger value is looser.
    Tolerance,
    /// Number: a smaller value is looser, and so is removing it.
    Floor,
    /// Number: a larger value is looser, and so is removing it.
    Cap,
    /// Version (`1.90`, `1.90.1`): a lower version is looser, and so is removing it. The
    /// two sides are compared as versions, never as text ([`version_order`]).
    VersionFloor,
    /// Boolean: `true` is the looser value.
    LooserWhenTrue,
    /// Boolean: `false` is the looser value.
    LooserWhenFalse,
    /// Names what the gate checks against or runs: removing or repointing it replaces
    /// the check.
    Evidence,
    /// Mode switch whose named value is the stricter check.
    StrictMode(&'static str),
    /// Mode switch with values ordered strictest first: a later value is looser.
    Ordered(&'static [&'static str]),
    /// `error` > `warning` > `note`.
    Severity,
    /// Does not move the bar, or is judged as part of its enclosing list entry.
    Neutral,
}

/// Every option accepted under `[gates.<id>]`, classified. An option missing from this
/// table is a hole in the gate: `every_gate_option_is_classified` fails until it is added.
/// Optional `Tolerance` keys whose absence means no limit, so removing one loosens. Others
/// fall back to a default (`noise_floor_pct`, filled before comparison) or to the strictest
/// reading (`noise_margin_pct` absent is 0).
pub const ABSENT_IS_UNLIMITED: &[&str] = &["max_noise_cv"];

/// Optional `Tolerance` keys whose absence means no tolerance is added, so adding one
/// above zero loosens.
pub const ABSENT_IS_NONE: &[&str] = &["noise_margin_pct", "ratio_tolerance_pct"];

pub const KEY_DIRECTIONS: &[(&str, Direction)] = &[
    // Common to every gate.
    ("enabled", Direction::LooserWhenFalse),
    ("severity", Direction::Severity),
    ("exempt_paths", Direction::Grown),
    ("ci_skip_severity", Direction::Severity),
    // Lists that widen what is tolerated.
    ("allow_patterns", Direction::Grown),
    ("allowed_users", Direction::Grown),
    ("extra_assert_macros", Direction::Grown),
    ("assert_helper_fns", Direction::Grown),
    ("approved_predicates", Direction::Grown),
    ("pending_issue_repos", Direction::Grown),
    ("exempt_arms", Direction::Grown),
    ("allowed_suppressions", Direction::Grown),
    ("excluded_jobs", Direction::Grown),
    ("first_party_action_prefixes", Direction::Grown),
    ("snapshot_ignore", Direction::Grown),
    // Allow-lists: growing or emptying both loosen.
    ("allow_dependencies", Direction::Allowlist),
    ("allowed_paths", Direction::Allowlist),
    // Lists that define what is examined or rejected.
    ("paths", Direction::Shrunk),
    ("include", Direction::Shrunk),
    ("extra_patterns", Direction::Shrunk),
    ("instruction_files", Direction::Shrunk),
    ("constant_fallback_paths", Direction::Shrunk),
    ("hostname_denylist", Direction::Shrunk),
    ("superseded_json_paths", Direction::Shrunk),
    ("record_paths", Direction::Shrunk),
    ("citation_source_paths", Direction::Shrunk),
    ("citation_measurement_jobs", Direction::Shrunk),
    ("unconditional_jobs", Direction::Shrunk),
    ("placeholders", Direction::Shrunk),
    ("workflows", Direction::Shrunk),
    ("forbidden_paths", Direction::Shrunk),
    ("deny_dependencies", Direction::Shrunk),
    ("manifests", Direction::Shrunk),
    ("required_paths", Direction::Shrunk),
    ("forbidden_patterns", Direction::Shrunk),
    ("forbid_output", Direction::Shrunk),
    ("corpus_dirs", Direction::Shrunk),
    ("fuzz_targets", Direction::Shrunk),
    ("extra_secret_patterns", Direction::Shrunk),
    ("required_suites", Direction::Shrunk),
    ("commands", Direction::Shrunk),
    ("mock_setup_fns", Direction::Shrunk),
    ("required_trailers", Direction::Shrunk),
    ("ratio_satisfied_by", Direction::Grown),
    ("deterministic_units", Direction::Grown),
    ("agent_markers", Direction::Shrunk),
    ("review_trailer", Direction::Evidence),
    ("require_agent_review", Direction::LooserWhenFalse),
    ("mock_assert_fns", Direction::Shrunk),
    ("rules", Direction::Shrunk),
    ("groups", Direction::Shrunk),
    // A lost entry lets a banned action back in; a gained one bans more.
    ("banned_actions", Direction::Shrunk),
    // Numbers.
    ("tolerance_pct", Direction::Tolerance),
    ("ratio_tolerance_pct", Direction::Tolerance),
    ("noise_floor_pct", Direction::Tolerance),
    ("noise_margin_pct", Direction::Tolerance),
    ("advisory_pct", Direction::Tolerance),
    ("max_noise_cv", Direction::Tolerance),
    ("tolerance", Direction::Tolerance),
    ("figure_tolerance_pct", Direction::Tolerance),
    ("min_count", Direction::Floor),
    ("min_tests", Direction::Floor),
    ("test_report", Direction::Evidence),
    ("base_report", Direction::Evidence),
    ("head_report", Direction::Evidence),
    ("min_assertions_per_test", Direction::Floor),
    // The version `msrv` reports and its command is said to pass under.
    ("pinned_version", Direction::VersionFloor),
    // A lower cap leaves more archive entries unscanned.
    ("max_entry_bytes", Direction::Floor),
    ("max_unsafe", Direction::Cap),
    ("max_increase", Direction::Cap),
    // Booleans where `true` relaxes the gate.
    ("allow_hidden", Direction::LooserWhenTrue),
    ("allow_zero", Direction::LooserWhenTrue),
    ("diff_only", Direction::LooserWhenTrue),
    ("allow_cross_host", Direction::LooserWhenTrue),
    ("allow_wildcards", Direction::LooserWhenTrue),
    ("allow_increase", Direction::LooserWhenTrue),
    // Booleans where `false` switches a check off.
    ("require_scope", Direction::LooserWhenFalse),
    ("scan_pr_body", Direction::LooserWhenFalse),
    ("home_paths", Direction::LooserWhenFalse),
    ("lan_ips", Direction::LooserWhenFalse),
    ("secrets", Direction::LooserWhenFalse),
    ("agent_config_refs", Direction::LooserWhenFalse),
    ("agent_config_standard_paths", Direction::LooserWhenTrue),
    ("require_sourced_override", Direction::LooserWhenFalse),
    ("check_tables", Direction::LooserWhenFalse),
    ("check_mechanisms", Direction::LooserWhenFalse),
    ("check_intervals", Direction::LooserWhenFalse),
    ("check_paired_figures", Direction::LooserWhenFalse),
    ("check_pending_citations", Direction::LooserWhenFalse),
    ("require_open_pending_issues", Direction::LooserWhenFalse),
    ("verify_measured_commit", Direction::LooserWhenFalse),
    ("verify_cited_figures", Direction::LooserWhenFalse),
    ("require_git_pins", Direction::LooserWhenFalse),
    ("scan_workflows", Direction::LooserWhenFalse),
    ("scan_scripts", Direction::LooserWhenFalse),
    ("require_in_commit_if_no_pr", Direction::LooserWhenFalse),
    ("verify_references", Direction::LooserWhenFalse),
    ("require_open_issue", Direction::LooserWhenFalse),
    ("accept_pull_references", Direction::LooserWhenTrue),
    ("reference_repos", Direction::Grown),
    ("waiver", Direction::StrictMode("none")),
    ("exempt_authors", Direction::Grown),
    ("protected_paths", Direction::Shrunk),
    ("never_ratifiable", Direction::Shrunk),
    ("ratifiers", Direction::Grown),
    ("agent_logins", Direction::Shrunk),
    ("marker", Direction::Evidence),
    ("closing_source", Direction::Neutral),
    ("closing_keywords", Direction::Evidence),
    ("ratification_repos", Direction::Grown),
    (
        "ratification_valid_from",
        Direction::Ordered(&["pull-created", "path-last-changed", "any"]),
    ),
    ("ratification_max_age_days", Direction::Cap),
    ("accept_edited", Direction::StrictMode("never")),
    ("accept_email_replies", Direction::LooserWhenTrue),
    ("refuse_author_ratification", Direction::LooserWhenFalse),
    ("pin_actions", Direction::LooserWhenFalse),
    ("forbid_continue_on_error", Direction::LooserWhenFalse),
    ("forbid_or_true", Direction::LooserWhenFalse),
    ("canary", Direction::LooserWhenFalse),
    ("scan_contents", Direction::LooserWhenFalse),
    // What the gate runs or checks against.
    ("superseded_registry", Direction::Evidence),
    ("record_commit_key", Direction::Evidence),
    ("ratio_baseline", Direction::Evidence),
    ("workflow", Direction::Evidence),
    ("change_job", Direction::Evidence),
    ("command", Direction::Evidence),
    ("test_command", Direction::Evidence),
    ("canary_command", Direction::Evidence),
    ("canary_expected_diagnostic", Direction::Evidence),
    ("preset", Direction::Evidence),
    ("count_pattern", Direction::Evidence),
    ("zero_items_pattern", Direction::Evidence),
    ("snapshot", Direction::Evidence),
    ("constant_file", Direction::Evidence),
    ("constant_name", Direction::Evidence),
    ("deny_file", Direction::Evidence),
    ("pattern", Direction::Evidence),
    ("rollup_job", Direction::Evidence),
    ("documented_job_count_path", Direction::Evidence),
    ("documented_job_count_pattern", Direction::Evidence),
    ("sanitizer", Direction::Evidence),
    ("archive_path", Direction::Evidence),
    ("mode", Direction::StrictMode("paired-ratio")),
    // Output shaping, run limits that can only fail a run sooner, and fields of list
    // entries (judged with their entry: an edited entry counts as a lost one unless
    // `ENTRY_SHAPES` finds it only tightened).
    ("redact_lan_ips", Direction::Neutral),
    ("timeout_seconds", Direction::Neutral),
    ("strip_components", Direction::Neutral),
    ("provenance", Direction::Neutral),
    ("base_file", Direction::Neutral),
    ("head_file", Direction::Neutral),
    ("args", Direction::Neutral),
    ("name", Direction::Neutral),
    // `banned_actions` entry fields: an edited entry is a lost one.
    ("uses", Direction::Neutral),
    ("reason", Direction::Neutral),
    ("job", Direction::Neutral),
    ("guard", Direction::Neutral),
];

/// A list-of-tables option whose entries are matched across base and head by identity, so
/// an entry that only tightened is not counted as lost.
pub struct EntryShape {
    /// The option under `[gates.<id>]`; it is classified `Shrunk`.
    pub key: &'static str,
    /// Fields that name the entry. The same values must name exactly one entry on each side.
    pub identity: &'static [&'static str],
    /// List fields where a gained item is stricter: every base item must stay.
    pub stricter_when_grown: &'static [&'static str],
    /// List fields where a lost item is stricter: no item may be gained.
    pub stricter_when_shrunk: &'static [&'static str],
    /// Fields whose addition is stricter: absent on base, any value on head passes.
    pub stricter_when_added: &'static [&'static str],
}

/// Every other field of a matched entry must be unchanged. `citation_measurement_jobs` is
/// absent: both of its fields name what is checked, so any edit repoints the check.
pub const ENTRY_SHAPES: &[EntryShape] = &[
    // version-lockstep: one more source is one more place held to the same version.
    EntryShape {
        key: "groups",
        identity: &["name"],
        stricter_when_grown: &["sources"],
        stricter_when_shrunk: &[],
        stricter_when_added: &[],
    },
    // manifest-sync: more watched paths require the manifest to move more often; fewer
    // exclusions leave less outside the rule.
    EntryShape {
        key: "rules",
        identity: &["manifest", "extract_regex"],
        stricter_when_grown: &["watched_paths"],
        stricter_when_shrunk: &["exclude_paths"],
        stricter_when_added: &[],
    },
    // command: one more forbidden output pattern is one more way the command fails; a
    // snapshot added is one more comparison, and fewer ignored lines compare more.
    EntryShape {
        key: "commands",
        identity: &["name"],
        stricter_when_grown: &["forbid_output"],
        stricter_when_shrunk: &["snapshot_ignore"],
        stricter_when_added: &["snapshot"],
    },
];

/// Optional evidence keys with no value that stands in when unset: the gate then counts
/// on another basis, so adding the key replaces the basis the base ref's floor was
/// measured on. `(gate, key)`.
pub const ADDED_IS_ANOTHER_BASIS: &[(&str, &str)] = &[
    ("test-floor", "test_command"),
    ("test-floor", "test_report"),
    // The head-side report is what the count is read from, as `test_report` is.
    // `base_report` is not here: it names what the head report is compared with, and
    // the gate cannot run with it alone. Added beside a `test_report` the base already
    // had, it is reported for what it replaces (`BASE_SIDE_MOVED`).
    ("test-floor", "head_report"),
];

pub fn direction_of(key: &str) -> Option<Direction> {
    KEY_DIRECTIONS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, d)| *d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DisciplineConfig;
    use toml::Value;

    #[test]
    fn every_entry_shape_names_a_shrunk_option_and_its_real_fields() {
        let schema = serde_json::to_string(&crate::schema::generate_schema()).unwrap();
        for shape in ENTRY_SHAPES {
            assert_eq!(
                direction_of(shape.key),
                Some(Direction::Shrunk),
                "{}",
                shape.key
            );
            for field in shape
                .identity
                .iter()
                .chain(shape.stricter_when_grown)
                .chain(shape.stricter_when_shrunk)
                .chain(shape.stricter_when_added)
            {
                assert!(
                    schema.contains(&format!("\"{field}\"")),
                    "{}.{field}",
                    shape.key
                );
            }
        }
    }

    /// Every option name a gate table accepts, from the published schema and from the
    /// serialized defaults (the schema has lagged the structs before; the union covers
    /// both). An `Option` field absent from the schema is the one shape this cannot see.
    fn all_gate_option_names() -> std::collections::BTreeSet<String> {
        let mut names = std::collections::BTreeSet::new();
        let schema = crate::schema::generate_schema();
        for def in schema["$defs"].as_object().unwrap().values() {
            if let Some(props) = def.get("properties").and_then(|p| p.as_object()) {
                names.extend(props.keys().cloned());
            }
        }
        fn walk(v: &Value, names: &mut std::collections::BTreeSet<String>) {
            match v {
                Value::Table(t) => {
                    for (k, child) in t {
                        names.insert(k.clone());
                        walk(child, names);
                    }
                }
                Value::Array(a) => a.iter().for_each(|x| walk(x, names)),
                _ => {}
            }
        }
        let gates = Value::try_from(DisciplineConfig::default_for_repo("t").gates).unwrap();
        for table in gates.as_table().unwrap().values() {
            walk(table, &mut names);
        }
        names
    }

    #[test]
    fn schema_and_config_structs_name_the_same_gate_options() {
        // The schema is what an editor validates against; the struct is what loads.
        // A key in one and not the other passes validation and then fails to load, or
        // loads and is never offered.
        let schema = crate::schema::generate_schema();
        let gates_schema = schema["properties"]["gates"]["properties"]
            .as_object()
            .unwrap();
        let defaults = Value::try_from(DisciplineConfig::default_for_repo("t").gates).unwrap();
        let mut drift = Vec::new();
        for (gate, table) in defaults.as_table().unwrap() {
            let def_ref = gates_schema[gate]["allOf"][0]["$ref"].as_str().unwrap();
            let def_name = def_ref.rsplit('/').next().unwrap();
            let props = schema["$defs"][def_name]["properties"].as_object().unwrap();
            for key in table.as_table().unwrap().keys() {
                if !props.contains_key(key) {
                    drift.push(format!("{gate}.{key} loads but is not in the schema"));
                }
            }
            for key in props.keys() {
                let toml =
                    format!("[meta]\nversion = 1\nname = \"t\"\n[gates.{gate}]\n{key} = 0\n");
                let err = DisciplineConfig::from_toml_str(&toml)
                    .err()
                    .map(|e| format!("{e:#}"))
                    .unwrap_or_default();
                if err.contains("unknown field") {
                    drift.push(format!("{gate}.{key} is in the schema but does not load"));
                }
            }
        }
        assert!(drift.is_empty(), "{drift:#?}");
    }

    #[test]
    fn every_gate_option_is_classified() {
        let names = all_gate_option_names();
        assert!(names.len() > 80, "option enumeration collapsed: {names:?}");
        let unclassified: Vec<_> = names.iter().filter(|n| direction_of(n).is_none()).collect();
        assert!(
            unclassified.is_empty(),
            "add these options to KEY_DIRECTIONS: {unclassified:?}"
        );
        let dead: Vec<_> = KEY_DIRECTIONS
            .iter()
            .map(|(k, _)| *k)
            .filter(|k| !names.contains(*k))
            .collect();
        assert!(
            dead.is_empty(),
            "KEY_DIRECTIONS names no real option: {dead:?}"
        );
        let mut seen = std::collections::BTreeSet::new();
        for (k, _) in KEY_DIRECTIONS {
            assert!(seen.insert(*k), "`{k}` is classified twice");
        }
    }
}
