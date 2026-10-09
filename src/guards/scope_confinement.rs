//! Scope confinement sentinel (`scope-confinement`).
//!
//! Restricts agent modifications strictly within authorized directory and file paths.

use crate::guards::{Context, GateOutcome, PathFilter};
use crate::tokens::ALLOW_SCOPE;
use anyhow::Result;

pub const GATE: &str = "scope-confinement";

/// Evaluates git diff paths against authorized and forbidden scope rules.
pub fn evaluate_scope_confinement(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.scope_confinement;
    let mut out = GateOutcome::new(GATE);

    let changed = ctx.git.changed_files()?;
    out.examined = changed.len();

    if changed.is_empty() {
        return Ok(out);
    }

    // A malformed glob is a configuration error (exit 2), never a silently
    // skipped pattern that would let a forbidden path pass.
    let exempt = PathFilter::new(&settings.exempt_paths)?;

    // Build allowed paths filter (if non-empty)
    let has_allowed = !settings.allowed_paths.is_empty();
    let allowed = PathFilter::new(&settings.allowed_paths)?;

    // Build forbidden paths filter
    let forbidden = PathFilter::new(&settings.forbidden_paths)?;

    for file in &changed {
        if exempt.matches(&file.path) {
            continue;
        }

        // 1. Check forbidden paths first
        if forbidden.matches(&file.path) {
            if let Some(ov) = ctx.find_override(
                GATE,
                &crate::findings::FILE_IN_FORBIDDEN_SCOPE,
                ALLOW_SCOPE,
                &file.path,
            ) {
                out.overrides.push(ov.clone());
                out.notes.push(format!(
                    "override applied: `{}: {}` for forbidden file `{}` ({})",
                    ov.directive, ov.reason, file.path, ov.source
                ));
            } else {
                out.add_violation(
                    ctx.overridable(settings.severity),
&crate::findings::FILE_IN_FORBIDDEN_SCOPE,
                    &file.path,
                    1,
                    format!("file `{}` is inside forbidden scope", file.path),
                    "changes to this path are forbidden by [gates.scope-confinement.forbidden_paths]; use `discipline:allow(scope-confinement): <reason>` to waive",
                );
            }
            continue;
        }

        // 2. Check allowed paths if configured
        if has_allowed && !allowed.matches(&file.path) {
            if let Some(ov) = ctx.find_override(
                GATE,
                &crate::findings::FILE_OUTSIDE_AUTHORIZED_SCOPE,
                ALLOW_SCOPE,
                &file.path,
            ) {
                out.overrides.push(ov.clone());
                out.notes.push(format!(
                    "override applied: `{}: {}` for out-of-scope file `{}` ({})",
                    ov.directive, ov.reason, file.path, ov.source
                ));
            } else {
                out.add_violation(
                    ctx.overridable(settings.severity),
&crate::findings::FILE_OUTSIDE_AUTHORIZED_SCOPE,
                    &file.path,
                    1,
                    format!("file `{}` is outside authorized scope", file.path),
                    "file is not included in [gates.scope-confinement.allowed_paths]; use `discipline:allow(scope-confinement): <reason>` to waive",
                );
            }
        }
    }

    // Check plan files if configured
    if settings.check_plans {
        let plan_filter = PathFilter::new(&settings.plan_paths)?;
        let mut plan_files_examined = 0;

        for file in &changed {
            if file.is_deleted() || !plan_filter.matches(&file.path) {
                continue;
            }
            plan_files_examined += 1;
            let Some(content) = ctx.git.head_content(&file.path)? else {
                continue;
            };

            let planned = extract_planned_outputs(&content);
            if planned.is_empty() {
                if settings.require_declared_outputs {
                    out.add_violation(
                        ctx.overridable(settings.severity),
                        &crate::findings::PLAN_WITHOUT_DECLARED_OUTPUTS,
                        &file.path,
                        1,
                        format!("plan file `{}` declares no output paths", file.path),
                        "plan files must declare at least one output path when [gates.scope-confinement.require_declared_outputs] is true",
                    );
                } else {
                    out.notes
                        .push(format!("plan `{}` declared no output paths", file.path));
                }
            } else {
                for (line_num, path) in planned {
                    if exempt.matches(&path) {
                        continue;
                    }

                    if forbidden.matches(&path) {
                        if let Some(ov) = ctx.find_override(
                            GATE,
                            &crate::findings::PLANNED_FILE_IN_FORBIDDEN_SCOPE,
                            ALLOW_SCOPE,
                            &path,
                        ) {
                            out.overrides.push(ov.clone());
                            out.notes.push(format!(
                                "override applied: `{}: {}` for forbidden planned file `{}` ({})",
                                ov.directive, ov.reason, path, ov.source
                            ));
                        } else {
                            out.add_violation(
                                ctx.overridable(settings.severity),
                                &crate::findings::PLANNED_FILE_IN_FORBIDDEN_SCOPE,
                                &file.path,
                                line_num,
                                format!("planned output `{}` is inside forbidden scope", path),
                                "changes to this path are forbidden by [gates.scope-confinement.forbidden_paths]; use `discipline:allow(scope-confinement): <reason>` or `allow-scope: <path> <reason>` to waive",
                            );
                        }
                        continue;
                    }

                    if has_allowed && !allowed.matches(&path) {
                        if let Some(ov) = ctx.find_override(
                            GATE,
                            &crate::findings::PLANNED_FILE_OUTSIDE_AUTHORIZED_SCOPE,
                            ALLOW_SCOPE,
                            &path,
                        ) {
                            out.overrides.push(ov.clone());
                            out.notes.push(format!(
                                "override applied: `{}: {}` for out-of-scope planned file `{}` ({})",
                                ov.directive, ov.reason, path, ov.source
                            ));
                        } else {
                            out.add_violation(
                                ctx.overridable(settings.severity),
                                &crate::findings::PLANNED_FILE_OUTSIDE_AUTHORIZED_SCOPE,
                                &file.path,
                                line_num,
                                format!("planned output `{}` is outside authorized scope", path),
                                "planned output is not included in [gates.scope-confinement.allowed_paths]; use `discipline:allow(scope-confinement): <reason>` or `allow-scope: <path> <reason>` to waive",
                            );
                        }
                    }
                }
            }
        }

        if plan_files_examined == 0 {
            out.notes.push("no plan files modified in diff".to_string());
        }
    }

    Ok(out)
}

