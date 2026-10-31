//! Review routing: what a change touches that a reviewer routes on (#944).
//!
//! A report section, not a gate: no outcome, no finding code, no severity, nothing to
//! waive. It is computed on every `check` that has a change and is never silenced. It is
//! an inventory, never a score or a verdict, and it renders after the verdict.
//!
//! It has two halves, kept apart in every rendering:
//!
//! * **deterministic**: the directives the run read from the change, the inline markers it
//!   adds or removes, and the authority files it changes (the policy file, CI workflows,
//!   lockfiles, build hooks, toolchain and sandbox configuration, agent instructions and
//!   hooks, code owners). These are exact. Automated routing (a merge queue, an
//!   auto-merge rule) may condition on this half only, and only with a repository's own
//!   opt-in: a zero is not a clearance.
//! * **heuristic**: the capability sites the change adds (process spawn, dynamic
//!   evaluation, network; [`crate::ast::capability_sinks`]) and the tests it adds or
//!   removes. The capability vocabulary is closed and covers the languages whose pack
//!   supplies `Fact::CapabilitySinks`, so this half informs reviewers and nothing else.
//!
//! A coverage footer says how many changed files were in a language with a capability
//! inventory, computed from the language packs ([`crate::ast::coverage_labels`]).
//!
//! The JSON is declared unstable until 1.0 (`stability`), and no name of it is in the
//! frozen interface (`tests/fixtures/v1_surface.json`).

use crate::ast::capability_sinks::Surface;
use crate::ast::Fact;
use crate::gitctx::ChangeKind;
use crate::guards::Context;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The `stability` every rendering of the section states.
pub const STABILITY: &str =
    "unstable until 1.0: names and shape may change in any release; an inventory, not a verdict";

/// Pointers a human surface lists before it counts the rest.
pub const POINTER_CAP: usize = 5;

/// The section, as the JSON report carries it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRouting {
    pub stability: String,
    pub deterministic: Deterministic,
    pub heuristic: Heuristic,
    pub coverage: Coverage,
}

/// The exact half: automated routing may read it, with the repository's opt-in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deterministic {
    /// Directives the run read from the change (pull-request body, commit bodies, merged
    /// pull-request bodies), whether or not they lifted anything.
    pub directives: usize,
    /// Their names, lower case, each once, sorted.
    pub directive_kinds: Vec<String>,
    /// `discipline:allow(<gate>)` markers on lines the change adds.
    pub inline_markers_added: usize,
    /// Marker lines the base side had and the change removes.
    pub inline_markers_removed: usize,
    /// Changed files that carry authority over the pipeline, in path order.
    pub authority_files: Vec<AuthorityFile>,
}

/// A changed file that governs how the change is built, checked or run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityFile {
    pub path: String,
    /// `policy`, `ci`, `lockfile`, `build-hook`, `toolchain`, `sandbox`,
    /// `agent-instructions` or `code-owners`.
    pub surface: String,
    /// `added`, `modified`, `deleted` or `renamed`.
    pub change: String,
}

/// The non-exhaustive half: it informs reviewers, never routing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Heuristic {
    /// Calls to a capability sink on lines the change adds, by surface, then path and line.
    pub capability_sites: Vec<CapabilitySite>,
    /// Tests the head side has that the base side does not, by name.
    pub tests_added: usize,
    /// Tests the base side has that the head side does not.
    pub tests_removed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitySite {
    pub file: String,
    pub line: usize,
    /// `process-spawn`, `dynamic-eval` or `network`.
    pub surface: String,
}

/// What the capability inventory looked at, and what it did not.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coverage {
    /// Files the change adds, modifies or renames (a deletion adds no code).
    pub changed_files: usize,
    /// Of those, the files in a language with a capability inventory that parsed.
    pub inventoried_files: usize,
    /// Files in such a language that did not parse: not inventoried.
    pub unparsed_files: usize,
    /// The languages with a capability inventory in this binary, in registry order.
    pub languages: Vec<String>,
}

