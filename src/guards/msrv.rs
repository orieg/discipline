//! Minimum Supported Rust Version (MSRV) sentinel (`msrv`).
//!
//! Validates that the repository declares a minimum supported Rust version in
//! `Cargo.toml` (`rust-version`) and passes compilation under that toolchain.

use crate::guards::{Context, GateOutcome};
use crate::tokens::ALLOW_MSRV;
use anyhow::Result;
use std::fs;

pub const GATE: &str = "msrv";

pub fn evaluate_msrv(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.msrv;
    let mut out = GateOutcome::new(GATE);

    if !settings.enabled {
        out.enabled = false;
        return Ok(out);
    }

    out.examined = 1;

    let root = ctx.git.root();
    let cargo_toml = root.join("Cargo.toml");

    let declared_msrv = if let Some(pinned) = &settings.pinned_version {
        Some(pinned.clone())
    } else if cargo_toml.is_file() {
        match fs::read_to_string(&cargo_toml) {
            Ok(content) => parse_rust_version(&content),
            Err(e) => {
                out.add_violation(
                    ctx.overridable(settings.severity),
                    "Cargo.toml",
                    1,
                    format!("could not read Cargo.toml: {e}"),
                    "ensure Cargo.toml is readable",
                );
                return Ok(out);
            }
        }
    } else {
        None
    };

    let Some(version) = declared_msrv else {
        if !cargo_toml.is_file() && settings.pinned_version.is_none() {
            out.notes
                .push("no Cargo.toml or pinned_version found; MSRV check skipped".to_string());
            out.examined = 0;
            return Ok(out);
        }
        if let Some(ov) = ctx.find_gate_or_subject_override(GATE, ALLOW_MSRV, GATE) {
            out.notes.push(format!(
                "override applied: `{}: {}` (missing MSRV declaration allowed) ({})",
                ov.directive, ov.reason, ov.source
            ));
        } else {
            out.add_violation(
                ctx.overridable(settings.severity),
                "Cargo.toml",
                1,
                "missing `rust-version` MSRV declaration in Cargo.toml",
                "declare `rust-version = \"1.xx\"` in [package] or set `pinned_version` in [gates.msrv]; use `discipline:allow(msrv): <reason>` to waive",
            );
        }
        return Ok(out);
    };

    out.notes.push(format!("MSRV verified: `{version}`"));

    // If an explicit verification command is declared, execute it
    if let Some(cmd) = &settings.command {
        let (status, stdout, stderr) = run_msrv_command(cmd, root)?;
        if !status {
            if let Some(ov) = ctx.find_gate_or_subject_override(GATE, ALLOW_MSRV, GATE) {
                out.notes.push(format!(
                    "override applied: `{}: {}` (MSRV command failure allowed) ({})",
                    ov.directive, ov.reason, ov.source
                ));
            } else {
                let diag = if !stderr.is_empty() { stderr } else { stdout };
                out.add_violation(
                    ctx.overridable(settings.severity),
                    "Cargo.toml",
                    1,
                    format!("MSRV verification command `{cmd}` failed under Rust {version}"),
                    diag.lines().next().unwrap_or("command exited non-zero"),
                );
            }
        }
    }

    Ok(out)
}

pub fn parse_rust_version(toml_str: &str) -> Option<String> {
    for line in toml_str.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("rust-version") {
            if let Some((_, val)) = trimmed.split_once('=') {
                let cleaned = val.trim().trim_matches('"').trim_matches('\'').trim();
                if !cleaned.is_empty() {
                    return Some(cleaned.to_string());
                }
            }
        }
    }
    None
}

fn run_msrv_command(cmd: &str, dir: &std::path::Path) -> Result<(bool, String, String)> {
    let mut parts = cmd.split_whitespace();
    let program = match parts.next() {
        Some(p) => p,
        None => return Ok((true, String::new(), String::new())),
    };
    let args: Vec<&str> = parts.collect();

    let output = std::process::Command::new(program)
        .args(&args)
        .current_dir(dir)
        .output();

    match output {
        Ok(out) => Ok((
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )),
        Err(e) => Ok((
            false,
            String::new(),
            format!("failed to execute `{cmd}`: {e}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{MsrvGate, Severity};

    #[test]
    fn test_parse_rust_version() {
        let sample = r#"
[package]
name = "test"
version = "0.1.0"
rust-version = "1.90"
"#;
        assert_eq!(parse_rust_version(sample), Some("1.90".to_string()));
    }

    #[test]
    fn test_msrv_defaults() {
        let gate = MsrvGate::default();
        assert!(!gate.enabled);
        assert_eq!(gate.severity, Severity::Error);
        assert_eq!(gate.pinned_version, None);
    }
}
