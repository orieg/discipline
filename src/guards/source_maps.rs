//! Source-map analysis behind `archive-contents`' content scan (`scan_contents`).
//!
//! A source map is parsed as JSON; it leaks when its `sourcesContent` (or that of a
//! section of an index map) holds the text of an original file. Inline maps are the
//! `data:` URL of a `sourceMappingURL` comment, base64- or percent-decoded and then
//! parsed the same way. Only paths from `sources` are ever reported, never content,
//! and home-directory user names in them are replaced by `~`.

use base64::Engine as _;
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

/// How many `sources` paths a finding names.
pub const SOURCES_SHOWN: usize = 5;

/// What a source map carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapVerdict {
    /// `sourcesContent` embeds `files` original files; `sources` names the first
    /// [`SOURCES_SHOWN`] of them.
    EmbedsSource { files: usize, sources: Vec<String> },
    /// A source map without embedded source.
    NoSourceContent,
    /// Not a source map, and why.
    NotASourceMap(String),
}

/// Parses `bytes` as a source map (revision 3, plain or index map).
pub fn analyse_map(bytes: &[u8]) -> MapVerdict {
    let mut body = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    // The format allows a first line starting `)]}'` (an XSSI guard) to be skipped.
    if body.starts_with(b")]}'") {
        body = match body.iter().position(|&b| b == b'\n') {
            Some(i) => &body[i + 1..],
            None => &[],
        };
    }
    let value: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return MapVerdict::NotASourceMap(format!("not JSON ({e})")),
    };
    let Some(object) = value.as_object() else {
        return MapVerdict::NotASourceMap("the JSON is not an object".to_string());
    };
    if !["mappings", "sources", "sections"]
        .iter()
        .any(|k| object.contains_key(*k))
    {
        return MapVerdict::NotASourceMap("no `mappings`, `sources` or `sections` key".to_string());
    }
    let mut files = 0;
    let mut sources = Vec::new();
    collect_embedded(&value, &mut files, &mut sources, 0);
    if files > 0 {
        MapVerdict::EmbedsSource { files, sources }
    } else {
        MapVerdict::NoSourceContent
    }
}

fn collect_embedded(map: &Value, files: &mut usize, sources: &mut Vec<String>, depth: usize) {
    if depth > 16 {
        return;
    }
    if let Some(contents) = map.get("sourcesContent").and_then(Value::as_array) {
        let names = map.get("sources").and_then(Value::as_array);
        for (i, content) in contents.iter().enumerate() {
            if content.as_str().is_some_and(|s| !s.is_empty()) {
                *files += 1;
                if sources.len() < SOURCES_SHOWN {
                    let name = names
                        .and_then(|n| n.get(i))
                        .and_then(Value::as_str)
                        .unwrap_or("<unnamed>");
                    sources.push(display_source(name));
                }
            }
        }
    }
    if let Some(sections) = map.get("sections").and_then(Value::as_array) {
        for section in sections {
            if let Some(inner) = section.get("map") {
                collect_embedded(inner, files, sources, depth + 1);
            }
        }
    }
}

static HOME_DIR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:/Users/|/home/|[a-z]:[\\/]+Users[\\/]+)[^/\\]+").expect("static regex")
});

/// A `sources` path as reported: home-directory user names replaced by `~`, and
/// cut to 160 characters.
pub fn display_source(path: &str) -> String {
    let redacted = HOME_DIR.replace_all(path, "~");
    let mut out: String = redacted.chars().take(160).collect();
    if redacted.chars().count() > 160 {
        out.push('…');
    }
    out
}

/// A `sourceMappingURL` found in a text entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapRef {
    /// A `data:` URL, decoded to the map's bytes.
    Inline(Vec<u8>),
    /// A `data:` URL that could not be decoded, and why.
    InlineUndecodable(String),
    /// A reference to a separate file (the URL as written).
    External(String),
}