/// The authority surface of `path`, if it has one. `config_path` is the configuration
/// file the run reads.
pub fn authority_surface(path: &str, config_path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    if path == config_path
        || name == "discipline.toml"
        || name == crate::baseline::DEFAULT_BASELINE_FILE
    {
        return Some("policy");
    }
    let workflow = (name.ends_with(".yml") || name.ends_with(".yaml"))
        && crate::doctor::WORKFLOW_DIRS
            .iter()
            .any(|dir| path.starts_with(&format!("{dir}/")));
    if workflow
        || crate::guards::ci_gitlab::is_gitlab_ci_path(path)
        || crate::guards::ci_integrity::is_action_metadata_path(path)
    {
        return Some("ci");
    }
    if LOCKFILES.contains(&name) {
        return Some("lockfile");
    }
    if crate::guards::build_hooks::is_build_hook_file(path) {
        return Some("build-hook");
    }
    if crate::doctor::CODEOWNERS_PATHS.contains(&path) {
        return Some("code-owners");
    }
    if crate::guards::instruction_smuggling::is_instruction_file(path) {
        return Some("agent-instructions");
    }
    if crate::guards::toolchain_config::classify(path).is_some() {
        return Some("toolchain");
    }
    if crate::guards::sandbox_config::classify(path).is_some() {
        return Some("sandbox");
    }
    None
}

/// Lockfiles by name: the formats `dependency-delta` reads, and the other common ones.
const LOCKFILES: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lockb",
    "poetry.lock",
    "uv.lock",
    "Pipfile.lock",
    "composer.lock",
    "Gemfile.lock",
    "go.sum",
    "packages.lock.json",
];

fn change_word(kind: &ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Added => "added",
        ChangeKind::Deleted => "deleted",
        ChangeKind::Renamed => "renamed",
        _ => "modified",
    }
}

/// The marker lines of `text`, trimmed, with how many times each occurs.
fn marker_lines(text: &str) -> BTreeMap<&str, usize> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        if crate::audit::marker_gate(line).is_some() {
            *out.entry(line.trim()).or_insert(0) += 1;
        }
    }
    out
}

/// Test names of one side of a file, with how many times each occurs.
fn test_names(
    pack: &dyn crate::ast::LanguagePack,
    path: &str,
    src: &str,
    vocab: &crate::ast::AssertVocabulary,
) -> Option<BTreeMap<String, usize>> {
    let facts = pack.extract(path, src, vocab).ok()?;
    let mut out = BTreeMap::new();
    for t in facts.tests {
        *out.entry(t.name).or_insert(0) += 1;
    }
    Some(out)
}

/// How many of `a`'s entries `b` lacks, counting repeats.
fn missing(a: &BTreeMap<String, usize>, b: &BTreeMap<String, usize>) -> usize {
    a.iter()
        .map(|(k, n)| n.saturating_sub(b.get(k).copied().unwrap_or(0)))
        .sum()
}