fn clean_candidate_path(s: &str) -> &str {
    let trimmed = s
        .trim()
        .trim_matches(|c: char| matches!(c, '`' | '"' | '\'' | '<' | '>'));
    trimmed.trim_matches(|c: char| matches!(c, ',' | ';' | ':' | ')' | ']' | '(' | '['))
}

fn is_plausible_path(path: &str) -> bool {
    if path.is_empty()
        || path.starts_with("http://")
        || path.starts_with("https://")
        || path == "."
        || path == ".."
        || path.contains(' ')
        || path.contains('\t')
    {
        return false;
    }
    let has_slash = path.contains('/');
    let has_ext = path.rsplit('.').next().is_some_and(|ext| {
        !ext.is_empty()
            && ext.len() <= 6
            && ext.chars().all(|c| c.is_ascii_alphanumeric())
            && ext != path
    });
    has_slash || has_ext
}

fn extract_paths_from_text(text: &str, line_num: usize, outputs: &mut Vec<(usize, String)>) {
    let cleaned = text.trim_matches(|c: char| matches!(c, '[' | ']'));
    for part in cleaned.split([',', ';']) {
        let p = clean_candidate_path(part);
        if is_plausible_path(p) {
            outputs.push((line_num, p.to_string()));
        } else {
            for word in part.split_whitespace() {
                let w = clean_candidate_path(word);
                if is_plausible_path(w) {
                    outputs.push((line_num, w.to_string()));
                }
            }
        }
    }
}

