//! Version declaration lockstep sentinel (`version-lockstep`).
//!
//! Guarantees that version declarations across multiple files (C headers,
//! packaging manifests, docs, lockfiles) remain strictly synchronized.

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

        let baseline_ver = &extracted[0].1;
        let has_mismatch = extracted.iter().any(|(_, v)| v != baseline_ver);

        if has_mismatch {
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
                    .map(|(p, v)| format!("  - `{}` declares `{}`", p, v))
                    .collect::<Vec<_>>()
                    .join("\n");

                out.push(
                    ctx.overridable(settings.severity),
                    "Version Declaration Lockstep Mismatch",
                    Some(&group.name),
                    None,
                    format!(
                        "version declarations in group `{}` disagree across sources:\n{}",
                        group.name, details
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_regexes() {
        let c_header = "#define PHP_JUDY_VERSION \"2.6.0\"\n";
        let c_re = Regex::new(r#"#define\s+PHP_JUDY_VERSION\s+"([^"]+)""#).unwrap();
        let cap = c_re.captures(c_header).unwrap();
        assert_eq!(cap.get(1).unwrap().as_str(), "2.6.0");

        let xml = "<release>2.6.0</release>";
        let xml_re = Regex::new(r#"<release>([^<]+)</release>"#).unwrap();
        let cap2 = xml_re.captures(xml).unwrap();
        assert_eq!(cap2.get(1).unwrap().as_str(), "2.6.0");
    }
}
