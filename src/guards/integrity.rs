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
    /// List: a lost entry is looser. An edited entry counts as lost.
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
    /// `error` > `warning` > `note`.
    Severity,
    /// Does not move the bar, or is judged as part of its enclosing list entry.
    Neutral,
}

/// Every option accepted under `[gates.<id>]`, classified. An option missing from this
/// table is a hole in the gate: `every_gate_option_is_classified` fails until it is added.
pub const KEY_DIRECTIONS: &[(&str, Direction)] = &[
    // Common to every gate.
    ("enabled", Direction::LooserWhenFalse),
    ("severity", Direction::Severity),
    ("exempt_paths", Direction::Grown),
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
    ("mock_assert_fns", Direction::Shrunk),
    ("rules", Direction::Shrunk),
    ("groups", Direction::Shrunk),
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
    // entries (an edited entry already counts as a lost one).
    ("redact_lan_ips", Direction::Neutral),
    ("timeout_seconds", Direction::Neutral),
    ("strip_components", Direction::Neutral),
    ("provenance", Direction::Neutral),
    ("base_file", Direction::Neutral),
    ("head_file", Direction::Neutral),
    ("pinned_version", Direction::Neutral),
    ("args", Direction::Neutral),
    ("name", Direction::Neutral),
    ("job", Direction::Neutral),
    ("guard", Direction::Neutral),
];

pub fn direction_of(key: &str) -> Option<Direction> {
    KEY_DIRECTIONS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, d)| *d)
}