/// `//# sourceMappingURL=<url>` (also `//@`, and the `/*# ... */` form CSS uses).
static MAP_COMMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)(?://|/\*)[#@][ \t]*sourceMappingURL[ \t]*=[ \t]*([^\s'"`*]+)"#)
        .expect("static regex")
});

/// Every `sourceMappingURL` comment in `text`.
pub fn map_refs(text: &str) -> Vec<MapRef> {
    MAP_COMMENT
        .captures_iter(text)
        .map(|c| {
            let url = &c[1];
            if url.len() >= 5 && url[..5].eq_ignore_ascii_case("data:") {
                match decode_data_url(&url[5..]) {
                    Ok(bytes) => MapRef::Inline(bytes),
                    Err(why) => MapRef::InlineUndecodable(why),
                }
            } else {
                MapRef::External(url.to_string())
            }
        })
        .collect()
}

/// Decodes the part of a `data:` URL after `data:` (RFC 2397).
fn decode_data_url(rest: &str) -> Result<Vec<u8>, String> {
    let (meta, payload) = rest
        .split_once(',')
        .ok_or_else(|| "a data URL without a comma".to_string())?;
    if meta.split(';').any(|p| p.eq_ignore_ascii_case("base64")) {
        let payload = percent_decode(payload);
        let engines = [
            &base64::engine::general_purpose::STANDARD,
            &base64::engine::general_purpose::STANDARD_NO_PAD,
            &base64::engine::general_purpose::URL_SAFE,
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        ];
        engines
            .iter()
            .find_map(|e| e.decode(&payload).ok())
            .ok_or_else(|| "invalid base64 in a data URL".to_string())
    } else {
        Ok(percent_decode(payload))
    }
}