pub fn extract_planned_outputs(content: &str) -> Vec<(usize, String)> {
    let mut outputs = Vec::new();
    let lines: Vec<&str> = content.lines().collect();
    let mut in_front_matter = false;
    let mut in_fm_outputs = false;
    let mut in_body_outputs = false;

    let label_re =
        regex::Regex::new(r"(?i)^\s*(?:[-*]\s*)?(?:outputs?|produces?|writes?):\s*(.*)$").unwrap();
    let heading_re =
        regex::Regex::new(r"(?i)^#+\s*(?:outputs?|planned\s+outputs?|produced\s+files?)\s*$")
            .unwrap();
    let action_re = regex::Regex::new(
        r#"(?i)\b(?:into|update|create|write\s+to)\s+([`'"]?[a-zA-Z0-9_./\-]+[`'"]?)"#,
    )
    .unwrap();

    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;
        let trimmed = line.trim();

        if idx == 0 && trimmed == "---" {
            in_front_matter = true;
            continue;
        }
        if in_front_matter {
            if trimmed == "---" {
                in_front_matter = false;
                in_fm_outputs = false;
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix("outputs:") {
                let rest = rest.trim();
                if rest.is_empty() {
                    in_fm_outputs = true;
                } else {
                    extract_paths_from_text(rest, line_num, &mut outputs);
                }
                continue;
            }
            if in_fm_outputs {
                if let Some(item) = trimmed.strip_prefix('-') {
                    let path = clean_candidate_path(item);
                    if is_plausible_path(path) {
                        outputs.push((line_num, path.to_string()));
                    }
                } else if !trimmed.is_empty() && !trimmed.starts_with('#') {
                    in_fm_outputs = false;
                }
                continue;
            }
        }

        if heading_re.is_match(line) {
            in_body_outputs = true;
            continue;
        }

        if let Some(caps) = label_re.captures(line) {
            if let Some(m) = caps.get(1) {
                let rest = m.as_str().trim();
                if rest.is_empty() {
                    in_body_outputs = true;
                } else {
                    extract_paths_from_text(rest, line_num, &mut outputs);
                }
            }
            continue;
        }

        if in_body_outputs {
            if let Some(item) = trimmed
                .strip_prefix('-')
                .or_else(|| trimmed.strip_prefix('*'))
            {
                let path = clean_candidate_path(item);
                if is_plausible_path(path) {
                    outputs.push((line_num, path.to_string()));
                } else {
                    extract_paths_from_text(item, line_num, &mut outputs);
                }
                continue;
            } else if trimmed.starts_with('#') || (!trimmed.is_empty() && !trimmed.starts_with('`'))
            {
                in_body_outputs = false;
            }
        }

        for caps in action_re.captures_iter(line) {
            if let Some(m) = caps.get(1) {
                let path = clean_candidate_path(m.as_str());
                if is_plausible_path(path) {
                    outputs.push((line_num, path.to_string()));
                }
            }
        }
    }

    let mut deduped = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (line_num, path) in outputs {
        if seen.insert((line_num, path.clone())) {
            deduped.push((line_num, path));
        }
    }
    deduped
}

