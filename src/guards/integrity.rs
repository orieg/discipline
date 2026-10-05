//! Gate-integrity: a change must not quietly lower the bar it is judged by.
//!
//! `discipline.toml` is fully user-configurable, which makes it the cheapest
//! thing for an agent to edit when a gate is in the way. This gate compares
//! the configuration on the base side with the head side and demands a scoped
//! `allow-gate-weakening:` directive for every loosening.

use super::{Context, GateOutcome, PathFilter};
use crate::config::{DisciplineConfig, GateSettings, RunMode, Severity};
use crate::gitctx::ChangeKind;
use crate::tokens;
use anyhow::Result;
use toml::Value;

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
    ("hostname_denylist", Direction::Shrunk),
    ("superseded_json_paths", Direction::Shrunk),
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
    ("min_count", Direction::Floor),
    ("min_tests", Direction::Floor),
    ("min_assertions_per_test", Direction::Floor),
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
    ("pinned_version", Direction::Neutral),
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

/// Whether `entry`, a base entry of `key` missing from head as written, is on head under the
/// same identity with only tightening edits. Anything ambiguous reads as lost.
fn entry_tightened(key: &str, entry: &Value, base: &[Value], head: &[Value]) -> bool {
    let Some(shape) = ENTRY_SHAPES.iter().find(|s| s.key == key) else {
        return false;
    };
    let Some(b) = entry.as_table() else {
        return false;
    };
    let id = |t: &toml::Table| -> Vec<Option<Value>> {
        shape.identity.iter().map(|f| t.get(*f).cloned()).collect()
    };
    let wanted = id(b);
    if wanted.iter().any(Option::is_none) {
        return false;
    }
    let named = |list: &[Value]| -> Vec<toml::Table> {
        list.iter()
            .filter_map(Value::as_table)
            .filter(|t| id(t) == wanted)
            .cloned()
            .collect()
    };
    let (on_base, on_head) = (named(base), named(head));
    let ([_], [h]) = (on_base.as_slice(), on_head.as_slice()) else {
        return false;
    };
    b.keys().chain(h.keys()).all(|field| {
        let (bv, hv) = (b.get(field), h.get(field));
        if shape.stricter_when_grown.contains(&field.as_str()) {
            within(bv, hv)
        } else if shape.stricter_when_shrunk.contains(&field.as_str()) {
            within(hv, bv)
        } else if shape.stricter_when_added.contains(&field.as_str()) && bv.is_none() {
            true
        } else {
            bv == hv
        }
    })
}

/// Every item of the list `small` is in the list `big`; an absent list reads as empty.
fn within(small: Option<&Value>, big: Option<&Value>) -> bool {
    fn items(v: Option<&Value>) -> Option<&[Value]> {
        match v {
            None => Some(&[]),
            Some(Value::Array(a)) => Some(a),
            Some(_) => None,
        }
    }
    match (items(small), items(big)) {
        (Some(s), Some(g)) => s.iter().all(|x| g.contains(x)),
        _ => false,
    }
}

pub fn direction_of(key: &str) -> Option<Direction> {
    KEY_DIRECTIONS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, d)| *d)
}

/// How a configuration option moved in the looser direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Change {
    /// A switch or a named value changed to a looser one.
    Changed,
    /// An option that bounds or names the check was removed.
    Removed,
    Increased,
    Decreased,
    /// A severity lowered.
    Lowered,
    /// A list gained entries.
    Gained,
    /// A list lost entries.
    Lost,
    /// An allow-list was emptied, which switches it off.
    Emptied,
}

/// One loosening of a configuration option, as data. [`Weakening::what`] is the sentence
/// the `config-integrity` finding shows.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Weakening {
    /// The gate id, or the table (`directives`, `tests`, `languages`, `meta`).
    pub gate: String,
    /// The option, as written under that table (`c.macros` under `languages`).
    pub key: String,
    pub change: Change,
    /// The value before and after, as the message shows them; `None` for a list, whose
    /// entries are counted, never named (an entry can be a login).
    pub before: Option<String>,
    pub after: Option<String>,
    /// Entries gained or lost, for a list.
    pub count: Option<usize>,
    /// Why the change is looser, when the sentence says so.
    #[serde(skip)]
    pub note: Option<&'static str>,
}

impl Weakening {
    fn new(gate: &str, key: &str, change: Change) -> Self {
        Weakening {
            gate: gate.to_string(),
            key: key.to_string(),
            change,
            before: None,
            after: None,
            count: None,
            note: None,
        }
    }

    fn values(mut self, before: impl ToString, after: impl ToString) -> Self {
        self.before = Some(before.to_string());
        self.after = Some(after.to_string());
        self
    }

    fn was(mut self, before: impl ToString) -> Self {
        self.before = Some(before.to_string());
        self
    }

    fn count(mut self, n: usize) -> Self {
        self.count = Some(n);
        self
    }

    /// The key the weakening is about.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The sentence the finding shows, starting with the key in backticks.
    pub fn what(&self) -> String {
        let key = &self.key;
        let b = self.before.as_deref().unwrap_or("");
        let a = self.after.as_deref().unwrap_or("");
        let n = self.count.unwrap_or(0);
        let text = match self.change {
            Change::Changed => format!("`{key}` changed from {b} to {a}"),
            Change::Removed => format!("`{key}` removed (was {b})"),
            Change::Increased => format!("`{key}` increased from {b} to {a}"),
            Change::Decreased => format!("`{key}` decreased from {b} to {a}"),
            Change::Lowered => format!("`{key}` lowered from {b} to {a}"),
            Change::Gained if self.after.is_some() => format!("`{key}` gained {a}"),
            Change::Gained => format!("`{key}` gained {n} entr(y/ies)"),
            Change::Lost => format!("`{key}` lost {n} entr(y/ies)"),
            Change::Emptied => format!("`{key}` emptied, which switches the allow-list off"),
        };
        match self.note {
            Some(note) => format!("{text} ({note})"),
            None => text,
        }
    }
}

/// The base-side configuration source. A `--config` pointing at a file the base does not
/// have still compares against the base `discipline.toml`.
fn base_config_source(ctx: &Context) -> Result<Option<String>> {
    ctx.base_config_text()
}

/// Whether the base side runs this gate. The change under review cannot switch off the
/// gate that judges its configuration: `enabled = false` takes effect once it has merged.
pub fn enabled_on_base(ctx: &Context) -> Result<bool> {
    Ok(base_config_source(ctx)?
        .and_then(|src| DisciplineConfig::from_toml_str(&src).ok())
        .is_some_and(|base| base.gates.config_integrity.enabled()))
}

/// Whether this change is what switched the run to advisory mode, with no scoped override
/// lifting it. Advisory mode exits 0 whatever the gates report, so honouring it here would
/// let the change excuse itself. Adopting discipline in advisory mode (no base
/// configuration) is unaffected.
pub fn advisory_mode_unapproved(ctx: &Context) -> Result<bool> {
    if ctx.config.meta.mode != RunMode::Advisory {
        return Ok(false);
    }
    let base_enforcing = base_config_source(ctx)?
        .and_then(|src| DisciplineConfig::from_toml_str(&src).ok())
        .is_some_and(|base| base.meta.mode == RunMode::Enforcing);
    Ok(base_enforcing
        && ctx
            .find_override(
                "config-integrity",
                &crate::findings::GATE_WEAKENED,
                tokens::ALLOW_GATE_WEAKENING,
                "meta",
            )
            .is_none())
}

/// The stricter of two severities.
fn stricter(a: Severity, b: Severity) -> Severity {
    let rank = |s: Severity| match s {
        Severity::Error => 2,
        Severity::Warning => 1,
        Severity::Note => 0,
    };
    if rank(a) >= rank(b) {
        a
    } else {
        b
    }
}