fn percent_decode(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// Resolves an external `sourceMappingURL` against the directory of `entry`.
/// `None` for an absolute URL or path, which cannot be checked against the archive.
pub fn resolve_external(entry: &str, url: &str) -> Option<String> {
    let url = url.split(['?', '#']).next().unwrap_or(url);
    if url.is_empty() || url.contains("://") || url.starts_with('/') || url.starts_with("//") {
        return None;
    }
    let mut parts: Vec<&str> = entry.split('/').collect();
    parts.pop();
    for segment in url.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEAK: &str = r#"{"version":3,"file":"cli.js","sources":["../src/cli.ts","../src/util.ts"],"sourcesContent":["export const x = 1;\n","export function f() {}\n"],"mappings":"AAAA"}"#;

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(s)
    }

    #[test]
    fn a_map_with_sources_content_embeds_source_and_names_the_sources() {
        assert_eq!(
            analyse_map(LEAK.as_bytes()),
            MapVerdict::EmbedsSource {
                files: 2,
                sources: vec!["../src/cli.ts".into(), "../src/util.ts".into()]
            }
        );
    }

    #[test]
    fn a_map_without_sources_content_or_with_only_nulls_embeds_nothing() {
        for map in [
            r#"{"version":3,"sources":["a.ts"],"mappings":"AAAA"}"#,
            r#"{"version":3,"sources":["a.ts"],"sourcesContent":[null],"mappings":"AAAA"}"#,
            r#"{"version":3,"sources":["a.ts"],"sourcesContent":[""],"mappings":"AAAA"}"#,
            r#"{"version":3,"sources":[],"sourcesContent":[],"mappings":""}"#,
        ] {
            assert_eq!(
                analyse_map(map.as_bytes()),
                MapVerdict::NoSourceContent,
                "{map}"
            );
        }
    }

    #[test]
    fn index_maps_bom_and_xssi_prefix_are_read() {
        let index = format!(
            r#"{{"version":3,"sections":[{{"offset":{{"line":0,"column":0}},"map":{LEAK}}}]}}"#
        );
        assert!(matches!(
            analyse_map(index.as_bytes()),
            MapVerdict::EmbedsSource { files: 2, .. }
        ));
        let guarded = format!(")]}}'\n{LEAK}");
        assert!(matches!(
            analyse_map(guarded.as_bytes()),
            MapVerdict::EmbedsSource { files: 2, .. }
        ));
        let bom = format!("\u{feff}{LEAK}");
        assert!(matches!(
            analyse_map(bom.as_bytes()),
            MapVerdict::EmbedsSource { .. }
        ));
    }

    #[test]
    fn json_that_is_not_a_source_map_and_text_are_not_maps() {
        for body in [
            "Memory map of the linker output\n.text 0x0000",
            r#"{"name":"not a map"}"#,
            "[1,2,3]",
        ] {
            assert!(
                matches!(analyse_map(body.as_bytes()), MapVerdict::NotASourceMap(_)),
                "{body}"
            );
        }
    }

    #[test]
    fn only_the_first_five_sources_are_named_and_home_directories_are_redacted() {
        let names: Vec<String> = (0..8)
            .map(|i| format!("\"/home/alice/app/src/{i}.ts\"")) // discipline:allow(pii) fixture for the redaction
            .collect();
        let contents: Vec<&str> = (0..8).map(|_| "\"x\"").collect();
        let map = format!(
            r#"{{"version":3,"sources":[{}],"sourcesContent":[{}],"mappings":""}}"#,
            names.join(","),
            contents.join(",")
        );
        let MapVerdict::EmbedsSource { files, sources } = analyse_map(map.as_bytes()) else {
            panic!("not a leak");
        };
        assert_eq!(files, 8);
        assert_eq!(sources.len(), SOURCES_SHOWN);
        assert_eq!(sources[0], "~/app/src/0.ts");
        assert_eq!(display_source(r"C:\Users\bob\src\a.ts"), r"~\src\a.ts"); // discipline:allow(pii)
        assert_eq!(display_source("/Users/carol/x.ts"), "~/x.ts"); // discipline:allow(pii)
    }

    #[test]
    fn inline_maps_are_decoded_from_base64_and_percent_data_urls() {
        let js = format!(
            "console.log(1);\n//# sourceMappingURL=data:application/json;charset=utf-8;base64,{}\n",
            b64(LEAK)
        );
        let refs = map_refs(&js);
        assert_eq!(refs, vec![MapRef::Inline(LEAK.as_bytes().to_vec())]);

        let css = format!(
            "a{{color:red}}\n/*# sourceMappingURL=data:application/json;base64,{} */\n",
            b64(LEAK)
        );
        assert_eq!(
            map_refs(&css),
            vec![MapRef::Inline(LEAK.as_bytes().to_vec())]
        );

        let legacy = "x\n//@ sourceMappingURL=data:application/json,%7B%22version%22%3A3%7D\n";
        assert_eq!(
            map_refs(legacy),
            vec![MapRef::Inline(br#"{"version":3}"#.to_vec())]
        );
    }

    #[test]
    fn external_and_undecodable_references_are_told_apart() {
        let refs = map_refs(
            "a();\n//# sourceMappingURL=cli.js.map\nb();\n//# sourceMappingURL=data:application/json;base64,!!!\n",
        );
        assert_eq!(refs[0], MapRef::External("cli.js.map".into()));
        assert!(matches!(refs[1], MapRef::InlineUndecodable(_)));
        // Mentions outside a comment are not references.
        assert!(map_refs("const s = 'sourceMappingURL=data:x';").is_empty());
    }

    #[test]
    fn external_references_resolve_against_the_entry_directory() {
        assert_eq!(
            resolve_external("package/dist/cli.js", "cli.js.map").as_deref(),
            Some("package/dist/cli.js.map")
        );
        assert_eq!(
            resolve_external("package/dist/cli.js", "../maps/cli.js.map?v=1").as_deref(),
            Some("package/maps/cli.js.map")
        );
        assert_eq!(
            resolve_external("cli.js", "https://cdn.example/cli.js.map"),
            None
        );
        assert_eq!(resolve_external("cli.js", "/abs/cli.js.map"), None);
    }
}
