//! What `hook install --upgrade` needs to tell a file a release generated from one a person
//! edited: the digest line a generated script, workflow or plugin carries, the merge of
//! this release's entries into a JSON hook file that has content of its own, and the
//! difference shown before anything is overwritten.

use crate::report::gitlab::sha256_hex;
use crate::report::text::terminal_line;

/// What the digest line of a generated file starts with, after its comment leader.
pub const STAMP: &str = "discipline-hook-file:";

/// What a file with the generated header says about its own content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stamp {
    /// Its digest line matches the rest of the file: exactly what a release wrote.
    Unedited,
    /// It has a digest line (or several) that does not match: changed since it was written.
    Edited,
    /// It has none: written before releases recorded a digest, so an edit cannot be told
    /// from that release's output.
    Absent,
}

/// The one digest line of a generated file: its comment leader, the options it was written
/// with and the digest. It names no release, so a file whose template did not change
/// between two releases is the same file in both.
static STAMP_LINE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^(?:#|//) discipline-hook-file: (?:[a-z]+=[a-z]+ )*sha256=([0-9a-f]{64})$")
        .unwrap()
});

/// What the digest is taken over: `text` with line endings as `\n` (a checkout can rewrite
/// them) and the digest itself left out of its line, so the options on that line are
/// covered too.
fn digest_of(text: &str, hex: &str) -> String {
    let unix = text.replace("\r\n", "\n");
    sha256_hex(
        unix.replacen(&format!("sha256={hex}"), "sha256=", 1)
            .as_bytes(),
    )
}

/// `content`, a generated file whose first comment line carrying `header` says what wrote
/// it, with the digest line after that line. `options` (`mode=observe`, `sums=pinned`)
/// records what the file was written with. The digest detects a later edit; it is not a
/// signature, since whoever edits the file can also recompute it.
pub fn stamp(content: &str, header: &str, options: &[&str]) -> String {
    let Some(at) = content.lines().position(|l| l.contains(header)) else {
        return content.to_string();
    };
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let leader = if lines[at].starts_with("//") {
        "//"
    } else {
        "#"
    };
    let opts: String = options.iter().map(|o| format!("{o} ")).collect();
    let line = format!("{leader} {STAMP} {opts}sha256=");
    let build = |hex: &str| -> String {
        let mut out = String::with_capacity(content.len() + line.len() + 66);
        for (i, l) in lines.iter().enumerate() {
            out.push_str(l);
            if i == at {
                out.push_str(&line);
                out.push_str(hex);
                out.push('\n');
            }
        }
        out
    };
    build(&digest_of(&build(""), ""))
}

/// Whether `text` is exactly what a release wrote, by its digest line.
pub fn stamp_state(text: &str) -> Stamp {
    let stamped: Vec<&str> = text
        .lines()
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .filter(|l| l.contains(STAMP))
        .collect();
    match stamped.as_slice() {
        [] => Stamp::Absent,
        [line] => match STAMP_LINE.captures(line) {
            Some(c) if digest_of(text, &c[1]) == c[1] => Stamp::Unedited,
            _ => Stamp::Edited,
        },
        _ => Stamp::Edited,
    }
}

/// `text` without its digest line (every line carrying [`STAMP`]).
pub fn unstamped(text: &str) -> String {
    text.split_inclusive('\n')
        .filter(|l| !l.contains(STAMP))
        .collect()
}

/// Whether `existing`, a file with no digest line, is `content` (what this release writes)
/// apart from that line: then it is provably what a release wrote, since rewriting it
/// only adds the digest.
pub fn lacks_only_the_digest(existing: &str, content: &str) -> bool {
    stamp_state(existing) == Stamp::Absent && existing.replace("\r\n", "\n") == unstamped(content)
}

/// The most lines of difference [`unified_diff`] prints.
pub const DIFF_LINES: usize = 120;

/// The most characters of one line [`unified_diff`] prints.
const DIFF_WIDTH: usize = 1000;