/// The section for the change `ctx` checks; `None` for a whole-tree run, which has no
/// change. A file that cannot be read is an error, as for any gate (exit 2); a file in an
/// inventoried language that does not parse is counted as not inventoried.
pub fn compute(ctx: &Context) -> Result<Option<ReviewRouting>> {
    if ctx.git.is_whole_tree() {
        return Ok(None);
    }
    let registry = crate::ast::default_registry();
    let vocab = crate::guards::agent_diff::assert_vocabulary(ctx.config);
    let changed = ctx.git.changed_files()?;

    let mut r = ReviewRouting {
        stability: STABILITY.to_string(),
        ..Default::default()
    };
    r.coverage.languages = crate::ast::coverage_labels(Fact::CapabilitySinks)
        .into_iter()
        .map(str::to_string)
        .collect();

    let mut kinds: Vec<String> = ctx
        .directives
        .iter()
        .map(|d| d.directive.to_ascii_lowercase())
        .collect();
    kinds.sort();
    kinds.dedup();
    r.deterministic.directives = ctx.directives.len();
    r.deterministic.directive_kinds = kinds;

    let mut sites: Vec<(Surface, String, usize)> = Vec::new();
    for file in &changed {
        let surface = authority_surface(&file.path, ctx.config_path).or_else(|| {
            (file.old_path != file.path)
                .then(|| authority_surface(&file.old_path, ctx.config_path))
                .flatten()
        });
        if let Some(surface) = surface {
            r.deterministic.authority_files.push(AuthorityFile {
                path: file.path.clone(),
                surface: surface.to_string(),
                change: change_word(&file.kind).to_string(),
            });
        }

        let base = match file.kind {
            ChangeKind::Added => None,
            _ => ctx
                .git
                .base_content(&file.old_path)?
                .filter(|s| !s.contains('\0')),
        };
        let head = match file.kind {
            ChangeKind::Deleted => None,
            _ => ctx
                .git
                .head_content(&file.path)?
                .filter(|s| !s.contains('\0')),
        };
        if !file.is_deleted() {
            r.coverage.changed_files += 1;
        }

        // Inline markers: added on the change's own lines; removed when the base side's
        // marker lines are not all on the head side.
        if let Some(head) = &head {
            r.deterministic.inline_markers_added += head
                .lines()
                .enumerate()
                .filter(|(i, line)| {
                    file.added_lines.contains(&(i + 1)) && crate::audit::marker_gate(line).is_some()
                })
                .count();
        }
        if let Some(base) = &base {
            let before = marker_lines(base);
            let after = head.as_deref().map(marker_lines).unwrap_or_default();
            r.deterministic.inline_markers_removed += before
                .iter()
                .map(|(k, n)| n.saturating_sub(after.get(k).copied().unwrap_or(0)))
                .sum::<usize>();
        }

        // Tests added and removed, by name, where a pack reads tests.
        let base_pack = registry
            .find_pack(&file.old_path)
            .filter(|p| p.supplies(Fact::Tests));
        let head_pack = registry
            .find_pack(&file.path)
            .filter(|p| p.supplies(Fact::Tests));
        let before = match (base_pack, &base) {
            (Some(p), Some(src)) => test_names(p, &file.old_path, src, &vocab),
            _ => None,
        }
        .unwrap_or_default();
        let after = match (head_pack, &head) {
            (Some(p), Some(src)) => test_names(p, &file.path, src, &vocab),
            _ => None,
        }
        .unwrap_or_default();
        r.heuristic.tests_added += missing(&after, &before);
        r.heuristic.tests_removed += missing(&before, &after);

        // Capability sites on added lines.
        let Some(pack) = registry
            .find_pack(&file.path)
            .filter(|p| p.supplies(Fact::CapabilitySinks))
        else {
            continue;
        };
        let Some(head) = &head else {
            continue;
        };
        match pack.capability_sinks(&file.path, head) {
            Ok(found) => {
                r.coverage.inventoried_files += 1;
                for s in found {
                    if file.added_lines.contains(&s.line) {
                        sites.push((s.surface, file.path.clone(), s.line));
                    }
                }
            }
            Err(_) => r.coverage.unparsed_files += 1,
        }
    }
    sites.sort();
    r.heuristic.capability_sites = sites
        .into_iter()
        .map(|(surface, file, line)| CapabilitySite {
            file,
            line,
            surface: surface.as_str().to_string(),
        })
        .collect();
    Ok(Some(r))
}

