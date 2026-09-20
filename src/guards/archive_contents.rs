//! Distribution archive contents sentinel (`archive-contents`).
//!
//! Validates built distribution archives before publishing, asserting that
//! required files exist and developer tooling / private artifacts do not leak.

use crate::guards::{Context, GateOutcome};
use crate::tokens;
use anyhow::{bail, Context as _, Result};
use globset::GlobBuilder;
use regex::Regex;
use std::fs::File;
use std::path::{Path, PathBuf};

pub const GATE: &str = "archive-contents";

/// Evaluates archive contents against required and forbidden rules.
pub fn evaluate_archive_contents(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.archive_contents;
    let mut out = GateOutcome::new(GATE);

    if !settings.enabled {
        out.enabled = false;
        return Ok(out);
    }

    let Some(archive_glob) = &settings.archive_path else {
        bail!("archive-contents gate is enabled but `archive_path` is not configured");
    };
    if archive_glob.trim().is_empty() {
        bail!("archive-contents gate is enabled but `archive_path` is empty");
    }

    let root = ctx.git.root();
    let matches = find_matching_archives(root, archive_glob)?;

    if matches.is_empty() {
        bail!(
            "archive_path `{}` matched 0 files; built distribution archive is missing",
            archive_glob
        );
    }
    if matches.len() > 1 {
        let names: Vec<_> = matches
            .iter()
            .map(|p| p.strip_prefix(root).unwrap_or(p).display().to_string())
            .collect();
        bail!(
            "archive_path `{}` matched multiple conflicting files ({}); specify an unambiguous archive path",
            archive_glob,
            names.join(", ")
        );
    }

    let archive_path = &matches[0];
    let rel_archive_display = archive_path
        .strip_prefix(root)
        .unwrap_or(archive_path)
        .display()
        .to_string();

    let entries =
        read_archive_entries(archive_path, settings.strip_components).with_context(|| {
            format!(
                "archive `{}` is corrupt, empty, or unreadable",
                rel_archive_display
            )
        })?;

    if entries.is_empty() {
        bail!(
            "archive `{}` is corrupt, empty, or unreadable: archive contains zero entries",
            rel_archive_display
        );
    }

    out.examined = entries.len();

    // 1. Check required paths
    let mut missing_paths = Vec::new();
    for req in &settings.required_paths {
        let found = entries.iter().any(|entry| {
            if req.contains('*') || req.contains('?') {
                GlobBuilder::new(req)
                    .literal_separator(false)
                    .build()
                    .map(|g| g.compile_matcher().is_match(entry))
                    .unwrap_or(false)
            } else {
                entry == req
            }
        });
        if !found {
            missing_paths.push(req.clone());
        }
    }

    if !missing_paths.is_empty() {
        out.push(
            ctx.overridable(settings.severity),
            "Missing Required Archive Path",
            Some(&rel_archive_display),
            None,
            format!(
                "archive `{}` is missing required distribution path(s): {}",
                rel_archive_display,
                missing_paths.join(", ")
            ),
            "ensure all required package files are bundled into the distribution archive",
        );
    }

    // 2. Check forbidden patterns
    let forbidden_regexes: Vec<(String, Regex)> = settings
        .forbidden_patterns
        .iter()
        .map(|p| {
            Regex::new(p)
                .map(|r| (p.clone(), r))
                .with_context(|| format!("invalid regex `{p}` in forbidden_patterns"))
        })
        .collect::<Result<_, _>>()?;

    let mut forbidden_violations = Vec::new();
    for entry in &entries {
        for (pattern_str, re) in &forbidden_regexes {
            if re.is_match(entry) {
                let allowed = ctx
                    .find_override(GATE, tokens::ALLOW_ARCHIVE_LEAK, entry)
                    .or_else(|| ctx.find_override(GATE, tokens::ALLOW_ARCHIVE_LEAK, pattern_str));

                if let Some(ov) = allowed {
                    out.overrides.push(ov);
                } else {
                    forbidden_violations.push((entry.clone(), pattern_str.clone()));
                }
            }
        }
    }

    if !forbidden_violations.is_empty() {
        let details = forbidden_violations
            .iter()
            .map(|(path, pat)| format!("  - {} (matches pattern `{}`)", path, pat))
            .collect::<Vec<_>>()
            .join("\n");

        out.push(
            ctx.overridable(settings.severity),
            "Forbidden Entry Found in Archive",
            Some(&rel_archive_display),
            None,
            format!(
                "archive `{}` contains {} forbidden developer or private artifact(s):\n{}",
                rel_archive_display,
                forbidden_violations.len(),
                details
            ),
            "remove forbidden files from the archive or justify with `allow-archive-leak: <pattern> <reason>`",
        );
    }

    Ok(out)
}

/// Discovers archive files matching `pattern` relative to `root`.
fn find_matching_archives(root: &Path, pattern: &str) -> Result<Vec<PathBuf>> {
    let glob = GlobBuilder::new(pattern)
        .literal_separator(false)
        .build()
        .with_context(|| format!("invalid glob pattern `{pattern}` in archive_path"))?
        .compile_matcher();

    let mut matches = Vec::new();
    let scan_target = pattern.starts_with("target/") || pattern.starts_with("target\\");
    find_archives_recursive(root, root, &glob, scan_target, &mut matches)?;
    matches.sort();
    Ok(matches)
}