pub fn config_integrity(ctx: &Context) -> Result<GateOutcome> {
    const GATE: &str = "config-integrity";
    let settings = &ctx.config.gates.config_integrity;
    let mut out = GateOutcome::new(GATE);
    // Findings are reported at the stricter of the base and head severity, so a change
    // cannot demote the report of its own weakenings.
    let mut severity = settings.severity();

    // A configuration outside the repository is the operator's, not the change's: the
    // change cannot have weakened it, so there is nothing of its own to compare.
    if !crate::gitctx::config_in_tree(ctx.config_path) {
        out.notes.push(format!(
            "the configuration in force (`{}`) is outside the repository, so this change cannot edit it; no weakening compared",
            ctx.config_path
        ));
    } else if let Some(base_src) = base_config_source(ctx)? {
        match DisciplineConfig::from_toml_str(&base_src) {
            Ok(base) => {
                let head = ctx.head_config.unwrap_or(ctx.config);
                severity = stricter(severity, base.gates.config_integrity.severity());
                if !settings.enabled() {
                    out.notes.push(
                        "this change disables `config-integrity`; evaluated anyway because the \
                         base configuration enables it"
                            .to_string(),
                    );
                }
                let weakenings = diff_configs(&base, head)?;
                out.examined = Value::try_from(&base.gates)?
                    .as_table()
                    .map(|t| t.len())
                    .unwrap_or(0);
                for w in weakenings {
                    if let Some(record) = ctx.find_override(
                        GATE,
                        &crate::findings::GATE_WEAKENED,
                        tokens::ALLOW_GATE_WEAKENING,
                        &w.gate,
                    ) {
                        out.overrides.push(record);
                        continue;
                    }
                    out.push(
                        ctx.overridable(severity),
                        &crate::findings::GATE_WEAKENED,
                        Some(ctx.config_path),
                        None,
                        format!("[{}] {}.", w.gate, w.what()),
                        &format!(
                            "Revert the change, or justify it on its own line in the PR body or a commit \
                             message: `allow-gate-weakening: {} <reason>`.",
                            w.gate
                        ),
                    );
                    out.anchor_last(format!("{}.{}", w.gate, w.key()));
                }
            }
            Err(e) => {
                // Blocking here would deadlock the PR that repairs the base config.
                out.push(
                    Severity::Warning,
                    &crate::findings::BASE_CONFIGURATION_UNREADABLE,
                    Some(ctx.config_path),
                    None,
                    format!("The base-side configuration does not load with this binary ({e:#}); weakening could not be checked."),
                    "Repair the configuration on the base branch.",
                );
            }
        }
    } else {
        out.notes.push(format!(
            "`{}` does not exist on the base side; nothing to compare against",
            ctx.config_path
        ));
    }

    // Baseline file integrity: growing grandfathered baseline or adding new ungrandfathered fingerprints is a weakening
    let baseline_filename = ctx
        .baseline_path
        .unwrap_or(crate::baseline::DEFAULT_BASELINE_FILE);
    let base_baseline: Option<crate::baseline::DisciplineBaseline> =
        match ctx.git.base_content(baseline_filename)? {
            Some(s) => toml::from_str(&s).ok(),
            None => None,
        };

    let head_baseline_path = ctx.git.root().join(baseline_filename);
    let head_baseline: Option<crate::baseline::DisciplineBaseline> = if head_baseline_path.exists()
    {
        std::fs::read_to_string(&head_baseline_path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
    } else {
        None
    };

    // A fingerprint-version migration (`discipline baseline --migrate`) rewrites every
    // fingerprint. It is accepted without a directive when it is the change's only file,
    // does not grow, and keeps every entry's gate and path: then no finding can have been
    // swapped in under it.
    if let (Some(b), Some(h)) = (&base_baseline, &head_baseline) {
        if b.version < crate::baseline::FINGERPRINT_VERSION
            && h.version >= crate::baseline::FINGERPRINT_VERSION
        {
            let others: Vec<String> = ctx
                .git
                .changed_files()?
                .into_iter()
                .map(|f| f.path)
                .filter(|p| p != baseline_filename)
                .collect();
            let mut base_places: std::collections::HashMap<(&str, &str), usize> =
                std::collections::HashMap::new();
            for e in &b.findings {
                *base_places
                    .entry((e.gate.as_str(), e.path.as_str()))
                    .or_insert(0) += 1;
            }
            let kept_places = h.findings.iter().all(|e| {
                base_places
                    .get_mut(&(e.gate.as_str(), e.path.as_str()))
                    .filter(|n| **n > 0)
                    .map(|n| *n -= 1)
                    .is_some()
            });
            if others.is_empty() && kept_places {
                out.notes.push(format!(
                    "`{baseline_filename}` migrated from fingerprint version {} to {} ({} of {} entries kept)",
                    b.version,
                    h.version,
                    h.findings.len(),
                    b.findings.len()
                ));
                return Ok(out);
            }
            if !others.is_empty() {
                if let Some(record) = ctx.find_override(
                    GATE,
                    &crate::findings::BASELINE_MIGRATION_NOT_ALONE,
                    tokens::ALLOW_GATE_WEAKENING,
                    "baseline",
                ) {
                    out.overrides.push(record);
                    return Ok(out);
                }
                out.push(
                    ctx.overridable(severity),
                    &crate::findings::BASELINE_MIGRATION_NOT_ALONE,
                    Some(baseline_filename),
                    None,
                    format!(
                        "[baseline] `{baseline_filename}` is rewritten from fingerprint version {} to {} in a change that also touches {} other file(s) (e.g. `{}`); a migration is only verifiable on its own.",
                        b.version,
                        h.version,
                        others.len(),
                        others[0]
                    ),
                    "Commit `discipline baseline --migrate` in a change of its own, or justify it on its own line in the PR body or a commit message: `allow-gate-weakening: baseline <reason>`.",
                );
                return Ok(out);
            }
        }
    }

    if let Some(ref h_base) = head_baseline {
        let b_count = base_baseline
            .as_ref()
            .map(|b| b.findings.len())
            .unwrap_or(0);
        let h_count = h_base.findings.len();

        let base_fps: std::collections::HashSet<&str> = base_baseline
            .as_ref()
            .map(|b| b.findings.iter().map(|f| f.fingerprint.as_str()).collect())
            .unwrap_or_default();

        let new_fps: Vec<&str> = h_base
            .findings
            .iter()
            .map(|f| f.fingerprint.as_str())
            .filter(|fp| !base_fps.contains(fp))
            .collect();

        if h_count > b_count || !new_fps.is_empty() {
            // The finding the else-branches below report: new findings under an unchanged
            // count, else a grown baseline.
            let lifts = if !new_fps.is_empty() && h_count <= b_count {
                &crate::findings::BASELINE_NEW_FINDINGS
            } else {
                &crate::findings::BASELINE_INCREASED
            };
            if let Some(record) = ctx
                .find_override(GATE, lifts, tokens::ALLOW_GATE_WEAKENING, "baseline")
                .or_else(|| {
                    ctx.find_override(GATE, lifts, tokens::ALLOW_GATE_WEAKENING, baseline_filename)
                })
            {
                out.overrides.push(record);
            } else if !new_fps.is_empty() && h_count <= b_count {
                let count = new_fps.len();
                let sample = new_fps.first().unwrap_or(&"");
                out.push(
                    ctx.overridable(severity),
                    &crate::findings::BASELINE_NEW_FINDINGS,
                    Some(baseline_filename),
                    None,
                    format!("[baseline] Grandfathered baseline contains {count} new fingerprint(s) not present on base (e.g. `{sample}`). A 1-for-1 replacement of grandfathered findings with new ones is forbidden."),
                    "Revert the baseline modification, or justify it on its own line in the PR body or a commit message: `allow-gate-weakening: baseline <reason>`.",
                );
            } else {
                let diff = h_count.saturating_sub(b_count);
                out.push(
                    ctx.overridable(severity),
                    &crate::findings::BASELINE_INCREASED,
                    Some(baseline_filename),
                    None,
                    format!("[baseline] Grandfathered baseline grew from {b_count} to {h_count} findings ({diff} new grandfathered findings)."),
                    "Revert the baseline growth, or justify it on its own line in the PR body or a commit message: `allow-gate-weakening: baseline <reason>`.",
                );
            }
        }
    }

    Ok(out)
}

/// The test file a snapshot belongs to, and the test names it records.
#[derive(Debug, PartialEq, Eq)]
pub struct SnapshotOwner {
    pub test_file: String,
    pub tests: Vec<String>,
}

/// Map a snapshot file to its test file and the tests it records: Jest
/// `__snapshots__/<file>.snap` (keys ``exports[`<title> 1`]``), insta
/// `snapshots/<crate>__<module>__<test>.snap` (`<module>.rs` beside the directory),
/// syrupy / pytest-snapshot `__snapshots__/<test_file>.ambr` (`# name: <test>` lines).
pub fn snapshot_owner(path: &str, content: &str) -> Option<SnapshotOwner> {
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    let (parent, snap_dir) = dir.rsplit_once('/').unwrap_or(("", dir));
    let join = |p: &str, f: &str| {
        if p.is_empty() {
            f.to_string()
        } else {
            format!("{p}/{f}")
        }
    };
    if snap_dir == "__snapshots__" {
        if let Some(stem) = name.strip_suffix(".snap") {
            // Jest: `foo.test.ts.snap` -> `foo.test.ts`; the keys carry the titles.
            let tests: Vec<String> = content
                .lines()
                .filter_map(|l| l.strip_prefix("exports[`"))
                // `<describe> <title> 1`: the trailing counter goes, the title stays whole.
                .filter_map(|l| l.split_once("`]").map(|(t, _)| t))
                .map(|t| t.rsplit_once(' ').map_or(t, |(t, _)| t).to_string())
                .collect();
            return Some(SnapshotOwner {
                test_file: join(parent, stem),
                tests,
            });
        }
        if let Some(stem) = name.strip_suffix(".ambr") {
            let tests: Vec<String> = content
                .lines()
                .filter_map(|l| l.strip_prefix("# name: "))
                .map(|t| t.split('[').next().unwrap_or(t).trim().to_string())
                .collect();
            return Some(SnapshotOwner {
                test_file: join(parent, &format!("{stem}.py")),
                tests,
            });
        }
        return None;
    }
    if snap_dir == "snapshots" {
        let stem = name.strip_suffix(".snap")?;
        // insta: `crate__module__test.snap` (a `-N` suffix numbers inline variants).
        let mut parts: Vec<&str> = stem.split("__").collect();
        let test = parts.pop()?;
        let test = test.rsplit_once('-').map_or(test, |(t, n)| {
            if n.chars().all(|c| c.is_ascii_digit()) {
                t
            } else {
                test
            }
        });
        let module = parts.last().copied().unwrap_or("lib");
        let test_file = if std::path::Path::new(&join(parent, &format!("{module}/mod.rs"))).exists()
        {
            join(parent, &format!("{module}/mod.rs"))
        } else {
            join(parent, &format!("{module}.rs"))
        };
        return Some(SnapshotOwner {
            test_file,
            tests: vec![test.to_string()],
        });
    }
    None
}

pub fn golden_output(ctx: &Context) -> Result<GateOutcome> {
    const GATE: &str = "golden-output";
    let settings = &ctx.config.gates.golden_output;
    let mut out = GateOutcome::new(GATE);

    let path_filter = PathFilter::new(&settings.paths)?;
    let exempt_filter = PathFilter::new(&settings.exempt_paths)?;

    let changed = ctx.git.changed_files()?;
    let is_golden = |f: &crate::gitctx::ChangedFile| {
        path_filter.matches(&f.path) || (!f.old_path.is_empty() && path_filter.matches(&f.old_path))
    };
    // Expected output rewritten while nothing that produces it changed: the shape of a
    // failing comparison "fixed" by regenerating the expectation. Prose and the gate's own
    // configuration do not produce output.
    let produces_output = |f: &crate::gitctx::ChangedFile| {
        let p = f.path.to_ascii_lowercase();
        !is_golden(f)
            && !p.ends_with(".md")
            && !p.ends_with(".markdown")
            && !p.ends_with(".txt")
            && !p.ends_with(".rst")
            && p != "discipline.toml"
    };
    let snapshot_only = !changed.iter().any(produces_output);
    for file in &changed {
        if file.kind == ChangeKind::Added {
            // An added snapshot is fine for a new test; for a test that already existed it
            // is an expectation written after the fact.
            let matches_target = path_filter.matches(&file.path);
            if !matches_target || exempt_filter.matches(&file.path) {
                continue;
            }
            let Some(head) = ctx.git.head_content(&file.path)? else {
                continue;
            };
            let Some(owner) = snapshot_owner(&file.path, &head) else {
                continue;
            };
            // The owning test file must already exist on the base side.
            if ctx.git.base_content(&owner.test_file)?.is_none() {
                continue;
            }
            out.examined += 1;
            let owner_file = changed.iter().find(|f| f.path == owner.test_file);
            let added_in_owner: Vec<String> = match owner_file {
                Some(f) => ctx
                    .git
                    .head_content(&f.path)?
                    .map(|content| {
                        content
                            .lines()
                            .enumerate()
                            .filter(|(i, _)| f.added_lines.contains(&(i + 1)))
                            .map(|(_, l)| l.to_string())
                            .collect()
                    })
                    .unwrap_or_default(),
                None => Vec::new(),
            };
            let unmatched: Vec<&String> = owner
                .tests
                .iter()
                .filter(|t| !added_in_owner.iter().any(|l| l.contains(t.as_str())))
                .collect();
            if unmatched.is_empty() {
                continue;
            }
            if let Some(ov) = ctx.find_override(
                GATE,
                &crate::findings::SNAPSHOT_ADDED_FOR_EXISTING_TEST,
                tokens::ALLOW_GOLDEN_UPDATE,
                &file.path,
            ) {
                out.overrides.push(ov);
                continue;
            }
            out.push(
                ctx.overridable(settings.severity()),
                &crate::findings::SNAPSHOT_ADDED_FOR_EXISTING_TEST,
                Some(&file.path),
                None,
                format!(
                    "Snapshot `{}` is new, but the test(s) it records ({}) already existed in `{}` and this change does not add them: the expectation was written after the behaviour.",
                    file.path,
                    unmatched.iter().map(|t| format!("`{t}`")).collect::<Vec<_>>().join(", "),
                    owner.test_file
                ),
                "Confirm the recorded output is the intended one, then record it: `allow-golden-update: <path> <reason>`.",
            );
            continue;
        }

        let matches_target = path_filter.matches(&file.path)
            || (!file.old_path.is_empty() && path_filter.matches(&file.old_path));
        if !matches_target {
            continue;
        }

        let is_exempt = exempt_filter.matches(&file.path)
            || (!file.old_path.is_empty() && exempt_filter.matches(&file.old_path));
        if is_exempt {
            continue;
        }

        out.examined += 1;

        // The finding reported below without a directive.
        let golden = if snapshot_only {
            &crate::findings::GOLDEN_REGENERATED_WITHOUT_SOURCE_CHANGE
        } else {
            &crate::findings::GOLDEN_CHANGED_WITHOUT_DIRECTIVE
        };
        if let Some(ov) = ctx.find_override(GATE, golden, tokens::ALLOW_GOLDEN_UPDATE, &file.path) {
            out.overrides.push(ov);
            continue;
        }
        if !file.old_path.is_empty() && file.old_path != file.path {
            if let Some(ov) =
                ctx.find_override(GATE, golden, tokens::ALLOW_GOLDEN_UPDATE, &file.old_path)
            {
                out.overrides.push(ov);
                continue;
            }
        }

        let action = match file.kind {
            ChangeKind::Deleted => "deleted",
            _ => "modified",
        };

        let (title, context) = if snapshot_only {
            (
                &crate::findings::GOLDEN_REGENERATED_WITHOUT_SOURCE_CHANGE,
                " No file that produces output changed in this diff, so the expectation was rewritten to match existing behaviour.",
            )
        } else {
            (&crate::findings::GOLDEN_CHANGED_WITHOUT_DIRECTIVE, "")
        };
        out.push(
            ctx.overridable(settings.severity()),
            title,
            Some(&file.path),
            None,
            format!(
                "Committed golden/snapshot file `{}` was {action} ({} line(s) rewritten) without an explicit override.{context}",
                file.path,
                file.added_lines.len()
            ),
            "Provide a scoped override on its own line in the PR body or a commit message: \
             `allow-golden-update: <path-or-prefix> <reason>` (or `discipline:allow(golden-output): ...`).",
        );
    }

    Ok(out)
}

pub fn diff_configs(base: &DisciplineConfig, head: &DisciplineConfig) -> Result<Vec<Weakening>> {
    let mut found = Vec::new();

    // Check [directives] table
    let mut dir_note = |w: Weakening| found.push(w);
    let dir = |key: &str, change: Change| Weakening::new("directives", key, change);
    if !base.directives.allow_hidden && head.directives.allow_hidden {
        dir_note(dir("allow_hidden", Change::Changed).values(false, true));
    }
    let gained_sources: Vec<_> = head
        .directives
        .sources
        .iter()
        .filter(|s| !base.directives.sources.contains(s))
        .collect();
    if gained_sources.iter().any(|s| s.as_str() == "commits") {
        let mut w = dir("sources", Change::Gained).count(1);
        w.after = Some("commits".to_string());
        dir_note(w);
    }
    if base.directives.fail_on_overrides && !head.directives.fail_on_overrides {
        dir_note(dir("fail_on_overrides", Change::Changed).values(true, false));
    }
    // A listed actor is exempt from `fail_on_overrides`.
    let gained_actors = head
        .directives
        .allowed_override_actors
        .iter()
        .filter(|a| !base.directives.allowed_override_actors.contains(a))
        .count();
    if gained_actors > 0 {
        dir_note(dir("allowed_override_actors", Change::Gained).count(gained_actors));
    }

    match (base.directives.max_overrides, head.directives.max_overrides) {
        (Some(b), None) => dir_note(dir("max_overrides", Change::Removed).was(b)),
        (Some(b), Some(h)) if h > b => {
            dir_note(dir("max_overrides", Change::Increased).values(b, h))
        }
        _ => {}
    }
    match (
        base.directives.max_inline_overrides,
        head.directives.max_inline_overrides,
    ) {
        (Some(b), None) => dir_note(dir("max_inline_overrides", Change::Removed).was(b)),
        (Some(b), Some(h)) if h > b => {
            dir_note(dir("max_inline_overrides", Change::Increased).values(b, h))
        }
        _ => {}
    }
    if base.directives.require_approval && !head.directives.require_approval {
        dir_note(dir("require_approval", Change::Changed).values(true, false));
    }
    if !base.directives.degrade_offline && head.directives.degrade_offline {
        let mut w = dir("degrade_offline", Change::Changed).values(false, true);
        w.note = Some("a failed merged-pr-body lookup no longer stops the run");
        dir_note(w);
    }
    // (true -> false is stricter: a failed lookup then stops the run.)

    // [tests]: widening what counts as test code narrows what the production-code gates see.
    for (key, b, h) in [
        ("functions", &base.tests.functions, &head.tests.functions),
        ("paths", &base.tests.paths, &head.tests.paths),
    ] {
        let gained = h.iter().filter(|x| !b.contains(x)).count();
        if gained > 0 {
            found.push(Weakening::new("tests", key, Change::Gained).count(gained));
        }
    }

    // [languages.c]: a macro blanked before parsing is code no gate reads.
    for (key, b, h) in [
        (
            "c.macros",
            &base.languages.c.macros,
            &head.languages.c.macros,
        ),
        (
            "c.function_macros",
            &base.languages.c.function_macros,
            &head.languages.c.function_macros,
        ),
    ] {
        let gained = h.iter().filter(|x| !b.contains(x)).count();
        if gained > 0 {
            found.push(Weakening::new("languages", key, Change::Gained).count(gained));
        }
    }

    // [meta]: advisory mode exits 0 whatever the gates found.
    if base.meta.mode == RunMode::Enforcing && head.meta.mode == RunMode::Advisory {
        found.push(Weakening::new("meta", "mode", Change::Changed).values("enforcing", "advisory"));
    }

    let base_v = Value::try_from(&base.gates)?;
    let head_v = Value::try_from(&head.gates)?;
    let (Some(base_t), Some(head_t)) = (base_v.as_table(), head_v.as_table()) else {
        return Ok(found);
    };

    for (gate, base_gate) in base_t {
        let (Some(b), Some(h)) = (
            base_gate.as_table(),
            head_t.get(gate).and_then(Value::as_table),
        ) else {
            continue;
        };
        let mut note = |w: Weakening| found.push(w);
        let w = |key: &str, change: Change| Weakening::new(gate, key, change);
        for (key, bv) in b {
            // An option this binary does not know is judged as a plain switch, so a
            // stale table degrades to the strict reading rather than to silence.
            let dir = direction_of(key).unwrap_or(Direction::LooserWhenFalse);
            // Only an optional key can be absent on the head side. Its removal is a
            // weakening when the absent reading is looser than the value removed.
            let Some(hv) = h.get(key) else {
                match dir {
                    Direction::Evidence | Direction::Floor | Direction::Cap => {
                        note(w(key, Change::Removed).was(bv))
                    }
                    // Absent means no limit at all (`max_noise_cv`: no noise check).
                    Direction::Tolerance if ABSENT_IS_UNLIMITED.contains(&key.as_str()) => {
                        note(w(key, Change::Removed).was(bv))
                    }
                    // A gate's `allow_hidden = false` removed: the gate then inherits
                    // `[directives] allow_hidden`, a loosening when that is `true`.
                    Direction::LooserWhenTrue
                        if *bv == Value::Boolean(false)
                            && key == "allow_hidden"
                            && head.directives.allow_hidden =>
                    {
                        note(w(key, Change::Removed).was(bv))
                    }
                    _ => {}
                }
                continue;
            };
            let num = |v: &Value| match v {
                Value::Integer(i) => Some(*i as f64),
                Value::Float(f) => Some(*f),
                _ => None,
            };
            match dir {
                Direction::Evidence if bv != hv => note(w(key, Change::Changed).values(bv, hv)),
                Direction::StrictMode(strict) => {
                    if let (Value::String(bs), Value::String(hs)) = (bv, hv) {
                        if bs == strict && hs != strict {
                            note(w(key, Change::Changed).values(bs, hs));
                        }
                    }
                }
                Direction::Ordered(order) => {
                    if let (Value::String(bs), Value::String(hs)) = (bv, hv) {
                        let rank = |v: &str| order.iter().position(|o| *o == v);
                        if let (Some(b), Some(h)) = (rank(bs), rank(hs)) {
                            if h > b {
                                note(w(key, Change::Changed).values(bs, hs));
                            }
                        }
                    }
                }
                Direction::LooserWhenFalse
                    if matches!((bv, hv), (Value::Boolean(true), Value::Boolean(false))) =>
                {
                    note(w(key, Change::Changed).values(true, false))
                }
                Direction::LooserWhenTrue
                    if matches!((bv, hv), (Value::Boolean(false), Value::Boolean(true))) =>
                {
                    note(w(key, Change::Changed).values(false, true))
                }
                Direction::Floor => {
                    if let (Some(bn), Some(hn)) = (num(bv), num(hv)) {
                        if hn < bn {
                            note(w(key, Change::Decreased).values(bv, hv));
                        }
                    }
                }
                Direction::Cap | Direction::Tolerance => {
                    if let (Some(bn), Some(hn)) = (num(bv), num(hv)) {
                        if hn > bn {
                            note(w(key, Change::Increased).values(bv, hv));
                        }
                    }
                }
                Direction::Severity => {
                    if let (Value::String(bs), Value::String(hs)) = (bv, hv) {
                        if (bs == "error" && (hs == "warning" || hs == "note"))
                            || (bs == "warning" && hs == "note")
                        {
                            note(w(key, Change::Lowered).values(bs, hs));
                        }
                    }
                }
                Direction::Grown | Direction::Shrunk | Direction::Allowlist => {
                    let (Value::Array(ba), Value::Array(ha)) = (bv, hv) else {
                        continue;
                    };
                    let gained = ha.iter().filter(|x| !ba.contains(x)).count();
                    let lost = ba
                        .iter()
                        .filter(|x| !ha.contains(x) && !entry_tightened(key, x, ba, ha))
                        .count();
                    if dir == Direction::Allowlist && !ba.is_empty() && ha.is_empty() {
                        note(w(key, Change::Emptied));
                    } else if dir == Direction::Allowlist && ba.is_empty() {
                        // No allow-list on base: adopting one is a tightening.
                    } else if dir != Direction::Shrunk && gained > 0 {
                        note(w(key, Change::Gained).count(gained));
                    } else if dir == Direction::Shrunk && lost > 0 {
                        note(w(key, Change::Lost).count(lost));
                    }
                }
                _ => {}
            }
        }
        // A key only on head: unset `ci_skip_severity` means the gate's `severity`. No other
        // optional gate key can loosen by being added (an added cap only tightens).
        for (key, hv) in h {
            if b.contains_key(key) {
                continue;
            }
            if gate == "ignored-tests" && key == "ci_skip_severity" {
                if let (Some(Value::String(bs)), Value::String(hs)) = (b.get("severity"), hv) {
                    if (bs == "error" && (hs == "warning" || hs == "note"))
                        || (bs == "warning" && hs == "note")
                    {
                        note(w(key, Change::Lowered).values(bs, hs));
                    }
                }
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {

    #[test]
    fn removing_an_optional_key_is_judged_by_what_absent_means() {
        let cfg = |body: &str| {
            DisciplineConfig::from_toml_str(&format!("[meta]\nversion = 1\nname = \"t\"\n{body}"))
                .unwrap()
        };
        let keys = |base: &str, head: &str| -> Vec<String> {
            diff_configs(&cfg(base), &cfg(head))
                .unwrap()
                .iter()
                .map(|w| format!("{}.{}", w.gate, w.key()))
                .collect()
        };
        // `max_noise_cv` absent is no noise check: a weakening.
        assert_eq!(
            keys(
                "[gates.bench-regression]\nmax_noise_cv = 0.05\n",
                "[gates.bench-regression]\n"
            ),
            ["bench-regression.max_noise_cv"]
        );
        // `noise_margin_pct` absent is 0: stricter, not reported.
        assert!(keys(
            "[gates.bench-regression]\nnoise_margin_pct = 2.0\n",
            "[gates.bench-regression]\n"
        )
        .is_empty());
        // A gate's `allow_hidden = false` removed loosens only when the global is `true`.
        let hidden = |global: bool, gate: &str| {
            format!("[directives]\nallow_hidden = {global}\n[gates.deletion-rationale]\n{gate}")
        };
        assert_eq!(
            keys(&hidden(true, "allow_hidden = false\n"), &hidden(true, "")),
            ["deletion-rationale.allow_hidden"]
        );
        assert!(keys(&hidden(false, "allow_hidden = false\n"), &hidden(false, "")).is_empty());
        assert!(keys(&hidden(true, "allow_hidden = true\n"), &hidden(true, "")).is_empty());
        // Every listed key is a real optional tolerance.
        for k in ABSENT_IS_UNLIMITED {
            assert_eq!(direction_of(k), Some(Direction::Tolerance), "{k}");
        }
    }

    use super::*;

    fn cfg(body: &str) -> DisciplineConfig {
        DisciplineConfig::from_toml_str(&format!("[meta]\nversion = 1\nname = \"t\"\n{body}"))
            .unwrap()
    }

    #[test]
    fn ratification_policy_loosening_is_a_weakening_and_tightening_is_not() {
        use crate::config::{AcceptEdited, RatificationWindow};
        let mut base = DisciplineConfig::default_for_repo("t");
        base.gates.ratified_paths.enabled = true;
        base.gates.ratified_paths.protected_paths = vec!["scripts/**".into()];
        base.gates.ratified_paths.ratifiers = vec!["owner".into()];
        base.gates.ratified_paths.ratification_max_age_days = Some(30);
        let weaker = |edit: &dyn Fn(&mut crate::config::RatifiedPathsGate)| {
            let mut head = base.clone();
            edit(&mut head.gates.ratified_paths);
            diff_configs(&base, &head).unwrap()
        };
        // The window is ordered: pull-created, path-last-changed, any.
        assert_eq!(
            weaker(&|g| g.ratification_valid_from = RatificationWindow::Any).len(),
            1
        );
        assert!(
            weaker(&|g| g.ratification_valid_from = RatificationWindow::PullCreated).is_empty()
        );
        assert_eq!(weaker(&|g| g.ratification_max_age_days = None).len(), 1);
        assert_eq!(weaker(&|g| g.ratification_max_age_days = Some(90)).len(), 1);
        assert!(weaker(&|g| g.ratification_max_age_days = Some(7)).is_empty());
        assert_eq!(weaker(&|g| g.ratifiers.push("helper".into())).len(), 1);
        assert_eq!(weaker(&|g| g.protected_paths.clear()).len(), 1);
        assert_eq!(
            weaker(&|g| g.never_ratifiable.pop().map(|_| ()).unwrap_or(())).len(),
            1
        );
        assert_eq!(
            weaker(&|g| g.accept_edited = AcceptEdited::ByAuthor).len(),
            1
        );
        assert_eq!(weaker(&|g| g.require_open_issue = false).len(), 1);
        assert!(weaker(&|g| g.agent_logins.push("bot".into())).is_empty());
        // Turning on `refuse_author_ratification` tightens; turning it off loosens.
        assert!(weaker(&|g| g.refuse_author_ratification = true).is_empty());
        let mut on = base.clone();
        on.gates.ratified_paths.refuse_author_ratification = true;
        assert_eq!(diff_configs(&on, &base).unwrap().len(), 1);
    }

    #[test]
    fn each_weakening_carries_its_key_change_and_values_as_data() {
        let base = cfg(
            "[directives]\nmax_overrides = 1\nallowed_override_actors = [\"lead\"]\n\
             [gates.pii]\nexempt_paths = [\"a/**\"]\n\
             [gates.test-floor]\nenabled = true\nmin_tests = 40\n",
        );
        let head = cfg(
            "[directives]\nallowed_override_actors = [\"lead\", \"someone\"]\n\
             [gates.pii]\nexempt_paths = [\"a/**\", \"b/**\", \"c/**\"]\nseverity = \"warning\"\n\
             [gates.test-floor]\nenabled = true\nmin_tests = 3\n",
        );
        let found = diff_configs(&base, &head).unwrap();
        let get = |gate: &str, key: &str| {
            found
                .iter()
                .find(|w| w.gate == gate && w.key == key)
                .unwrap_or_else(|| panic!("{gate}.{key} not in {found:?}"))
        };
        let removed = get("directives", "max_overrides");
        assert_eq!(
            (
                removed.change,
                removed.before.as_deref(),
                removed.after.as_deref()
            ),
            (Change::Removed, Some("1"), None)
        );
        assert_eq!(removed.what(), "`max_overrides` removed (was 1)");
        let actors = get("directives", "allowed_override_actors");
        assert_eq!((actors.change, actors.count), (Change::Gained, Some(1)));
        // A list entry is counted, never named: an actor is a login.
        let json = serde_json::to_string(&found).unwrap();
        assert!(!json.contains("someone"), "{json}");
        let paths = get("pii", "exempt_paths");
        assert_eq!((paths.change, paths.count), (Change::Gained, Some(2)));
        assert_eq!(paths.what(), "`exempt_paths` gained 2 entr(y/ies)");
        let sev = get("pii", "severity");
        assert_eq!(
            (sev.change, sev.before.as_deref(), sev.after.as_deref()),
            (Change::Lowered, Some("error"), Some("warning"))
        );
        let floor = get("test-floor", "min_tests");
        assert_eq!(
            (
                floor.change,
                floor.before.as_deref(),
                floor.after.as_deref()
            ),
            (Change::Decreased, Some("40"), Some("3"))
        );
        assert_eq!(floor.what(), "`min_tests` decreased from 40 to 3");
        assert!(json.contains(r#""change":"decreased""#), "{json}");
    }

    #[test]
    fn identical_and_stricter_configs_are_not_weakenings() {
        let base = cfg("[gates.pii]\nexempt_paths = [\"a/**\"]\n");
        assert!(diff_configs(&base, &base).unwrap().is_empty());
        let stricter = cfg("[gates.pii]\nhostname_denylist = [\"h\"]\n");
        assert!(diff_configs(&base, &stricter).unwrap().is_empty());
    }

    #[test]
    fn each_loosening_class_is_detected_and_attributed() {
        let base = cfg("[gates.pii]\nhostname_denylist = [\"h\"]\n");
        let head = cfg(
            "[gates.pii]\nlan_ips = false\nexempt_paths = [\"docs/**\"]\n\
             [gates.vacuous-tests]\nenabled = false\n\
             [gates.agent-scratch]\nseverity = \"warning\"\n",
        );
        let found = diff_configs(&base, &head).unwrap();
        let has = |gate: &str, needle: &str| {
            found
                .iter()
                .any(|w| w.gate == gate && w.what().contains(needle))
        };
        assert!(has("pii", "`lan_ips` changed from true to false"));
        assert!(has("pii", "`exempt_paths` gained 1"));
        assert!(has("pii", "`hostname_denylist` lost 1"));
        assert!(has("vacuous-tests", "`enabled` changed from true to false"));
        assert!(has("agent-scratch", "`severity` lowered"));
        assert_eq!(found.len(), 5, "{found:?}");
    }

    #[test]
    fn a_dropped_or_edited_banned_action_is_a_loosening_and_an_added_one_is_not() {
        let base = cfg(
            "[gates.ci-integrity]\nbanned_actions = [\"a/b\", { uses = \"c/d@v1\", reason = \"x\" }]\n",
        );
        let dropped = cfg("[gates.ci-integrity]\nbanned_actions = [\"a/b\"]\n");
        let narrowed = cfg(
            "[gates.ci-integrity]\nbanned_actions = [\"a/b@v1\", { uses = \"c/d@v1\", reason = \"x\" }]\n",
        );
        let added = cfg(
            "[gates.ci-integrity]\nbanned_actions = [\"a/b\", { uses = \"c/d@v1\", reason = \"x\" }, \"e/f\"]\n",
        );
        let lost = |head: &DisciplineConfig| {
            diff_configs(&base, head)
                .unwrap()
                .iter()
                .any(|w| w.gate == "ci-integrity" && w.what().contains("`banned_actions` lost"))
        };
        assert!(lost(&dropped));
        assert!(lost(&narrowed));
        assert!(diff_configs(&base, &added).unwrap().is_empty());
        assert!(diff_configs(&base, &base).unwrap().is_empty());
    }

    #[test]
    fn integer_and_security_boolean_loosening_detected() {
        let base = cfg("[gates.command]\nmin_count = 10\n[gates.unsafe-budget]\nmax_unsafe = 5\n[gates.pii]\ndiff_only = false\n");
        let head = cfg("[gates.command]\nmin_count = 5\n[gates.unsafe-budget]\nmax_unsafe = 10\n[gates.pii]\ndiff_only = true\n");
        let found = diff_configs(&base, &head).unwrap();
        let has = |gate: &str, needle: &str| {
            found
                .iter()
                .any(|w| w.gate == gate && w.what().contains(needle))
        };
        assert!(has("command", "`min_count` decreased from 10 to 5"));
        assert!(has("unsafe-budget", "`max_unsafe` increased from 5 to 10"));
        assert!(has("pii", "`diff_only` changed from false to true"));
    }

    #[test]
    fn evidence_references_and_strict_modes_cannot_be_dropped_silently() {
        let base = cfg(
            "[gates.provenance-tags]\nsuperseded_registry = \"reg.json\"\n\
             superseded_json_paths = [\"docs/**/*.json\"]\nrequire_open_pending_issues = true\n\
             [gates.bench-regression]\nmode = \"paired-ratio\"\nratio_baseline = \"b.json\"\n\
             citation_source_paths = [\"src\"]\n",
        );
        let removed = cfg("[gates.bench-regression]\nratio_baseline = \"other.json\"\n");
        let found = diff_configs(&base, &removed).unwrap();
        let has = |gate: &str, needle: &str| {
            found
                .iter()
                .any(|w| w.gate == gate && w.what().contains(needle))
        };
        assert!(
            has("provenance-tags", "`superseded_registry` removed"),
            "{found:?}"
        );
        assert!(
            has("provenance-tags", "`superseded_json_paths` lost 1"),
            "{found:?}"
        );
        assert!(
            has(
                "provenance-tags",
                "`require_open_pending_issues` changed from true to false"
            ),
            "{found:?}"
        );
        assert!(
            has("bench-regression", "`mode` changed from paired-ratio"),
            "{found:?}"
        );
        assert!(
            has("bench-regression", "`ratio_baseline` changed from"),
            "{found:?}"
        );
        assert!(
            has("bench-regression", "`citation_source_paths` lost 1"),
            "{found:?}"
        );

        // Keys added with ci-skip-set and bench rigor.
        let skip_base = cfg("[gates.ci-skip-set]\nunconditional_jobs = [\"lint\"]\n\
             [gates.bench-regression]\nratio_tolerance_pct = 2.0\nexempt_arms = [\"a\"]\n");
        let skip_head = cfg(
            "[gates.ci-skip-set]\nunconditional_jobs = []\nchange_job = \"\"\nworkflow = \"x.yml\"\n\
             [gates.bench-regression]\nratio_tolerance_pct = 9.5\nexempt_arms = [\"a\", \"*\"]\n",
        );
        let skip_found = diff_configs(&skip_base, &skip_head).unwrap();
        let has = |gate: &str, needle: &str| {
            skip_found
                .iter()
                .any(|w| w.gate == gate && w.what().contains(needle))
        };
        let found = &skip_found;
        assert!(
            has("ci-skip-set", "`unconditional_jobs` lost 1"),
            "{found:?}"
        );
        assert!(has("ci-skip-set", "`change_job` changed"), "{found:?}");
        assert!(has("ci-skip-set", "`workflow` changed"), "{found:?}");
        assert!(
            has("bench-regression", "`ratio_tolerance_pct` increased"),
            "{found:?}"
        );
        assert!(
            has("bench-regression", "`exempt_arms` gained 1"),
            "{found:?}"
        );

        // Adopting the registry or the stricter mode is not a weakening.
        let plain = cfg("");
        assert!(diff_configs(&plain, &base).unwrap().is_empty());
    }

    fn lockstep(group: &str, sources: &[(&str, &str)]) -> DisciplineConfig {
        let mut body = format!("[[gates.version-lockstep.groups]]\nname = \"{group}\"\n");
        for (path, regex) in sources {
            body.push_str(&format!(
                "[[gates.version-lockstep.groups.sources]]\npath = \"{path}\"\nregex = '{regex}'\n"
            ));
        }
        cfg(&body)
    }

    #[test]
    fn a_list_entry_is_matched_by_identity_and_only_a_tightening_edit_passes() {
        let base = lockstep(
            "release",
            &[("Cargo.toml", "v(.+)"), ("README.md", "@v(.+)")],
        );
        let whats = |head: &DisciplineConfig| -> Vec<String> {
            diff_configs(&base, head)
                .unwrap()
                .into_iter()
                .map(|w| format!("{}: {}", w.gate, w.what()))
                .collect()
        };
        let lost = vec!["version-lockstep: `groups` lost 1 entr(y/ies)".to_string()];
        // A source added to the same group, every base source kept: a tightening.
        let added = lockstep(
            "release",
            &[
                ("Cargo.toml", "v(.+)"),
                ("CITATION.cff", "version: (.+)"),
                ("README.md", "@v(.+)"),
            ],
        );
        assert!(whats(&added).is_empty(), "{:?}", whats(&added));
        // A source removed.
        assert_eq!(
            whats(&lockstep("release", &[("Cargo.toml", "v(.+)")])),
            lost
        );
        // A source's regex edited, even alongside an added source.
        let edited = lockstep(
            "release",
            &[
                ("Cargo.toml", "v(.+)"),
                ("README.md", "(.+)"),
                ("CITATION.cff", "version: (.+)"),
            ],
        );
        assert_eq!(whats(&edited), lost);
        // The group renamed: the base group is gone.
        let renamed = lockstep(
            "rel",
            &[
                ("Cargo.toml", "v(.+)"),
                ("README.md", "@v(.+)"),
                ("CITATION.cff", "version: (.+)"),
            ],
        );
        assert_eq!(whats(&renamed), lost);
        // Two head groups under the base name: which one kept the sources is ambiguous.
        // Two groups, one grows: each base group is found by its name, not its position.
        let mut two = base.clone();
        let mut docs = lockstep("docs", &[("a.md", "(.+)"), ("b.md", "(.+)")])
            .gates
            .version_lockstep
            .groups;
        two.gates.version_lockstep.groups.splice(0..0, docs.clone());
        docs.extend(added.gates.version_lockstep.groups.clone());
        let mut two_added = base.clone();
        two_added.gates.version_lockstep.groups = docs;
        assert!(diff_configs(&two, &two_added).unwrap().is_empty());
        let mut dup = added.clone();
        dup.gates
            .version_lockstep
            .groups
            .push(added.gates.version_lockstep.groups[0].clone());
        assert_eq!(whats(&dup), lost);
    }

    #[test]
    fn manifest_sync_rules_and_command_entries_follow_the_same_identity_rule() {
        let rules = |watched: &str, exclude: &str, regex: &str| {
            cfg(&format!(
                "[[gates.manifest-sync.rules]]\nmanifest = \"m.json\"\nextract_regex = '{regex}'\n\
                 watched_paths = [{watched}]\nexclude_paths = [{exclude}]\n"
            ))
        };
        let base = rules("\"src/**\"", "\"src/gen/**\", \"src/x/**\"", "n(.+)");
        let count = |head: &DisciplineConfig| diff_configs(&base, head).unwrap().len();
        // A watched path added and an exclusion dropped: both tighten.
        assert_eq!(
            count(&rules("\"src/**\", \"lib/**\"", "\"src/gen/**\"", "n(.+)")),
            0
        );
        // A watched path dropped, an exclusion added, the regex edited: each loosens.
        assert_eq!(
            count(&rules("", "\"src/gen/**\", \"src/x/**\"", "n(.+)")),
            1
        );
        assert_eq!(
            count(&rules(
                "\"src/**\"",
                "\"src/gen/**\", \"src/x/**\", \"a/**\"",
                "n(.+)"
            )),
            1
        );
        assert_eq!(
            count(&rules("\"src/**\"", "\"src/gen/**\", \"src/x/**\"", "(.+)")),
            1
        );

        let cmd = |forbid: &str, min: u64| {
            cfg(&format!(
                "[[gates.command.commands]]\nname = \"t\"\ncommand = \"cargo test\"\n\
                 min_count = {min}\nforbid_output = [{forbid}]\n"
            ))
        };
        let base = cmd("\"panicked\"", 5);
        let count = |head: &DisciplineConfig| diff_configs(&base, head).unwrap().len();
        assert_eq!(count(&cmd("\"panicked\", \"ignored\"", 5)), 0);
        assert_eq!(count(&cmd("", 5)), 1);
        // A field outside the list is judged whole: even a raised floor reads as lost.
        assert_eq!(count(&cmd("\"panicked\"", 9)), 1);

        // A snapshot added to an entry tightens; repointed or removed, it loosens. Fewer
        // ignored lines tighten; one more, or an edited pattern, loosens, even when it
        // arrives with the snapshot (a preset can supply the snapshot itself).
        let snap = |extra: &str| {
            cfg(&format!(
                "[[gates.command.commands]]\nname = \"t\"\ncommand = \"cargo test\"\n{extra}"
            ))
        };
        let plain = snap("");
        let with = snap("snapshot = \"api.txt\"\nsnapshot_ignore = ['^#', '^//']\n");
        let count = |base: &DisciplineConfig, head: &DisciplineConfig| {
            diff_configs(base, head).unwrap().len()
        };
        assert_eq!(count(&plain, &snap("snapshot = \"api.txt\"\n")), 0);
        assert_eq!(count(&plain, &with), 1);
        assert_eq!(count(&with, &plain), 1);
        assert_eq!(
            count(
                &with,
                &snap("snapshot = \"other.txt\"\nsnapshot_ignore = ['^#', '^//']\n")
            ),
            1
        );
        assert_eq!(
            count(
                &with,
                &snap("snapshot = \"api.txt\"\nsnapshot_ignore = ['^#']\n")
            ),
            0
        );
        assert_eq!(
            count(
                &with,
                &snap("snapshot = \"api.txt\"\nsnapshot_ignore = ['^#', '^//', '.*']\n")
            ),
            1
        );
        assert_eq!(
            count(
                &with,
                &snap("snapshot = \"api.txt\"\nsnapshot_ignore = ['^#', '.*']\n")
            ),
            1
        );
    }

    #[test]
    fn command_table_snapshot_keys_are_directional() {
        let top = |extra: &str| cfg(&format!("[gates.command]\ncommand = \"x\"\n{extra}"));
        let keys = |base: &str, head: &str| -> Vec<String> {
            diff_configs(&top(base), &top(head))
                .unwrap()
                .iter()
                .map(|w| w.key.clone())
                .collect()
        };
        let with = "snapshot = \"api.txt\"\nsnapshot_ignore = ['^#']\n";
        assert!(keys("", "snapshot = \"api.txt\"\n").is_empty());
        assert_eq!(keys("", with), ["snapshot_ignore"]);
        assert_eq!(keys(with, "snapshot_ignore = ['^#']\n"), ["snapshot"]);
        assert_eq!(
            keys(with, "snapshot = \"b.txt\"\nsnapshot_ignore = ['^#']\n"),
            ["snapshot"]
        );
        assert_eq!(
            keys(with, "snapshot = \"api.txt\"\nsnapshot_ignore = ['.*']\n"),
            ["snapshot_ignore"]
        );
        assert!(keys(with, "snapshot = \"api.txt\"\n").is_empty());
    }

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

    #[test]
    fn snapshot_files_map_to_their_test_file_and_tests() {
        let jest = snapshot_owner(
            "web/src/__snapshots__/app.test.tsx.snap",
            "exports[`renders the header 1`] = `<h1/>`;\n\nexports[`renders the footer 1`] = `<p/>`;\n",
        )
        .unwrap();
        assert_eq!(jest.test_file, "web/src/app.test.tsx");
        assert_eq!(jest.tests, vec!["renders the header", "renders the footer"]);
        let insta = snapshot_owner(
            "crates/x/src/snapshots/x__parser__parses_empty-2.snap",
            "---\n",
        )
        .unwrap();
        assert_eq!(insta.test_file, "crates/x/src/parser.rs");
        assert_eq!(insta.tests, vec!["parses_empty"]);
        let ambr = snapshot_owner(
            "tests/__snapshots__/test_api.ambr",
            "# name: test_get\n  'x'\n# ---\n# name: test_put[1]\n  'y'\n",
        )
        .unwrap();
        assert_eq!(ambr.test_file, "tests/test_api.py");
        assert_eq!(ambr.tests, vec!["test_get", "test_put"]);
        assert!(snapshot_owner("tests/golden/out.txt", "").is_none());
    }

    #[test]
    fn every_directives_option_is_judged() {
        // `diff_configs` reads [directives] field by field; a new option must be added
        // there and here.
        const JUDGED: &[&str] = &[
            "sources",
            "allow_hidden",
            "fail_on_overrides",
            "allowed_override_actors",
            "max_overrides",
            "max_inline_overrides",
            "require_approval",
            "degrade_offline",
        ];
        let schema = crate::schema::generate_schema();
        let props = schema["properties"]["directives"]["properties"]
            .as_object()
            .unwrap();
        let unjudged: Vec<_> = props
            .keys()
            .filter(|k| !JUDGED.contains(&k.as_str()))
            .collect();
        assert!(
            unjudged.is_empty(),
            "judge these in diff_configs: {unjudged:?}"
        );
        assert_eq!(props.len(), JUDGED.len());
    }

    #[test]
    fn a_raised_or_removed_override_budget_and_dropped_approval_are_weakenings() {
        let cfg = |d: &str| {
            DisciplineConfig::from_toml_str(&format!(
                "[meta]\nversion = 1\nname = \"t\"\n[directives]\n{d}"
            ))
            .unwrap()
        };
        let base = cfg("max_overrides = 1\nmax_inline_overrides = 2\nrequire_approval = true\n");
        let whats = |head: &DisciplineConfig| -> Vec<String> {
            diff_configs(&base, head)
                .unwrap()
                .into_iter()
                .map(|w| w.what())
                .collect()
        };
        assert_eq!(
            whats(&cfg("max_overrides = 4\nmax_inline_overrides = 5\n")),
            vec![
                "`max_overrides` increased from 1 to 4",
                "`max_inline_overrides` increased from 2 to 5",
                "`require_approval` changed from true to false"
            ]
        );
        assert_eq!(
            whats(&cfg("require_approval = true\n")),
            vec![
                "`max_overrides` removed (was 1)",
                "`max_inline_overrides` removed (was 2)"
            ]
        );
        assert!(whats(&cfg(
            "max_overrides = 0\nmax_inline_overrides = 1\nrequire_approval = true\n"
        ))
        .is_empty());
        // Switching the failed-lookup degrade on is a loosening; off is a tightening.
        let off = cfg("degrade_offline = false\n");
        let on = cfg("degrade_offline = true\n");
        let notes: Vec<String> = diff_configs(&off, &on)
            .unwrap()
            .into_iter()
            .map(|w| w.what())
            .collect();
        assert!(
            notes
                .iter()
                .any(|w| w.contains("`degrade_offline` changed from false to true")),
            "{notes:?}"
        );
        assert!(diff_configs(&on, &off).unwrap().is_empty());
        // Adopting either is a tightening.
        assert!(diff_configs(&cfg(""), &base).unwrap().is_empty());
    }

    #[test]
    fn advisory_mode_and_override_actors_are_weakenings() {
        let base = DisciplineConfig::from_toml_str(
            "[meta]\nversion = 1\nname = \"t\"\n[directives]\nallowed_override_actors = [\"lead\"]\n",
        )
        .unwrap();
        let head = DisciplineConfig::from_toml_str(
            "[meta]\nversion = 1\nname = \"t\"\nmode = \"advisory\"\n\
             [directives]\nallowed_override_actors = [\"lead\", \"bot\"]\n",
        )
        .unwrap();
        let found = diff_configs(&base, &head).unwrap();
        assert_eq!(
            found
                .iter()
                .map(|w| (w.gate.as_str(), w.what()))
                .collect::<Vec<_>>(),
            vec![
                (
                    "directives",
                    "`allowed_override_actors` gained 1 entr(y/ies)".to_string()
                ),
                (
                    "meta",
                    "`mode` changed from enforcing to advisory".to_string()
                ),
            ]
        );
        // Leaving advisory mode, or dropping an actor, tightens.
        assert!(diff_configs(&head, &base).unwrap().is_empty());
    }

    #[test]
    fn floors_caps_allowlists_and_commands_are_directional() {
        let base = cfg(
            "[gates.test-floor]\nmin_tests = 40\ntolerance = 0\ntest_command = \"cargo test\"\n\
             [gates.suppression-delta]\nmax_increase = 0\nallowed_suppressions = []\n\
             [gates.dependency-delta]\nallow_dependencies = [\"serde\"]\ndeny_dependencies = [\"openssl\"]\n\
             [gates.scope-confinement]\nforbidden_paths = [\"ci/**\"]\n\
             [gates.ci-integrity]\nworkflows = [\".github/workflows/*.yml\"]\n",
        );
        let head = cfg(
            "[gates.test-floor]\ntolerance = 5\ntest_command = \"true\"\n\
             [gates.suppression-delta]\nmax_increase = 9\nallowed_suppressions = [\"noqa\"]\n\
             [gates.dependency-delta]\nallow_dependencies = []\ndeny_dependencies = []\n\
             [gates.scope-confinement]\nforbidden_paths = []\n\
             [gates.ci-integrity]\nworkflows = []\n",
        );
        let found = diff_configs(&base, &head).unwrap();
        let has = |gate: &str, needle: &str| {
            found
                .iter()
                .any(|w| w.gate == gate && w.what().contains(needle))
        };
        assert!(has("test-floor", "`min_tests` removed"), "{found:?}");
        assert!(
            has("test-floor", "`tolerance` increased from 0 to 5"),
            "{found:?}"
        );
        assert!(has("test-floor", "`test_command` changed"), "{found:?}");
        assert!(
            has("suppression-delta", "`max_increase` increased"),
            "{found:?}"
        );
        assert!(
            has("suppression-delta", "`allowed_suppressions` gained 1"),
            "{found:?}"
        );
        assert!(
            has("dependency-delta", "`allow_dependencies` emptied"),
            "{found:?}"
        );
        assert!(
            has("dependency-delta", "`deny_dependencies` lost 1"),
            "{found:?}"
        );
        assert!(
            has("scope-confinement", "`forbidden_paths` lost 1"),
            "{found:?}"
        );
        assert!(has("ci-integrity", "`workflows` lost 1"), "{found:?}");
        assert_eq!(found.len(), 9, "{found:?}");

        // The reverse direction tightens everything except the repointed command: which
        // command is the stronger check is not something a diff can decide.
        let back = diff_configs(&head, &base).unwrap();
        assert_eq!(back.len(), 1, "{back:?}");
        assert!(
            back[0].what().contains("`test_command` changed"),
            "{back:?}"
        );
        // Adopting an allow-list where none existed is a tightening; growing one is not.
        let none = cfg("");
        let adopted = cfg("[gates.dependency-delta]\nallow_dependencies = [\"serde\"]\n");
        let grown =
            cfg("[gates.dependency-delta]\nallow_dependencies = [\"serde\", \"left-pad\"]\n");
        assert!(diff_configs(&none, &adopted).unwrap().is_empty());
        assert_eq!(diff_configs(&adopted, &grown).unwrap().len(), 1);
    }

    #[test]
    fn ci_skip_severity_added_on_head_is_detected_as_weakening() {
        // a. base "" , head "[gates.ignored-tests]\nci_skip_severity = \"note\"" -> 1 weakening naming ci_skip_severity
        let found_a = diff_configs(
            &cfg(""),
            &cfg("[gates.ignored-tests]\nci_skip_severity = \"note\"\n"),
        )
        .unwrap();
        assert_eq!(found_a.len(), 1);
        assert_eq!(found_a[0].gate, "ignored-tests");
        assert_eq!(found_a[0].key(), "ci_skip_severity");
        assert_eq!(found_a[0].change, Change::Lowered);
        assert_eq!(found_a[0].before.as_deref(), Some("error"));
        assert_eq!(found_a[0].after.as_deref(), Some("note"));

        // b. base ci_skip_severity="error", head "note" -> 1
        let found_b = diff_configs(
            &cfg("[gates.ignored-tests]\nci_skip_severity = \"error\"\n"),
            &cfg("[gates.ignored-tests]\nci_skip_severity = \"note\"\n"),
        )
        .unwrap();
        assert_eq!(found_b.len(), 1);
        assert_eq!(found_b[0].gate, "ignored-tests");
        assert_eq!(found_b[0].key(), "ci_skip_severity");

        // c. control: base "", head severity="note" -> 1
        let found_c = diff_configs(
            &cfg(""),
            &cfg("[gates.ignored-tests]\nseverity = \"note\"\n"),
        )
        .unwrap();
        assert_eq!(found_c.len(), 1);
        assert_eq!(found_c[0].gate, "ignored-tests");
        assert_eq!(found_c[0].key(), "severity");

        // d. base severity="warning", head severity="warning" + ci_skip_severity="error" -> 0
        let found_d = diff_configs(
            &cfg("[gates.ignored-tests]\nseverity = \"warning\"\n"),
            &cfg("[gates.ignored-tests]\nseverity = \"warning\"\nci_skip_severity = \"error\"\n"),
        )
        .unwrap();
        assert!(found_d.is_empty(), "expected 0 weakenings, got {found_d:?}");
    }
}
