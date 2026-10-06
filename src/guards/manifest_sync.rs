//! Packaging manifest synchronization sentinel (`manifest-sync`).
//!
//! Reconciles git-tracked files in declared directories against packaging
//! manifest declarations (e.g. `package.xml`, `.nuspec`) to ensure no unbundled
//! files exist and no phantom files are packaged.

use crate::guards::{Context, GateOutcome, PathFilter};
use crate::tokens;
use anyhow::{bail, Context as _, Result};
use regex::Regex;
use std::collections::BTreeSet;

pub const GATE: &str = "manifest-sync";

/// Evaluates git-tracked files against manifest declarations.
pub fn evaluate_manifest_sync(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.manifest_sync;
    let mut out = GateOutcome::new(GATE);

    if !settings.enabled {
        out.enabled = false;
        return Ok(out);
    }

    if settings.rules.is_empty() {
        bail!("manifest-sync gate is enabled but no rules are configured");
    }

    let tracked_files = ctx.git.tracked_files()?;
    let root = ctx.git.root();
    let mut total_examined = 0;

    for (position, rule) in settings.rules.iter().enumerate() {
        let manifest_path = root.join(&rule.manifest);
        if !manifest_path.is_file() {
            if ctx.predates_config(&rule.manifest)? {
                out.notes.push(format!(
                    "manifest `{}` skipped: it is not in this change's tree {}",
                    rule.manifest,
                    super::PREDATES_CONFIG_NOTE
                ));
                continue;
            }
            bail!("manifest `{}` does not exist", rule.manifest);
        }

        let manifest_content = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed to read manifest `{}`", rule.manifest))?;

        let re = Regex::new(&rule.extract_regex).with_context(|| {
            format!(
                "invalid extract_regex `{}` in rule for `{}`",
                rule.extract_regex, rule.manifest
            )
        })?;

        let mut manifest_files = BTreeSet::new();
        for cap in re.captures_iter(&manifest_content) {
            let path_str = cap
                .get(1)
                .or_else(|| cap.get(0))
                .map(|m| m.as_str().trim())
                .unwrap_or("");
            if !path_str.is_empty() {
                let normalized = path_str
                    .replace('\\', "/")
                    .trim_start_matches("./")
                    .to_string();
                manifest_files.insert(normalized);
            }
        }

        let watched = PathFilter::new(&rule.watched_paths)
            .with_context(|| format!("invalid watched glob in manifest `{}`", rule.manifest))?;

        let exclude = PathFilter::new(&rule.exclude_paths)
            .with_context(|| format!("invalid exclude glob in manifest `{}`", rule.manifest))?;

        let mut watched_git_files = BTreeSet::new();
        for f in &tracked_files {
            let norm = f.replace('\\', "/");
            if watched.matches(&norm) && !exclude.matches(&norm) {
                watched_git_files.insert(norm);
            }
        }

        if !watched_git_files.is_empty() && manifest_files.is_empty() {
            bail!(
                "manifest `{}` extracted 0 entries with regex `{}` while {} files were watched",
                rule.manifest,
                rule.extract_regex,
                watched_git_files.len()
            );
        }

        total_examined += watched_git_files.len() + manifest_files.len();

        let mut unmanifested = Vec::new();
        for path in &watched_git_files {
            if !manifest_files.contains(path) {
                let allowed = ctx
                    .find_override(
                        GATE,
                        &crate::findings::MANIFEST_DRIFT,
                        tokens::ALLOW_MANIFEST_DRIFT,
                        path,
                    )
                    .or_else(|| {
                        ctx.find_override(
                            GATE,
                            &crate::findings::MANIFEST_DRIFT,
                            tokens::ALLOW_MANIFEST_DRIFT,
                            &rule.manifest,
                        )
                    });

                if let Some(ov) = allowed {
                    out.overrides.push(ov);
                } else {
                    unmanifested.push(path.clone());
                }
            }
        }

        let mut ghost = Vec::new();
        for path in &manifest_files {
            if watched.matches(path) && !exclude.matches(path) && !watched_git_files.contains(path)
            {
                let allowed = ctx
                    .find_override(
                        GATE,
                        &crate::findings::MANIFEST_DRIFT,
                        tokens::ALLOW_MANIFEST_DRIFT,
                        path,
                    )
                    .or_else(|| {
                        ctx.find_override(
                            GATE,
                            &crate::findings::MANIFEST_DRIFT,
                            tokens::ALLOW_MANIFEST_DRIFT,
                            &rule.manifest,
                        )
                    });

                if let Some(ov) = allowed {
                    out.overrides.push(ov);
                } else {
                    ghost.push(path.clone());
                }
            }
        }

        if !unmanifested.is_empty() || !ghost.is_empty() {
            let mut diff_lines = Vec::new();
            for p in &unmanifested {
                diff_lines.push(format!("+ {}", p));
            }
            for p in &ghost {
                diff_lines.push(format!("- {}", p));
            }

            out.push(
                ctx.overridable(settings.severity),
                &crate::findings::MANIFEST_DRIFT,
                Some(&rule.manifest),
                None,
                format!(
                    "manifest `{}` is out of sync with git-tracked files ({} drift(s)):\n{}",
                    rule.manifest,
                    diff_lines.len(),
                    diff_lines.join("\n")
                ),
                "update the packaging manifest to match git-tracked files or justify with `allow-manifest-drift: <manifest> <reason>`",
            );
            if let Some(anchor) = later_rule_anchor(&settings.rules, position) {
                out.anchor_last(anchor);
            }
        }
    }

    out.examined = total_examined;
    Ok(out)
}

/// What tells the finding of the rule at `position` from that of an earlier rule naming
/// the same manifest: a hash of what the rule extracts and watches. `None` for the first
/// rule that names a manifest, whose finding is identified by its code and path as it
/// always was. The hash, not the position, so a rule added or removed elsewhere in the
/// list changes nothing.
pub(crate) fn later_rule_anchor(
    rules: &[crate::config::ManifestSyncRule],
    position: usize,
) -> Option<String> {
    let rule = &rules[position];
    if !rules[..position]
        .iter()
        .any(|r| r.manifest == rule.manifest)
    {
        return None;
    }
    let mut patterns = vec![rule.extract_regex.as_str(), "watched"];
    patterns.extend(rule.watched_paths.iter().map(String::as_str));
    patterns.push("exclude");
    patterns.extend(rule.exclude_paths.iter().map(String::as_str));
    Some(format!(
        "rule:{}",
        crate::report::gitlab::sha256_hex(patterns.join("\u{1f}").as_bytes())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_regex_extraction_package_xml() {
        let xml = r#"
        <package>
            <contents>
                <dir name="/">
                    <file name="tests/001.phpt" role="test" />
                    <file name="tests/002.phpt" role="test" />
                    <file name="libjudy/Judy1.c" role="src" />
                </dir>
            </contents>
        </package>
        "#;
        let re = Regex::new(r#"<file\s+name="([^"]+)""#).unwrap();
        let mut extracted = Vec::new();
        for cap in re.captures_iter(xml) {
            extracted.push(cap.get(1).unwrap().as_str().to_string());
        }
        assert_eq!(extracted.len(), 3);
        assert!(extracted.contains(&"tests/001.phpt".to_string()));
        assert!(extracted.contains(&"libjudy/Judy1.c".to_string()));
    }
}