#[derive(Debug, PartialEq, Eq)]
pub struct Weakening {
    pub gate: String,
    pub what: String,
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
            .find_override("config-integrity", tokens::ALLOW_GATE_WEAKENING, "meta")
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
                    if let Some(record) =
                        ctx.find_override(GATE, tokens::ALLOW_GATE_WEAKENING, &w.gate)
                    {
                        out.overrides.push(record);
                        continue;
                    }
                    out.push(
                        ctx.overridable(severity),
                        &crate::findings::GATE_WEAKENED,
                        Some(ctx.config_path),
                        None,
                        format!("[{}] {}.", w.gate, w.what),
                        &format!(
                            "Revert the change, or justify it on its own line in the PR body or a commit \
                             message: `allow-gate-weakening: {} <reason>`.",
                            w.gate
                        ),
                    );
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
                if let Some(record) =
                    ctx.find_override(GATE, tokens::ALLOW_GATE_WEAKENING, "baseline")
                {
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
            if let Some(record) = ctx
                .find_override(GATE, tokens::ALLOW_GATE_WEAKENING, "baseline")
                .or_else(|| {
                    ctx.find_override(GATE, tokens::ALLOW_GATE_WEAKENING, baseline_filename)
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
            let added_in_owner: Vec<String> = changed
                .iter()
                .find(|f| f.path == owner.test_file)
                .and_then(|f| {
                    let content = ctx.git.head_content(&f.path).ok().flatten()?;
                    Some(
                        content
                            .lines()
                            .enumerate()
                            .filter(|(i, _)| f.added_lines.contains(&(i + 1)))
                            .map(|(_, l)| l.to_string())
                            .collect(),
                    )
                })
                .unwrap_or_default();
            let unmatched: Vec<&String> = owner
                .tests
                .iter()
                .filter(|t| !added_in_owner.iter().any(|l| l.contains(t.as_str())))
                .collect();
            if unmatched.is_empty() {
                continue;
            }
            if let Some(ov) = ctx.find_override(GATE, tokens::ALLOW_GOLDEN_UPDATE, &file.path) {
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

        if let Some(ov) = ctx.find_override(GATE, tokens::ALLOW_GOLDEN_UPDATE, &file.path) {
            out.overrides.push(ov);
            continue;
        }
        if !file.old_path.is_empty() && file.old_path != file.path {
            if let Some(ov) = ctx.find_override(GATE, tokens::ALLOW_GOLDEN_UPDATE, &file.old_path) {
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
    let mut dir_note = |what: String| {
        found.push(Weakening {
            gate: "directives".to_string(),
            what,
        })
    };
    if !base.directives.allow_hidden && head.directives.allow_hidden {
        dir_note("`allow_hidden` changed from false to true".to_string());
    }
    let gained_sources: Vec<_> = head
        .directives
        .sources
        .iter()
        .filter(|s| !base.directives.sources.contains(s))
        .collect();
    if gained_sources.iter().any(|s| s.as_str() == "commits") {
        dir_note("`sources` gained commits".to_string());
    }
    if base.directives.fail_on_overrides && !head.directives.fail_on_overrides {
        dir_note("`fail_on_overrides` changed from true to false".to_string());
    }
    // A listed actor is exempt from `fail_on_overrides`.
    let gained_actors = head
        .directives
        .allowed_override_actors
        .iter()
        .filter(|a| !base.directives.allowed_override_actors.contains(a))
        .count();
    if gained_actors > 0 {
        dir_note(format!(
            "`allowed_override_actors` gained {gained_actors} entr(y/ies)"
        ));
    }

    match (base.directives.max_overrides, head.directives.max_overrides) {
        (Some(b), None) => dir_note(format!("`max_overrides` removed (was {b})")),
        (Some(b), Some(h)) if h > b => {
            dir_note(format!("`max_overrides` increased from {b} to {h}"))
        }
        _ => {}
    }
    if base.directives.require_approval && !head.directives.require_approval {
        dir_note("`require_approval` changed from true to false".to_string());
    }
    if !base.directives.degrade_offline && head.directives.degrade_offline {
        dir_note("`degrade_offline` changed from false to true (a failed merged-pr-body lookup no longer stops the run)".to_string());
    }
    // (true -> false is stricter: a failed lookup then stops the run.)

    // [tests]: widening what counts as test code narrows what the production-code gates see.
    for (key, b, h) in [
        ("functions", &base.tests.functions, &head.tests.functions),
        ("paths", &base.tests.paths, &head.tests.paths),
    ] {
        let gained = h.iter().filter(|x| !b.contains(x)).count();
        if gained > 0 {
            found.push(Weakening {
                gate: "tests".to_string(),
                what: format!("`{key}` gained {gained} entr(y/ies)"),
            });
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
            found.push(Weakening {
                gate: "languages".to_string(),
                what: format!("`{key}` gained {gained} entr(y/ies)"),
            });
        }
    }

    // [meta]: advisory mode exits 0 whatever the gates found.
    if base.meta.mode == RunMode::Enforcing && head.meta.mode == RunMode::Advisory {
        found.push(Weakening {
            gate: "meta".to_string(),
            what: "`mode` changed from enforcing to advisory".to_string(),
        });
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
        let mut note = |what: String| {
            found.push(Weakening {
                gate: gate.clone(),
                what,
            })
        };
        for (key, bv) in b {
            // An option this binary does not know is judged as a plain switch, so a
            // stale table degrades to the strict reading rather than to silence.
            let dir = direction_of(key).unwrap_or(Direction::LooserWhenFalse);
            let Some(hv) = h.get(key) else {
                match dir {
                    Direction::Evidence | Direction::Floor | Direction::Cap => {
                        note(format!("`{key}` removed (was {bv})"))
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
                Direction::Evidence if bv != hv => {
                    note(format!("`{key}` changed from {bv} to {hv}"))
                }
                Direction::StrictMode(strict) => {
                    if let (Value::String(bs), Value::String(hs)) = (bv, hv) {
                        if bs == strict && hs != strict {
                            note(format!("`{key}` changed from {bs} to {hs}"));
                        }
                    }
                }
                Direction::LooserWhenFalse
                    if matches!((bv, hv), (Value::Boolean(true), Value::Boolean(false))) =>
                {
                    note(format!("`{key}` changed from true to false"))
                }
                Direction::LooserWhenTrue
                    if matches!((bv, hv), (Value::Boolean(false), Value::Boolean(true))) =>
                {
                    note(format!("`{key}` changed from false to true"))
                }
                Direction::Floor => {
                    if let (Some(bn), Some(hn)) = (num(bv), num(hv)) {
                        if hn < bn {
                            note(format!("`{key}` decreased from {bv} to {hv}"));
                        }
                    }
                }
                Direction::Cap | Direction::Tolerance => {
                    if let (Some(bn), Some(hn)) = (num(bv), num(hv)) {
                        if hn > bn {
                            note(format!("`{key}` increased from {bv} to {hv}"));
                        }
                    }
                }
                Direction::Severity => {
                    if let (Value::String(bs), Value::String(hs)) = (bv, hv) {
                        if (bs == "error" && (hs == "warning" || hs == "note"))
                            || (bs == "warning" && hs == "note")
                        {
                            note(format!("`severity` lowered from {bs} to {hs}"));
                        }
                    }
                }
                Direction::Grown | Direction::Shrunk | Direction::Allowlist => {
                    let (Value::Array(ba), Value::Array(ha)) = (bv, hv) else {
                        continue;
                    };
                    let gained = ha.iter().filter(|x| !ba.contains(x)).count();
                    let lost = ba.iter().filter(|x| !ha.contains(x)).count();
                    if dir == Direction::Allowlist && !ba.is_empty() && ha.is_empty() {
                        note(format!(
                            "`{key}` emptied, which switches the allow-list off"
                        ));
                    } else if dir == Direction::Allowlist && ba.is_empty() {
                        // No allow-list on base: adopting one is a tightening.
                    } else if dir != Direction::Shrunk && gained > 0 {
                        note(format!("`{key}` gained {gained} entr(y/ies)"));
                    } else if dir == Direction::Shrunk && lost > 0 {
                        note(format!("`{key}` lost {lost} entr(y/ies)"));
                    }
                }
                _ => {}
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(body: &str) -> DisciplineConfig {
        DisciplineConfig::from_toml_str(&format!("[meta]\nversion = 1\nname = \"t\"\n{body}"))
            .unwrap()
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
                .any(|w| w.gate == gate && w.what.contains(needle))
        };
        assert!(has("pii", "`lan_ips` changed from true to false"));
        assert!(has("pii", "`exempt_paths` gained 1"));
        assert!(has("pii", "`hostname_denylist` lost 1"));
        assert!(has("vacuous-tests", "`enabled` changed from true to false"));
        assert!(has("agent-scratch", "`severity` lowered"));
        assert_eq!(found.len(), 5, "{found:?}");
    }

    #[test]
    fn integer_and_security_boolean_loosening_detected() {
        let base = cfg("[gates.command]\nmin_count = 10\n[gates.unsafe-budget]\nmax_unsafe = 5\n[gates.pii]\ndiff_only = false\n");
        let head = cfg("[gates.command]\nmin_count = 5\n[gates.unsafe-budget]\nmax_unsafe = 10\n[gates.pii]\ndiff_only = true\n");
        let found = diff_configs(&base, &head).unwrap();
        let has = |gate: &str, needle: &str| {
            found
                .iter()
                .any(|w| w.gate == gate && w.what.contains(needle))
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
                .any(|w| w.gate == gate && w.what.contains(needle))
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
                .any(|w| w.gate == gate && w.what.contains(needle))
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
        let base = cfg("max_overrides = 1\nrequire_approval = true\n");
        let whats = |head: &DisciplineConfig| -> Vec<String> {
            diff_configs(&base, head)
                .unwrap()
                .into_iter()
                .map(|w| w.what)
                .collect()
        };
        assert_eq!(
            whats(&cfg("max_overrides = 4\n")),
            vec![
                "`max_overrides` increased from 1 to 4",
                "`require_approval` changed from true to false"
            ]
        );
        assert_eq!(
            whats(&cfg("require_approval = true\n")),
            vec!["`max_overrides` removed (was 1)"]
        );
        assert!(whats(&cfg("max_overrides = 0\nrequire_approval = true\n")).is_empty());
        // Switching the failed-lookup degrade on is a loosening; off is a tightening.
        let off = cfg("degrade_offline = false\n");
        let on = cfg("degrade_offline = true\n");
        let notes: Vec<String> = diff_configs(&off, &on)
            .unwrap()
            .into_iter()
            .map(|w| w.what)
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
            found,
            vec![
                Weakening {
                    gate: "directives".into(),
                    what: "`allowed_override_actors` gained 1 entr(y/ies)".into()
                },
                Weakening {
                    gate: "meta".into(),
                    what: "`mode` changed from enforcing to advisory".into()
                },
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
                .any(|w| w.gate == gate && w.what.contains(needle))
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
        assert!(back[0].what.contains("`test_command` changed"), "{back:?}");
        // Adopting an allow-list where none existed is a tightening; growing one is not.
        let none = cfg("");
        let adopted = cfg("[gates.dependency-delta]\nallow_dependencies = [\"serde\"]\n");
        let grown =
            cfg("[gates.dependency-delta]\nallow_dependencies = [\"serde\", \"left-pad\"]\n");
        assert!(diff_configs(&none, &adopted).unwrap().is_empty());
        assert_eq!(diff_configs(&adopted, &grown).unwrap().len(), 1);
    }
}
