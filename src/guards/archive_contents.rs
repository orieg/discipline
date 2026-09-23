//! Distribution archive contents sentinel (`archive-contents`).
//!
//! Validates built distribution archives before publishing, asserting that
//! required files exist and developer tooling / private artifacts do not leak.

use crate::config::Severity;
use crate::guards::archive_formats::{self, EntryData};
use crate::guards::source_maps::{self, MapRef, MapVerdict};
use crate::guards::{Context, GateOutcome};
use crate::tokens;
use anyhow::{bail, Context as _, Result};
use globset::GlobBuilder;
use regex::Regex;
use std::io::Read;
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

    let scan_limit = if settings.scan_contents {
        if settings.max_entry_bytes == 0 {
            bail!(
                "archive-contents `max_entry_bytes` must be at least 1 when `scan_contents` is on"
            );
        }
        Some(settings.max_entry_bytes)
    } else {
        None
    };
    let ArchiveRead { entries, scan } =
        read_archive(archive_path, settings.strip_components, scan_limit).with_context(|| {
            format!(
                "archive `{}` could not be read (corrupt, truncated, or a format archive-contents does not analyse)",
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

    if let Some(scan) = scan {
        report_scan(ctx, &rel_archive_display, &entries, scan, &mut out);
    }

    Ok(out)
}

/// How many entries a finding or note lists before summarising the rest.
const LISTED: usize = 20;

fn listing(items: &[String]) -> String {
    let mut out = items
        .iter()
        .take(LISTED)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if items.len() > LISTED {
        out.push_str(&format!(" (+{} more)", items.len() - LISTED));
    }
    out
}

/// Turns the content scan into findings and notes.
fn report_scan(
    ctx: &Context,
    archive: &str,
    entries: &[String],
    scan: ScanReport,
    out: &mut GateOutcome,
) {
    let settings = &ctx.config.gates.archive_contents;

    let mut leaks = Vec::new();
    for leak in &scan.leaks {
        if let Some(ov) = ctx.find_override(GATE, tokens::ALLOW_ARCHIVE_LEAK, &leak.entry) {
            out.overrides.push(ov);
            continue;
        }
        let shown = leak.sources.join(", ");
        let more = leak.files.saturating_sub(leak.sources.len());
        let more = if more > 0 {
            format!(" (+{more} more)")
        } else {
            String::new()
        };
        let how = if leak.inline {
            "inline source map (sourceMappingURL data URL)"
        } else {
            "source map"
        };
        leaks.push(format!(
            "  - {}: {how} embeds {} original file(s) in `sourcesContent`: {shown}{more}",
            leak.entry, leak.files
        ));
    }
    if !leaks.is_empty() {
        let total = leaks.len();
        let mut lines: Vec<String> = leaks.into_iter().take(LISTED).collect();
        if total > LISTED {
            lines.push(format!("  - (+{} more entries)", total - LISTED));
        }
        out.push(
            ctx.overridable(settings.severity),
            "Source Leaked In Archive",
            Some(archive),
            None,
            format!(
                "archive `{archive}` ships original source in {total} entr{}:\n{}",
                if total == 1 { "y" } else { "ies" },
                lines.join("\n")
            ),
            "build the release without `sourcesContent` (or without source maps), or justify with `allow-archive-leak: <entry> <reason>`",
        );
    }

    let mut shipped = Vec::new();
    for entry in &scan.maps_without_source {
        if let Some(ov) = ctx.find_override(GATE, tokens::ALLOW_ARCHIVE_LEAK, entry) {
            out.overrides.push(ov);
        } else {
            shipped.push(entry.clone());
        }
    }
    if !shipped.is_empty() {
        let severity = if settings.severity == Severity::Note {
            Severity::Note
        } else {
            Severity::Warning
        };
        out.push(
            severity,
            "Source Map Shipped",
            Some(archive),
            None,
            format!(
                "archive `{archive}` ships {} source map(s) without embedded source: {}",
                shipped.len(),
                listing(&shipped)
            ),
            "leave source maps out of the published package unless they are meant to ship",
        );
    }

    if !scan.oversized.is_empty() {
        out.notes.push(format!(
            "not scanned: {} entr{} larger than max_entry_bytes ({} bytes): {}",
            scan.oversized.len(),
            if scan.oversized.len() == 1 {
                "y"
            } else {
                "ies"
            },
            settings.max_entry_bytes,
            listing(&scan.oversized)
        ));
    }
    if !scan.undecodable.is_empty() {
        out.notes.push(format!(
            "not scanned: {} entr{} whose bytes this reader cannot decode: {}",
            scan.undecodable.len(),
            if scan.undecodable.len() == 1 {
                "y"
            } else {
                "ies"
            },
            listing(&scan.undecodable)
        ));
    }
    if !scan.not_maps.is_empty() {
        out.notes.push(format!(
            "`.map` entr{} that are not JSON source maps, not checked for sourcesContent: {}",
            if scan.not_maps.len() == 1 { "y" } else { "ies" },
            listing(&scan.not_maps)
        ));
    }
    if !scan.bad_inline.is_empty() {
        out.notes.push(format!(
            "inline sourceMappingURL data that is not a decodable source map: {}",
            listing(&scan.bad_inline)
        ));
    }
    let present: std::collections::HashSet<&str> = entries.iter().map(String::as_str).collect();
    let missing: Vec<String> = scan
        .external_refs
        .iter()
        .filter(|(_, target)| !present.contains(target.as_str()))
        .map(|(entry, target)| format!("{entry} -> {target}"))
        .collect();
    if !missing.is_empty() {
        out.notes.push(format!(
            "sourceMappingURL names a map that is not in the archive: {}",
            listing(&missing)
        ));
    }
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
    Ok(read_archive(archive_path, strip_components, None)?.entries)
}

/// An archive's entry names and, when asked for, what the content scan found.
pub struct ArchiveRead {
    pub entries: Vec<String>,
    pub scan: Option<ScanReport>,
}

/// A source map that embeds original source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leak {
    pub entry: String,
    /// The map is a `sourceMappingURL` data URL inside `entry`, not a `.map` file.
    pub inline: bool,
    pub files: usize,
    pub sources: Vec<String>,
}

/// What the content scan found and what it could not read. Entry names are the
/// stripped names the forbidden patterns see.
#[derive(Debug, Default)]
pub struct ScanReport {
    pub leaks: Vec<Leak>,
    pub maps_without_source: Vec<String>,
    pub oversized: Vec<String>,
    pub undecodable: Vec<String>,
    pub not_maps: Vec<String>,
    pub bad_inline: Vec<String>,
    /// (entry, the archive path its external `sourceMappingURL` resolves to).
    pub external_refs: Vec<(String, String)>,
}

/// Reads an archive's entry names (stripped) and, with `scan_limit`, scans every
/// entry of at most that many bytes for source maps.
pub fn read_archive(
    archive_path: &Path,
    strip_components: usize,
    scan_limit: Option<u64>,
) -> Result<ArchiveRead> {
    let mut raw_paths = Vec::new();
    let mut scan = scan_limit.map(|_| ScanReport::default());
    archive_formats::walk(archive_path, &mut |entry| {
        if let (Some(report), Some(limit), Some(name)) = (
            scan.as_mut(),
            scan_limit,
            strip_entry(&entry.name, strip_components),
        ) {
            if !entry.is_dir {
                match entry.data {
                    EntryData::Bytes(reader) => {
                        scan_entry(&name, entry.size, reader, limit, report)?;
                    }
                    EntryData::Undecodable(why) => {
                        report.undecodable.push(format!("{name} ({why})"));
                    }
                    EntryData::Expanded => {}
                }
            }
        }
        raw_paths.push(entry.name);
        Ok(())
    })?;
    Ok(ArchiveRead {
        entries: strip_entries(raw_paths, strip_components),
        scan,
    })
}

/// Bytes sniffed for a NUL to tell a binary entry from text.
const BINARY_SNIFF: usize = 8000;

fn scan_entry(
    name: &str,
    declared: u64,
    reader: &mut dyn Read,
    limit: u64,
    report: &mut ScanReport,
) -> Result<()> {
    if declared > limit {
        report.oversized.push(name.to_string());
        return Ok(());
    }
    let mut bytes = Vec::new();
    Read::take(&mut *reader, limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading entry `{name}`"))?;
    if bytes.len() as u64 > limit {
        report.oversized.push(name.to_string());
        return Ok(());
    }
    let binary = bytes[..bytes.len().min(BINARY_SNIFF)].contains(&0);
    if name.to_ascii_lowercase().ends_with(".map") {
        if binary {
            report.not_maps.push(format!("{name} (binary)"));
            return Ok(());
        }
        match source_maps::analyse_map(&bytes) {
            MapVerdict::EmbedsSource { files, sources } => report.leaks.push(Leak {
                entry: name.to_string(),
                inline: false,
                files,
                sources,
            }),
            MapVerdict::NoSourceContent => report.maps_without_source.push(name.to_string()),
            MapVerdict::NotASourceMap(why) => report.not_maps.push(format!("{name} ({why})")),
        }
        return Ok(());
    }
    if binary {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&bytes);
    for map_ref in source_maps::map_refs(&text) {
        match map_ref {
            MapRef::Inline(map) => match source_maps::analyse_map(&map) {
                MapVerdict::EmbedsSource { files, sources } => report.leaks.push(Leak {
                    entry: name.to_string(),
                    inline: true,
                    files,
                    sources,
                }),
                MapVerdict::NoSourceContent => {
                    report.maps_without_source.push(format!("{name} (inline)"))
                }
                MapVerdict::NotASourceMap(why) => {
                    report.bad_inline.push(format!("{name} ({why})"));
                }
            },
            MapRef::InlineUndecodable(why) => report.bad_inline.push(format!("{name} ({why})")),
            MapRef::External(url) => {
                if let Some(target) = source_maps::resolve_external(name, &url) {
                    report.external_refs.push((name.to_string(), target));
                }
            }
        }
    }
    Ok(())
}

/// Normalises raw entry names (`./` and empty components dropped), strips
/// `strip_components` leading directories, and removes duplicates in order.
fn strip_entries(raw_paths: Vec<String>, strip_components: usize) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut stripped_paths = Vec::new();
    for raw in raw_paths {
        if let Some(stripped) = strip_entry(&raw, strip_components) {
            if seen.insert(stripped.clone()) {
                stripped_paths.push(stripped);
            }
        }
    }
    stripped_paths
}

fn strip_entry(raw: &str, strip_components: usize) -> Option<String> {
    let parts: Vec<&str> = raw
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    if parts.len() <= strip_components {
        return None;
    }
    Some(parts[strip_components..].join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::fs::File;
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

        zip.start_file("pkg/example_ext.h", options).unwrap();
        zip.write_all(b"#define EXAMPLE_EXT_VERSION 2.6.0").unwrap();
        zip.finish().unwrap();

        let entries = read_archive_entries(&zip_path, 1).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], "example_ext.h");
    }

    use crate::guards::archive_formats::fixtures;
    use base64::Engine as _;

    const LEAKING_MAP: &str = r#"{"version":3,"file":"cli.js","sources":["../src/cli.ts"],"sourcesContent":["export const secret = 1;\n"],"mappings":"AAAA"}"#;
    const PLAIN_MAP: &str =
        r#"{"version":3,"file":"cli.js","sources":["../src/cli.ts"],"mappings":"AAAA"}"#;

    fn npm_tgz(dir: &tempfile::TempDir, files: &[(&str, &[u8])]) -> std::path::PathBuf {
        let path = dir.path().join("pkg-1.0.0.tgz");
        std::fs::write(&path, fixtures::gzip(&fixtures::tar(files))).unwrap();
        path
    }

    fn inline_js(map: &str) -> Vec<u8> {
        format!(
            "console.log(1);\n//# sourceMappingURL=data:application/json;charset=utf-8;base64,{}\n",
            base64::engine::general_purpose::STANDARD.encode(map)
        )
        .into_bytes()
    }

    #[test]
    fn the_scan_reports_map_and_inline_leaks_and_maps_without_source() {
        let dir = tempdir().unwrap();
        let inline = inline_js(LEAKING_MAP);
        let inline_plain = inline_js(PLAIN_MAP);
        let path = npm_tgz(
            &dir,
            &[
                ("package/package.json", b"{}"),
                (
                    "package/dist/cli.js",
                    b"x();\n//# sourceMappingURL=cli.js.map\n",
                ),
                ("package/dist/cli.js.map", LEAKING_MAP.as_bytes()),
                ("package/dist/inline.js", &inline),
                ("package/dist/plain.js.map", PLAIN_MAP.as_bytes()),
                ("package/dist/plain-inline.js", &inline_plain),
                ("package/dist/linker.map", b"Memory map\n.text 0x0\n"),
                (
                    "package/dist/logo.png",
                    b"\x89PNG\r\n\x1a\n\0\0sourceMappingURL",
                ),
            ],
        );
        let read = read_archive(&path, 1, Some(1 << 20)).unwrap();
        let scan = read.scan.unwrap();
        assert_eq!(
            scan.leaks,
            vec![
                Leak {
                    entry: "dist/cli.js.map".into(),
                    inline: false,
                    files: 1,
                    sources: vec!["../src/cli.ts".into()]
                },
                Leak {
                    entry: "dist/inline.js".into(),
                    inline: true,
                    files: 1,
                    sources: vec!["../src/cli.ts".into()]
                },
            ]
        );
        assert_eq!(
            scan.maps_without_source,
            vec!["dist/plain.js.map", "dist/plain-inline.js (inline)"]
        );
        assert_eq!(scan.not_maps.len(), 1);
        assert!(scan.not_maps[0].starts_with("dist/linker.map"));
        assert_eq!(
            scan.external_refs,
            vec![("dist/cli.js".to_string(), "dist/cli.js.map".to_string())]
        );
        assert!(scan.oversized.is_empty() && scan.bad_inline.is_empty());
    }

    #[test]
    fn the_scan_is_off_unless_asked_for() {
        let dir = tempdir().unwrap();
        let path = npm_tgz(&dir, &[("package/dist/cli.js.map", LEAKING_MAP.as_bytes())]);
        let read = read_archive(&path, 1, None).unwrap();
        assert!(read.scan.is_none());
        assert_eq!(read.entries, vec!["dist/cli.js.map"]);
    }

    #[test]
    fn an_entry_over_the_size_cap_is_named_not_scanned_and_not_passed() {
        let dir = tempdir().unwrap();
        let path = npm_tgz(&dir, &[("package/dist/cli.js.map", LEAKING_MAP.as_bytes())]);
        let small = LEAKING_MAP.len() as u64 - 1;
        let scan = read_archive(&path, 1, Some(small)).unwrap().scan.unwrap();
        assert!(scan.leaks.is_empty());
        assert_eq!(scan.oversized, vec!["dist/cli.js.map"]);
        // At exactly its size the entry is read and the leak is found.
        let exact = LEAKING_MAP.len() as u64;
        let scan = read_archive(&path, 1, Some(exact)).unwrap().scan.unwrap();
        assert_eq!(scan.leaks.len(), 1);
        assert!(scan.oversized.is_empty());
    }

    #[test]
    fn a_zip_entry_this_reader_cannot_decode_is_named_not_scanned() {
        // A zip entry whose compression this reader lacks: named in a note, not passed.
        let dir = tempdir().unwrap();
        let path = dir.path().join("pkg.zip");
        let f = File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(f);
        let stored =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("dist/cli.js.map", stored).unwrap();
        zip.write_all(LEAKING_MAP.as_bytes()).unwrap();
        zip.finish().unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        // Local and central headers both carry the method at a fixed offset; mark it
        // as bzip2 (12), which this reader does not decode.
        for sig in [&b"PK\x03\x04"[..], &b"PK\x01\x02"[..]] {
            let at = bytes.windows(4).position(|w| w == sig).unwrap();
            let offset = if sig == b"PK\x03\x04" { 8 } else { 10 };
            bytes[at + offset] = 12;
        }
        std::fs::write(&path, bytes).unwrap();
        let scan = read_archive(&path, 0, Some(1 << 20)).unwrap().scan.unwrap();
        assert!(scan.leaks.is_empty());
        assert_eq!(scan.undecodable.len(), 1, "{:?}", scan.undecodable);
        assert!(scan.undecodable[0].starts_with("dist/cli.js.map"));
    }
}