impl ReviewRouting {
    /// The deterministic rows that are not zero, as `(label, count text)`.
    fn deterministic_rows(&self) -> Vec<(&'static str, String)> {
        let d = &self.deterministic;
        let mut rows = Vec::new();
        if d.directives > 0 {
            rows.push(("directives", d.directives.to_string()));
        }
        if d.inline_markers_added + d.inline_markers_removed > 0 {
            rows.push((
                "inline markers",
                format!("+{} -{}", d.inline_markers_added, d.inline_markers_removed),
            ));
        }
        if !d.authority_files.is_empty() {
            rows.push(("authority files", d.authority_files.len().to_string()));
        }
        rows
    }

    /// The heuristic rows that are not zero.
    fn heuristic_rows(&self) -> Vec<(&'static str, String)> {
        let h = &self.heuristic;
        let mut rows = Vec::new();
        if !h.capability_sites.is_empty() {
            rows.push(("capability sites", h.capability_sites.len().to_string()));
        }
        if h.tests_added + h.tests_removed > 0 {
            rows.push(("tests", format!("+{} -{}", h.tests_added, h.tests_removed)));
        }
        rows
    }

    /// The coverage footer, or `None` when the change has no file to inventory.
    pub fn coverage_text(&self) -> Option<String> {
        let c = &self.coverage;
        if c.changed_files == 0 {
            return None;
        }
        let unparsed = match c.unparsed_files {
            0 => String::new(),
            n => format!("; {n} did not parse"),
        };
        Some(format!(
            "{} of {} changed files in a language with a capability inventory ({}){unparsed}; runtime semantics not inspected",
            c.inventoried_files,
            c.changed_files,
            c.languages.join(", ")
        ))
    }