/// The difference between `old` and `new` as a unified diff with three lines of context,
/// `name` in its two header lines. The files are repository content: every line goes
/// through [`terminal_line`] and is cut at [`DIFF_WIDTH`] characters, and at most `limit`
/// lines of hunks are printed, with a count of the rest.
pub fn unified_diff(old: &str, new: &str, name: &str, limit: usize) -> String {
    let (a, b): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    let name = terminal_line(name);
    let mut out = format!("--- {name} (as it is)\n+++ {name} (as this release writes it)\n");
    // Longest common subsequence by lines; hook files are a few hundred lines at most.
    if a.len().saturating_mul(b.len()) > 4_000_000 {
        out.push_str("(the files are too long to compare line by line)\n");
        return out;
    }
    let mut lcs = vec![vec![0u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    // Every line of either file in order: ' ' in both, '-' only in `old`, '+' only in `new`.
    let mut ops: Vec<(char, &str)> = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            ops.push((' ', a[i]));
            i += 1;
            j += 1;
        } else if j == b.len() || (i < a.len() && lcs[i + 1][j] >= lcs[i][j + 1]) {
            ops.push(('-', a[i]));
            i += 1;
        } else {
            ops.push(('+', b[j]));
            j += 1;
        }
    }
    if ops.iter().all(|(op, _)| *op == ' ') {
        out.push_str(if old == new {
            "(no difference)\n"
        } else {
            "(only line endings differ)\n"
        });
        return out;
    }
    const CONTEXT: usize = 3;
    let changed: Vec<usize> = (0..ops.len()).filter(|k| ops[*k].0 != ' ').collect();
    let near = |k: usize| {
        changed
            .iter()
            .any(|c| k + CONTEXT >= *c && k <= *c + CONTEXT)
    };
    let (mut old_no, mut new_no) = (1usize, 1usize);
    let mut body: Vec<String> = Vec::new();
    let mut k = 0;
    while k < ops.len() {
        if !near(k) {
            if ops[k].0 != '+' {
                old_no += 1;
            }
            if ops[k].0 != '-' {
                new_no += 1;
            }
            k += 1;
            continue;
        }
        let end = (k..ops.len()).find(|e| !near(*e)).unwrap_or(ops.len());
        let hunk = &ops[k..end];
        let olds = hunk.iter().filter(|(op, _)| *op != '+').count();
        let news = hunk.iter().filter(|(op, _)| *op != '-').count();
        body.push(format!("@@ -{old_no},{olds} +{new_no},{news} @@"));
        for (op, line) in hunk {
            let mut shown: String = line.chars().take(DIFF_WIDTH).collect();
            if line.chars().count() > DIFF_WIDTH {
                shown.push_str(" [...]");
            }
            body.push(format!("{op}{}", terminal_line(&shown)));
        }
        old_no += olds;
        new_no += news;
        k = end;
    }
    for line in body.iter().take(limit) {
        out.push_str(line);
        out.push('\n');
    }
    if body.len() > limit {
        out.push_str(&format!(
            "({} more lines of difference not shown)\n",
            body.len() - limit
        ));
    }
    out
}