pub fn check_path_confinement(
    path: &str,
    exempt: &PathFilter,
    allowed: &PathFilter,
    has_allowed: bool,
    forbidden: &PathFilter,
) -> Option<&'static str> {
    if exempt.matches(path) {
        return None;
    }
    if forbidden.matches(path) {
        return Some("forbidden");
    }
    if has_allowed && !allowed.matches(path) {
        return Some("outside-allowed");
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::config::{ScopeConfinementGate, Severity};
    use crate::guards::PathFilter;

    #[test]
    fn test_scope_confinement_defaults() {
        let gate = ScopeConfinementGate::default();
        assert!(!gate.enabled);
        assert_eq!(gate.severity, Severity::Error);
        assert!(gate.allowed_paths.is_empty());
        assert!(gate.forbidden_paths.is_empty());
    }

    /// Calls the gate over a change to `src/lib.rs` with `body` as its table.
    fn evaluate(body: &str) -> anyhow::Result<crate::guards::GateOutcome> {
        let config = crate::config::DisciplineConfig::from_toml_str(&format!(
            "[meta]\nversion = 1\nname = \"t\"\n[gates.scope-confinement]\nenabled = true\n{body}\n"
        ))?;
        let (_dir, git) = crate::gitctx::test_support::repo_with_changed_file(
            "src/lib.rs",
            "pub fn a() {}\n",
            "pub fn a() {}\npub fn b() {}\n",
        );
        super::evaluate_scope_confinement(&crate::guards::test_support::context(&config, &git))
    }

    /// `run_checks` compiles every glob before a gate runs, so the binary never brings
    /// an invalid one this far; the gate is called directly to reach its own refusal.
    #[test]
    fn invalid_glob_is_an_error_not_a_silently_skipped_pattern() {
        for key in ["exempt_paths", "allowed_paths", "forbidden_paths"] {
            let err = evaluate(&format!("{key} = [\"[\"]"))
                .err()
                .unwrap_or_else(|| panic!("`{key}` with an invalid glob was accepted"));
            let text = format!("{err:#}");
            assert!(
                text.contains("invalid glob `[` in configuration"),
                "{key}: {text}"
            );
        }
    }

    /// Control: the same change under globs that compile is judged, not refused.
    #[test]
    fn the_gate_judges_the_change_when_its_globs_compile() {
        let forbidden = evaluate("forbidden_paths = [\"src/**\"]").unwrap();
        assert_eq!(forbidden.examined, 1);
        assert_eq!(forbidden.violations.len(), 1, "{:?}", forbidden.notes);
        let allowed =
            evaluate("allowed_paths = [\"src/**\"]\nexempt_paths = [\"docs/**\"]").unwrap();
        assert_eq!(allowed.examined, 1);
        assert!(allowed.violations.is_empty(), "{:?}", allowed.notes);
    }

    #[test]
    fn valid_globs_still_match_through_check_path_confinement() {
        let exempt = PathFilter::new(&["tests/fixtures/**".to_string()]).unwrap();
        let allowed = PathFilter::new(&["src/**".to_string()]).unwrap();
        let forbidden = PathFilter::new(&[".github/**".to_string()]).unwrap();
        assert!(
            super::check_path_confinement("src/lib.rs", &exempt, &allowed, true, &forbidden)
                .is_none()
        );
        assert_eq!(
            super::check_path_confinement(
                ".github/workflows/ci.yml",
                &exempt,
                &allowed,
                true,
                &forbidden
            ),
            Some("forbidden")
        );
        assert_eq!(
            super::check_path_confinement("docs/guide.md", &exempt, &allowed, true, &forbidden),
            Some("outside-allowed")
        );
        assert!(super::check_path_confinement(
            "tests/fixtures/data.bin",
            &exempt,
            &allowed,
            true,
            &forbidden
        )
        .is_none());
    }

    #[test]
    fn test_extract_planned_outputs_motivating_example() {
        let content = "Step 2: Generate test outputs into benchmarks/baselines/results.json and update scripts/guardrails/check.sh\n";
        let outputs = super::extract_planned_outputs(content);
        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[0].1, "benchmarks/baselines/results.json");
        assert_eq!(outputs[1].1, "scripts/guardrails/check.sh");
    }

    #[test]
    fn test_extract_planned_outputs_front_matter_and_labels() {
        let content = "---\noutputs:\n  - src/core/feature.rs\n  - docs/plans/out.md\n---\n# Plan\n- Outputs: lib/utils.rs, `lib/helper.rs`\n";
        let outputs = super::extract_planned_outputs(content);
        assert!(outputs.iter().any(|(_, p)| p == "src/core/feature.rs"));
        assert!(outputs.iter().any(|(_, p)| p == "docs/plans/out.md"));
        assert!(outputs.iter().any(|(_, p)| p == "lib/utils.rs"));
        assert!(outputs.iter().any(|(_, p)| p == "lib/helper.rs"));
    }

    #[test]
    fn test_plan_checks_forbidden_and_allowed_and_directive() {
        let config_toml = r#"
[meta]
version = 1
name = "t"
[gates.scope-confinement]
enabled = true
check_plans = true
forbidden_paths = ["scripts/guardrails/**", "benchmarks/baselines/**"]
allowed_paths = ["src/**", "plans/**"]
exempt_paths = ["exempt/**"]
"#;
        let config = crate::config::DisciplineConfig::from_toml_str(config_toml).unwrap();
        let (_dir, git) = crate::gitctx::test_support::repo_with_changed_file(
            "plans/feature.md",
            "",
            "Step 2: Generate test outputs into benchmarks/baselines/results.json and update scripts/guardrails/check.sh\n",
        );
        let out =
            super::evaluate_scope_confinement(&crate::guards::test_support::context(&config, &git))
                .unwrap();
        assert_eq!(out.violations.len(), 2);
        assert_eq!(
            out.violations[0].code,
            "scope-confinement/planned-file-in-forbidden-scope"
        );
        assert_eq!(
            out.violations[1].code,
            "scope-confinement/planned-file-in-forbidden-scope"
        );

        // Allowed output passes
        let (_dir, git) = crate::gitctx::test_support::repo_with_changed_file(
            "plans/feature.md",
            "",
            "- Outputs: src/core/feature.rs\n",
        );
        let out =
            super::evaluate_scope_confinement(&crate::guards::test_support::context(&config, &git))
                .unwrap();
        assert!(out.violations.is_empty(), "{:?}", out.violations);

        // Exempt path passes
        let (_dir, git) = crate::gitctx::test_support::repo_with_changed_file(
            "plans/feature.md",
            "",
            "- Outputs: exempt/scratch.txt\n",
        );
        let out =
            super::evaluate_scope_confinement(&crate::guards::test_support::context(&config, &git))
                .unwrap();
        assert!(out.violations.is_empty(), "{:?}", out.violations);

        // Waived with directive
        let (_dir, git) = crate::gitctx::test_support::repo_with_changed_file(
            "plans/feature.md",
            "",
            "- Outputs: scripts/guardrails/check.sh\n",
        );
        let mut ctx = crate::guards::test_support::context(&config, &git);
        ctx.directives = vec![crate::tokens::ParsedDirective {
            directive: "allow-scope".to_string(),
            reason: "scripts/guardrails/check.sh authorized infra upgrade".to_string(),
            source: crate::tokens::OverrideSource::Commit("head".to_string()),
            hidden: false,
        }];
        let out = super::evaluate_scope_confinement(&ctx).unwrap();
        assert!(out.violations.is_empty(), "{:?}", out.violations);
        assert_eq!(out.overrides.len(), 1);

        // Require declared outputs
        let req_toml = r#"
[meta]
version = 1
name = "t"
[gates.scope-confinement]
enabled = true
check_plans = true
require_declared_outputs = true
"#;
        let req_config = crate::config::DisciplineConfig::from_toml_str(req_toml).unwrap();
        let (_dir, git) = crate::gitctx::test_support::repo_with_changed_file(
            "plans/feature.md",
            "",
            "Just some notes with no outputs declared.\n",
        );
        let out = super::evaluate_scope_confinement(&crate::guards::test_support::context(
            &req_config,
            &git,
        ))
        .unwrap();
        assert_eq!(out.violations.len(), 1);
        assert_eq!(
            out.violations[0].code,
            "scope-confinement/plan-without-declared-outputs"
        );
    }
}