    /// The counts on one line, zero rows left out: the commit-status description, the
    /// first line of the job summary, the hook's line. `None` when every row is zero and
    /// the change has no file to inventory.
    pub fn counts_line(&self) -> Option<String> {
        let join = |rows: Vec<(&str, String)>| {
            rows.into_iter()
                .map(|(label, n)| format!("{label} {n}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut parts = Vec::new();
        let det = self.deterministic_rows();
        if !det.is_empty() {
            parts.push(format!("deterministic: {}", join(det)));
        }
        let heu = self.heuristic_rows();
        if !heu.is_empty() {
            parts.push(format!("heuristic: {}", join(heu)));
        }
        let c = &self.coverage;
        if c.changed_files > 0 {
            parts.push(format!(
                "{} of {} files inventoried",
                c.inventoried_files, c.changed_files
            ));
        }
        if parts.is_empty() {
            return None;
        }
        Some(format!("review routing: {}", parts.join("; ")))
    }

    /// At most [`POINTER_CAP`] capability pointers, through `quote`, and the tail that
    /// counts the rest by surface (`and 3 more in network`).
    pub fn capability_pointers(&self, quote: impl Fn(&str) -> String) -> String {
        let sites = &self.heuristic.capability_sites;
        let mut out: Vec<String> = sites
            .iter()
            .take(POINTER_CAP)
            .map(|s| {
                format!(
                    "{} ({})",
                    quote(&format!("{}:{}", s.file, s.line)),
                    s.surface
                )
            })
            .collect();
        if let Some(tail) = more_tail(sites.iter().skip(POINTER_CAP).map(|s| s.surface.as_str())) {
            out.push(tail);
        }
        out.join(", ")
    }

    /// At most [`POINTER_CAP`] authority files, through `quote`, and the tail.
    pub fn authority_pointers(&self, quote: impl Fn(&str) -> String) -> String {
        let files = &self.deterministic.authority_files;
        let mut out: Vec<String> = files
            .iter()
            .take(POINTER_CAP)
            .map(|f| format!("{} ({}, {})", quote(&f.path), f.surface, f.change))
            .collect();
        if let Some(tail) = more_tail(files.iter().skip(POINTER_CAP).map(|f| f.surface.as_str())) {
            out.push(tail);
        }
        out.join(", ")
    }

    /// The section for the terminal report, after the verdict. Empty when every row is
    /// zero and nothing was changed.
    pub fn terminal(&self) -> String {
        let Some(counts) = self.counts_line() else {
            return String::new();
        };
        let q = |s: &str| crate::report::text::terminal_line(s);
        let mut out = format!("\n{counts}\n  ({STABILITY})\n");
        let d = &self.deterministic;
        let det = self.deterministic_rows();
        if !det.is_empty() {
            out.push_str(
                "  deterministic (exact; routing may read it, with a repository's opt-in):\n",
            );
            if d.directives > 0 {
                out.push_str(&format!(
                    "    directives: {} ({})\n",
                    d.directives,
                    q(&d.directive_kinds.join(", "))
                ));
            }
            if d.inline_markers_added + d.inline_markers_removed > 0 {
                out.push_str(&format!(
                    "    inline markers: {} added, {} removed\n",
                    d.inline_markers_added, d.inline_markers_removed
                ));
            }
            if !d.authority_files.is_empty() {
                out.push_str(&format!(
                    "    authority files: {}: {}\n",
                    d.authority_files.len(),
                    self.authority_pointers(q)
                ));
            }
        }
        let h = &self.heuristic;
        if !self.heuristic_rows().is_empty() {
            out.push_str("  heuristic (non-exhaustive; informs reviewers, never routing):\n");
            if !h.capability_sites.is_empty() {
                out.push_str(&format!(
                    "    capability sites: {}: {}\n",
                    h.capability_sites.len(),
                    self.capability_pointers(q)
                ));
            }
            if h.tests_added + h.tests_removed > 0 {
                out.push_str(&format!(
                    "    tests: {} added, {} removed\n",
                    h.tests_added, h.tests_removed
                ));
            }
        }
        if let Some(cov) = self.coverage_text() {
            out.push_str(&format!("  coverage: {cov}\n"));
        }
        out
    }

    /// The section for the job summary (Markdown): a heading, one table per half, and
    /// the coverage footer. Text from the change is escaped as every summary cell is.
    pub fn markdown(&self) -> String {
        if self.counts_line().is_none() {
            return String::new();
        }
        let span = |s: &str| crate::report::text::code_span_cell(s);
        let mut out = format!(
            "\n#### Review routing\n\n_{}. Only the deterministic half is fit for automated routing, and only with a repository's opt-in: a zero is not a clearance._\n",
            crate::report::text::markdown(STABILITY)
        );
        let d = &self.deterministic;
        if !self.deterministic_rows().is_empty() {
            out.push_str(
                "\n**Deterministic** (exact)\n\n| Row | Count | Where |\n|---|---:|---|\n",
            );
            if d.directives > 0 {
                let kinds: Vec<String> = d.directive_kinds.iter().map(|k| span(k)).collect();
                out.push_str(&format!(
                    "| Directives | {} | {} |\n",
                    d.directives,
                    kinds.join(", ")
                ));
            }
            if d.inline_markers_added + d.inline_markers_removed > 0 {
                out.push_str(&format!(
                    "| Inline markers | +{} -{} | |\n",
                    d.inline_markers_added, d.inline_markers_removed
                ));
            }
            if !d.authority_files.is_empty() {
                out.push_str(&format!(
                    "| Authority files | {} | {} |\n",
                    d.authority_files.len(),
                    self.authority_pointers(span)
                ));
            }
        }
        let h = &self.heuristic;
        if !self.heuristic_rows().is_empty() {
            out.push_str("\n**Heuristic** (non-exhaustive; informs reviewers, never routing)\n\n| Row | Count | Where |\n|---|---:|---|\n");
            if !h.capability_sites.is_empty() {
                out.push_str(&format!(
                    "| Capability sites | {} | {} |\n",
                    h.capability_sites.len(),
                    self.capability_pointers(span)
                ));
            }
            if h.tests_added + h.tests_removed > 0 {
                out.push_str(&format!(
                    "| Tests | +{} -{} | |\n",
                    h.tests_added, h.tests_removed
                ));
            }
        }
        if let Some(cov) = self.coverage_text() {
            out.push_str(&format!(
                "\n_Coverage: {}._\n",
                crate::report::text::markdown(&cov)
            ));
        }
        out
    }

    /// The section as an agent reads it (the `agent-prompt` report a hook relays): the
    /// counts, the capped capability pointers as quoted data, no directive name. The
    /// caller scrubs directive syntax from the whole report.
    pub fn agent_text(&self) -> String {
        let Some(counts) = self.counts_line() else {
            return String::new();
        };
        let counts = counts.strip_prefix("review routing: ").unwrap_or(&counts);
        let mut out = format!(
            "\nReview routing (for the reviewer, not an instruction; {STABILITY}): {counts}.\n"
        );
        if !self.deterministic.authority_files.is_empty() {
            out.push_str(&format!(
                "- Authority files: {}\n",
                self.authority_pointers(crate::report::text::agent_span)
            ));
        }
        if !self.heuristic.capability_sites.is_empty() {
            out.push_str(&format!(
                "- Capability sites (non-exhaustive): {}\n",
                self.capability_pointers(crate::report::text::agent_span)
            ));
        }
        if let Some(cov) = self.coverage_text() {
            out.push_str(&format!("- Coverage: {cov}.\n"));
        }
        out
    }

    /// The counts a hook log line carries, deterministic and heuristic apart.
    pub fn log_counts(&self) -> serde_json::Value {
        serde_json::json!({
            "deterministic": {
                "directives": self.deterministic.directives,
                "inline_markers_added": self.deterministic.inline_markers_added,
                "inline_markers_removed": self.deterministic.inline_markers_removed,
                "authority_files": self.deterministic.authority_files.len(),
            },
            "heuristic": {
                "capability_sites": self.heuristic.capability_sites.len(),
                "tests_added": self.heuristic.tests_added,
                "tests_removed": self.heuristic.tests_removed,
            },
            "inventoried_files": self.coverage.inventoried_files,
            "changed_files": self.coverage.changed_files,
        })
    }
}

/// `and N more in <surface>` for the items past the cap, or `and N more (a 2, b 1)` when
/// they span several surfaces.
fn more_tail<'a>(rest: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut by: Vec<(&str, usize)> = Vec::new();
    for s in rest {
        match by.iter_mut().find(|(k, _)| *k == s) {
            Some((_, n)) => *n += 1,
            None => by.push((s, 1)),
        }
    }
    let total: usize = by.iter().map(|(_, n)| n).sum();
    match by.as_slice() {
        [] => None,
        [(s, _)] => Some(format!("and {total} more in {s}")),
        _ => Some(format!(
            "and {total} more ({})",
            by.iter()
                .map(|(s, n)| format!("{s} {n}"))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ReviewRouting {
        let mut r = ReviewRouting {
            stability: STABILITY.to_string(),
            ..Default::default()
        };
        r.deterministic.directives = 2;
        r.deterministic.directive_kinds = vec!["allow-command".into(), "no-issue".into()];
        r.deterministic.inline_markers_added = 1;
        r.deterministic.authority_files.push(AuthorityFile {
            path: ".github/workflows/ci.yml".into(),
            surface: "ci".into(),
            change: "modified".into(),
        });
        for i in 0..8 {
            r.heuristic.capability_sites.push(CapabilitySite {
                file: "src/a.rs".into(),
                line: i + 1,
                surface: if i < 6 { "process-spawn" } else { "network" }.into(),
            });
        }
        r.heuristic.tests_added = 3;
        r.coverage = Coverage {
            changed_files: 9,
            inventoried_files: 4,
            unparsed_files: 0,
            languages: vec!["Rust".into(), "Go".into()],
        };
        r
    }

    #[test]
    fn counts_line_keeps_the_halves_apart_and_leaves_out_zero_rows() {
        let line = sample().counts_line().unwrap();
        assert_eq!(
            line,
            "review routing: deterministic: directives 2, inline markers +1 -0, authority files 1; heuristic: capability sites 8, tests +3 -0; 4 of 9 files inventoried"
        );
        // A change with nothing to route still says what was inventoried.
        let mut empty = ReviewRouting::default();
        empty.coverage.changed_files = 2;
        assert_eq!(
            empty.counts_line().unwrap(),
            "review routing: 0 of 2 files inventoried"
        );
        assert!(ReviewRouting::default().counts_line().is_none());
        assert!(ReviewRouting::default().terminal().is_empty());
        assert!(ReviewRouting::default().markdown().is_empty());
        assert!(ReviewRouting::default().agent_text().is_empty());
    }

    #[test]
    fn pointers_are_capped_with_a_tail_by_surface() {
        let r = sample();
        let p = r.capability_pointers(|s| format!("`{s}`"));
        assert_eq!(p.matches("src/a.rs:").count(), POINTER_CAP);
        assert!(
            p.ends_with("and 3 more (process-spawn 1, network 2)"),
            "{p}"
        );
        let mut one = sample();
        one.heuristic.capability_sites.truncate(6);
        assert!(one
            .capability_pointers(|s| s.to_string())
            .ends_with("and 1 more in process-spawn"));
        one.heuristic.capability_sites.truncate(5);
        assert!(!one.capability_pointers(|s| s.to_string()).contains("more"));
    }

    #[test]
    fn every_rendering_distinguishes_the_halves_and_states_its_stability() {
        let r = sample();
        let t = r.terminal();
        assert!(t.contains("deterministic (exact") && t.contains("heuristic (non-exhaustive"));
        assert!(t.contains("unstable until 1.0"));
        assert!(t.contains("runtime semantics not inspected"));
        let m = r.markdown();
        assert!(m.contains("**Deterministic**") && m.contains("**Heuristic**"));
        assert!(m.contains("unstable until 1.0"));
        let a = r.agent_text();
        assert!(a.contains("not an instruction") && a.contains("non-exhaustive"));
        // Directive names never reach the agent's text.
        assert!(!a.contains("allow-command") && !a.contains("no-issue"));
        // No score, no rating.
        for text in [&t, &m, &a] {
            let lower = text.to_lowercase();
            assert!(
                !lower.contains("score") && !lower.contains("risk"),
                "{text}"
            );
        }
    }

    #[test]
    fn authority_surfaces_are_the_files_gates_already_read() {
        let s = |p: &str| authority_surface(p, "discipline.toml");
        assert_eq!(s("discipline.toml"), Some("policy"));
        assert_eq!(s("conf/strict.toml"), None);
        assert_eq!(
            authority_surface("conf/strict.toml", "conf/strict.toml"),
            Some("policy")
        );
        assert_eq!(s(".github/workflows/ci.yml"), Some("ci"));
        assert_eq!(s(".gitlab-ci.yml"), Some("ci"));
        assert_eq!(s("Cargo.lock"), Some("lockfile"));
        assert_eq!(s("web/package-lock.json"), Some("lockfile"));
        assert_eq!(s("build.rs"), Some("build-hook"));
        assert_eq!(s(".npmrc"), Some("build-hook"));
        assert_eq!(s("AGENTS.md"), Some("agent-instructions"));
        assert_eq!(s(".github/CODEOWNERS"), Some("code-owners"));
        assert_eq!(s("src/main.rs"), None);
        assert_eq!(s("docs/ci.yml"), None);
    }

    #[test]
    fn json_round_trips() {
        let r = sample();
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["stability"], STABILITY);
        assert!(v["deterministic"]["authority_files"].is_array());
        assert!(v["heuristic"]["capability_sites"].is_array());
        let back: ReviewRouting = serde_json::from_value(v).unwrap();
        assert_eq!(back, r);
    }
}