/// `generated`, this release's JSON hook file, merged into `existing`, a JSON hook file
/// with content of its own: every handler `ours` recognises by its command is taken out
/// (a group left without handlers goes with it), this release's entries for an event go
/// where the first of them was (at the end when there was none), and everything else stays:
/// other handlers, other events, other top-level keys. A top-level value this release
/// writes (`version`) is added only when the file has none.
///
/// `None` when `existing` is not strict JSON (comments, trailing commas) or has another
/// shape where the events are: nothing can be merged into it by rule.
pub fn merge_json(
    existing: &str,
    generated: &str,
    ours: &dyn Fn(&str) -> bool,
) -> Option<serde_json::Value> {
    use serde_json::Value;
    let mut merged: Value = serde_json::from_str(existing).ok()?;
    let generated: Value = serde_json::from_str(generated).ok()?;
    let is_ours = |handler: &Value| {
        handler
            .get("command")
            .or_else(|| handler.get("bash"))
            .and_then(Value::as_str)
            .is_some_and(ours)
    };
    let top = merged.as_object_mut()?;
    for (key, value) in generated.as_object()? {
        let Value::Object(events) = value else {
            if !top.contains_key(key) {
                top.insert(key.clone(), value.clone());
            }
            continue;
        };
        let table = top
            .entry(key.clone())
            .or_insert_with(|| Value::Object(serde_json::Map::new()))
            .as_object_mut()?;
        // Where this release's entries go, for each event that had one of ours.
        let mut places: std::collections::BTreeMap<String, usize> = Default::default();
        for (event, list) in table.iter_mut() {
            let mut kept = Vec::new();
            for mut item in std::mem::take(list.as_array_mut()?) {
                let mut removed = is_ours(&item);
                let mut emptied = false;
                if let Some(handlers) = item.get_mut("hooks").and_then(Value::as_array_mut) {
                    let before = handlers.len();
                    handlers.retain(|h| !is_ours(h));
                    removed |= handlers.len() != before;
                    emptied = handlers.is_empty() && before > 0;
                }
                if removed {
                    places.entry(event.clone()).or_insert(kept.len());
                }
                if !(is_ours(&item) || emptied) {
                    kept.push(item);
                }
            }
            *list = Value::Array(kept);
        }
        for (event, entries) in events {
            let list = table
                .entry(event.clone())
                .or_insert_with(|| Value::Array(Vec::new()))
                .as_array_mut()?;
            let at = places.get(event).copied().unwrap_or(list.len());
            for (n, entry) in entries.as_array()?.iter().enumerate() {
                list.insert(at + n, entry.clone());
            }
        }
        // An event that held only entries of ours and that this release no longer writes.
        table.retain(|_, list| list.as_array().is_none_or(|l| !l.is_empty()));
    }
    Some(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "Written by `tool`";

    #[test]
    fn a_stamped_file_is_unedited_until_any_line_changes() {
        let body = "#!/bin/bash\n# Written by `tool`.\nset -u\necho hi\n";
        let stamped = stamp(body, HEADER, &["mode=observe"]);
        assert_eq!(stamped.lines().count(), 5, "{stamped}");
        assert!(
            stamped
                .lines()
                .nth(2)
                .unwrap()
                .starts_with(&format!("# {STAMP} mode=observe sha256=")),
            "{stamped}"
        );
        assert_eq!(stamp_state(&stamped), Stamp::Unedited);
        // Line endings a checkout rewrote are not an edit.
        assert_eq!(stamp_state(&stamped.replace('\n', "\r\n")), Stamp::Unedited);
        // Each of: the body, an added comment, the recorded option.
        for edited in [
            stamped.replace("echo hi", "echo ho"),
            format!("{stamped}# kept on purpose\n"),
            stamped.replace("mode=observe", "mode=enforcing"),
        ] {
            assert_ne!(edited, stamped);
            assert_eq!(stamp_state(&edited), Stamp::Edited, "{edited}");
        }
        // No digest line: cannot tell. Two of them: edited.
        assert_eq!(stamp_state(body), Stamp::Absent);
        let line = stamped.lines().nth(2).unwrap();
        assert_eq!(stamp_state(&format!("{stamped}{line}\n")), Stamp::Edited);
        // Without its digest line it is the body again, and only that is "the same file
        // before digests".
        assert_eq!(unstamped(&stamped), body);
        assert!(lacks_only_the_digest(body, &stamped));
        assert!(lacks_only_the_digest(&body.replace('\n', "\r\n"), &stamped));
        assert!(!lacks_only_the_digest(
            &body.replace("echo hi", "echo ho"),
            &stamped
        ));
        assert!(!lacks_only_the_digest(&stamped, &stamped));
        // A `//` header gets a `//` digest line.
        let js = stamp("// Written by `tool`.\nexport {}\n", HEADER, &[]);
        assert!(js.lines().nth(1).unwrap().starts_with("// "), "{js}");
        assert_eq!(stamp_state(&js), Stamp::Unedited);
    }

    #[test]
    fn the_diff_shows_changed_lines_with_context_and_is_bounded() {
        let old: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        let new = old
            .replace("line 10\n", "line ten\n")
            .replace("line 20\n", "");
        let diff = unified_diff(&old, &new, "f.sh", DIFF_LINES);
        assert!(
            diff.starts_with("--- f.sh (as it is)\n+++ f.sh (as this release writes it)\n"),
            "{diff}"
        );
        assert!(diff.contains("@@ -7,7 +7,7 @@\n line 7\n"), "{diff}");
        assert!(diff.contains("\n-line 10\n+line ten\n line 11\n"), "{diff}");
        assert!(
            diff.ends_with(" line 13\n@@ -17,4 +17,3 @@\n line 17\n line 18\n line 19\n-line 20\n"),
            "{diff}"
        );
        assert!(
            !diff.contains("line 6") && !diff.contains("line 15"),
            "{diff}"
        );
        // Bounded, with a count of what is left out.
        let short = unified_diff(&old, &new, "f.sh", 3);
        assert!(
            short.ends_with(" line 8\n(11 more lines of difference not shown)\n"),
            "{short}"
        );
        // A control character in a line or in the name does not reach the terminal.
        let esc = unified_diff("a\n", "\u{1b}[2Jb\n", "f\u{1b}.sh", DIFF_LINES);
        assert!(!esc.contains('\u{1b}'), "{esc:?}");
        assert!(unified_diff("a\n", "a\n", "f", 9).contains("(no difference)"));
        assert!(unified_diff("a\r\n", "a\n", "f", 9).contains("only line endings"));
    }

    #[test]
    fn the_merge_replaces_only_our_handlers() {
        let ours = |c: &str| c.contains("tool run");
        let generated = r#"{"version":1,"hooks":{"Stop":[{"hooks":[{"command":"tool run"}]}],"Pre":[{"matcher":"E","hooks":[{"command":"tool run --pre"}]}]}}"#;
        let existing = r#"{"model":"m","version":2,"hooks":{
            "Stop":[{"hooks":[{"command":"mine"}]},{"hooks":[{"command":"x || tool run"}]},{"hooks":[{"command":"last"}]}],
            "Old":[{"command":"tool run --old"}],
            "Mixed":[{"matcher":"E","hooks":[{"command":"tool run"},{"command":"theirs"}]}]}}"#;
        let merged = merge_json(existing, generated, &ours).unwrap();
        assert_eq!(merged["model"], "m");
        assert_eq!(merged["version"], 2, "a value the file has is kept");
        let stop = merged["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 3, "{stop:?}");
        assert_eq!(stop[0]["hooks"][0]["command"], "mine");
        assert_eq!(stop[1]["hooks"][0]["command"], "tool run");
        assert_eq!(stop[2]["hooks"][0]["command"], "last");
        assert_eq!(merged["hooks"]["Pre"][0]["matcher"], "E");
        assert!(merged["hooks"].get("Old").is_none(), "{merged}");
        assert_eq!(
            merged["hooks"]["Mixed"],
            serde_json::json!([{"matcher":"E","hooks":[{"command":"theirs"}]}])
        );
        // Merging again changes nothing.
        let again = merge_json(&merged.to_string(), generated, &ours).unwrap();
        assert_eq!(again, merged);
        // Not strict JSON, or events that are not lists: nothing to merge by rule.
        assert!(merge_json("{ // c\n}", generated, &ours).is_none());
        assert!(merge_json(r#"{"hooks":{"Stop":{}}}"#, generated, &ours).is_none());
        assert!(merge_json(r#"{"hooks":[]}"#, generated, &ours).is_none());
    }
}