fn find_archives_recursive(
    current: &Path,
    root: &Path,
    glob: &globset::GlobMatcher,
    scan_target: bool,
    matches: &mut Vec<PathBuf>,
) -> Result<()> {
    if !current.is_dir() {
        return Ok(());
    }

    let entries = match std::fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();

        if path.is_dir() {
            if name_str == ".git"
                || name_str == ".claude"
                || name_str == ".gemini"
                || name_str == ".antigravity"
                || name_str == "node_modules"
                || (!scan_target && name_str == "target")
            {
                continue;
            }
            find_archives_recursive(&path, root, glob, scan_target, matches)?;
        } else if path.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                if glob.is_match(&rel_str) {
                    matches.push(path);
                }
            }
        }
    }

    Ok(())
}

/// Reads relative paths from an archive, stripping `strip_components` leading directory elements.
pub fn read_archive_entries(archive_path: &Path, strip_components: usize) -> Result<Vec<String>> {
    let file = File::open(archive_path)?;
    let mut raw_paths = Vec::new();

    let path_str = archive_path.to_string_lossy().to_lowercase();
    if path_str.ends_with(".tar.gz") || path_str.ends_with(".tgz") || path_str.ends_with(".crate") {
        let gz = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(gz);
        for entry in archive.entries()? {
            let entry = entry?;
            let path = entry.path()?;
            raw_paths.push(path.to_string_lossy().replace('\\', "/"));
        }
    } else if path_str.ends_with(".tar.bz2") || path_str.ends_with(".tbz2") {
        let bz = bzip2_rs::DecoderReader::new(file);
        let mut archive = tar::Archive::new(bz);
        for entry in archive.entries()? {
            let entry = entry?;
            let path = entry.path()?;
            raw_paths.push(path.to_string_lossy().replace('\\', "/"));
        }
    } else if path_str.ends_with(".zip") {
        let mut zip = zip::ZipArchive::new(file)?;
        for i in 0..zip.len() {
            let f = zip.by_index(i)?;
            raw_paths.push(f.name().replace('\\', "/"));
        }
    } else if path_str.ends_with(".tar") {
        let mut archive = tar::Archive::new(file);
        for entry in archive.entries()? {
            let entry = entry?;
            let path = entry.path()?;
            raw_paths.push(path.to_string_lossy().replace('\\', "/"));
        }
    } else {
        // Generic fallback: try gzip tar first, then zip
        if let Ok(file_clone) = File::open(archive_path) {
            let gz = flate2::read::GzDecoder::new(file_clone);
            let mut archive = tar::Archive::new(gz);
            if let Ok(entries) = archive.entries() {
                for entry in entries.flatten() {
                    if let Ok(p) = entry.path() {
                        raw_paths.push(p.to_string_lossy().replace('\\', "/"));
                    }
                }
            }
        }
        if raw_paths.is_empty() {
            if let Ok(file_clone) = File::open(archive_path) {
                if let Ok(mut zip) = zip::ZipArchive::new(file_clone) {
                    for i in 0..zip.len() {
                        if let Ok(f) = zip.by_index(i) {
                            raw_paths.push(f.name().replace('\\', "/"));
                        }
                    }
                }
            }
        }
        if raw_paths.is_empty() {
            bail!(
                "unrecognized or unreadable archive format for `{}`",
                archive_path.display()
            );
        }
    }

    let mut stripped_paths = Vec::new();
    for raw in raw_paths {
        let clean = raw.trim_start_matches("./");
        let parts: Vec<&str> = clean
            .split('/')
            .filter(|s| !s.is_empty() && *s != ".")
            .collect();

        if parts.len() <= strip_components {
            continue;
        }

        let stripped = parts[strip_components..].join("/");
        if !stripped.is_empty() && !stripped_paths.contains(&stripped) {
            stripped_paths.push(stripped);
        }
    }

    Ok(stripped_paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_read_archive_tar_gz_with_strip_components() {
        let dir = tempdir().unwrap();
        let tar_gz_path = dir.path().join("Judy-2.6.0.tgz");

        let f = File::create(&tar_gz_path).unwrap();
        let enc = GzEncoder::new(f, Compression::default());
        let mut tar = tar::Builder::new(enc);

        let data = b"echo 'hello'";
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, "Judy-2.6.0/config.m4", &data[..])
            .unwrap();

        let mut header2 = tar::Header::new_gnu();
        header2.set_size(data.len() as u64);
        header2.set_mode(0o644);
        header2.set_cksum();
        tar.append_data(&mut header2, "Judy-2.6.0/tools/check.sh", &data[..])
            .unwrap();

        let enc = tar.into_inner().unwrap();
        enc.finish().unwrap();

        let entries = read_archive_entries(&tar_gz_path, 1).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.contains(&"config.m4".to_string()));
        assert!(entries.contains(&"tools/check.sh".to_string()));
    }

    #[test]
    fn test_read_archive_zip() {
        let dir = tempdir().unwrap();
        let zip_path = dir.path().join("package.zip");

        let f = File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(f);
        let options = zip::write::FileOptions::default();

        zip.start_file("pkg/php_judy.h", options).unwrap();
        zip.write_all(b"#define PHP_JUDY_VERSION 2.6.0").unwrap();
        zip.finish().unwrap();

        let entries = read_archive_entries(&zip_path, 1).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], "php_judy.h");
    }
}
