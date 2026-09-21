//! Version declaration lockstep sentinel (`version-lockstep`).
//!
//! Guarantees that version declarations across multiple files (C headers,
//! packaging manifests, docs, lockfiles) remain strictly synchronized. A
//! mismatch is reported against the file that drifted from the group's
//! consensus version.

use crate::guards::{Context, GateOutcome};
use crate::tokens;
use anyhow::{bail, Context as _, Result};
use regex::Regex;

pub const GATE: &str = "version-lockstep";

/// Evaluates multi-source version lockstep consistency.
pub fn evaluate_version_lockstep(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.version_lockstep;
    let mut out = GateOutcome::new(GATE);

    if !settings.enabled {
        out.enabled = false;
        return Ok(out);
    }

    if settings.groups.is_empty() {
        bail!("version-lockstep gate is enabled but no groups are configured");
    }

    let root = ctx.git.root();
    let mut total_examined = 0;

    for group in &settings.groups {
        if group.sources.len() < 2 {
            bail!(
                "version-lockstep group `{}` requires at least 2 sources to compare, got {}",
                group.name,
                group.sources.len()
            );
        }

        let mut extracted: Vec<(String, String)> = Vec::new();
        for source in &group.sources {
            let path = root.join(&source.path);
            if !path.is_file() {
                bail!(
                    "version-lockstep group `{}`: source file `{}` does not exist",
                    group.name,
                    source.path
                );
            }

            let content = std::fs::read_to_string(&path).with_context(|| {
                format!(
                    "failed to read source file `{}` in group `{}`",
                    source.path, group.name
                )
            })?;

            let re = Regex::new(&source.regex).with_context(|| {
                format!(
                    "invalid regex `{}` for source `{}` in group `{}`",
                    source.regex, source.path, group.name
                )
            })?;

            let Some(caps) = re.captures(&content) else {
                bail!(
                    "version-lockstep group `{}`: regex `{}` failed to match in `{}`",
                    group.name,
                    source.regex,
                    source.path
                );
            };

            let ver = caps
                .get(1)
                .or_else(|| caps.get(0))
                .map(|m| m.as_str().trim())
                .unwrap_or("")
                .to_string();

            if ver.is_empty() {
                bail!(
                    "version-lockstep group `{}`: regex `{}` extracted an empty version in `{}`",
                    group.name,
                    source.regex,
                    source.path
                );
            }

            extracted.push((source.path.clone(), ver));
        }

        total_examined += extracted.len();

        let reference = consensus_version(&extracted);
        let drifted: Vec<&str> = extracted
            .iter()
            .filter(|(_, v)| v != reference)
            .map(|(p, _)| p.as_str())
            .collect();

        if let Some(first_drifted) = drifted.first().copied() {
            let allowed = ctx
                .find_override(GATE, tokens::ALLOW_VERSION_MISMATCH, &group.name)
                .or_else(|| {
                    extracted.iter().find_map(|(p, _)| {
                        ctx.find_override(GATE, tokens::ALLOW_VERSION_MISMATCH, p)
                    })
                });

            if let Some(ov) = allowed {
                out.overrides.push(ov);
            } else {
                let details = extracted
                    .iter()
                    .map(|(p, v)| {
                        let mark = if v != reference { " (drifted)" } else { "" };
                        format!("  - `{p}` declares `{v}`{mark}")
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                // The finding points at the file that drifted from the group's
                // consensus, so an annotation lands where the edit is needed.
                out.push(
                    ctx.overridable(settings.severity),
                    "Version Declaration Lockstep Mismatch",
                    Some(first_drifted),
                    None,
                    format!(
                        "version declarations in group `{}` disagree: {} drifted from `{}`:\n{}",
                        group.name,
                        drifted
                            .iter()
                            .map(|p| format!("`{p}`"))
                            .collect::<Vec<_>>()
                            .join(", "),
                        reference,
                        details
                    ),
                    &format!(
                        "synchronize version strings across all sources in group `{}` or justify with `allow-version-mismatch: {} <reason>`",
                        group.name, group.name
                    ),
                );
            }
        }
    }

    out.examined = total_examined;
    Ok(out)
}

/// The version most sources in a group agree on; a tie goes to the version
/// declared first, so the group's first source is the reference for a pair.
fn consensus_version(extracted: &[(String, String)]) -> &str {
    let mut best: Option<(&str, usize)> = None;
    for (_, candidate) in extracted {
        let count = extracted.iter().filter(|(_, v)| v == candidate).count();
        if best.is_none_or(|(_, c)| count > c) {
            best = Some((candidate.as_str(), count));
        }
    }
    best.map(|(v, _)| v).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_regexes() {
        let c_header = "#define EXAMPLE_VERSION \"2.6.0\"\n";
        let c_re = Regex::new(r#"#define\s+EXAMPLE_VERSION\s+"([^"]+)""#).unwrap();
        let cap = c_re.captures(c_header).unwrap();
        assert_eq!(cap.get(1).unwrap().as_str(), "2.6.0");

        let xml = "<release>2.6.0</release>";
        let xml_re = Regex::new(r#"<release>([^<]+)</release>"#).unwrap();
        let cap2 = xml_re.captures(xml).unwrap();
        assert_eq!(cap2.get(1).unwrap().as_str(), "2.6.0");
    }

    fn sources(versions: &[&str]) -> Vec<(String, String)> {
        versions
            .iter()
            .enumerate()
            .map(|(i, v)| (format!("f{i}"), v.to_string()))
            .collect()
    }

    #[test]
    fn consensus_is_the_majority_version() {
        assert_eq!(consensus_version(&sources(&["1.0", "1.1", "1.1"])), "1.1");
        assert_eq!(consensus_version(&sources(&["1.1", "1.1", "1.0"])), "1.1");
    }

    #[test]
    fn consensus_tie_goes_to_the_first_declared_version() {
        assert_eq!(consensus_version(&sources(&["1.0", "1.1"])), "1.0");
        assert_eq!(
            consensus_version(&sources(&["2.0", "1.0", "1.0", "2.0"])),
            "2.0"
        );
    }
}
