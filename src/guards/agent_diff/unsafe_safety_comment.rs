//! `unsafe-safety-comment`: an `unsafe` block without a `// SAFETY:` comment.

use super::FileFacts;
use crate::ast::{default_registry, ParsedFileFacts};
use crate::config::GateSettings;
use crate::guards::{exempt_filter, GateOutcome};
use anyhow::Result;

pub fn evaluate_unsafe_safety_comment(
    files: &[FileFacts],
    settings: &crate::config::UnsafeSafetyCommentGate,
) -> Result<GateOutcome> {
    const GATE: &str = "unsafe-safety-comment";
    let exempt = exempt_filter(settings)?;
    let mut out = GateOutcome::new(GATE);
    // Only a pack that reads unsafe sites examines a file for this gate. A file in a
    // language with its own explicit unsafe construct (Go's `unsafe` package, C#
    // `unsafe` blocks, Swift's `Unsafe*Pointer`) that no pack reads is named, never
    // counted as examined; a language without one has nothing here to miss.
    let registry = default_registry();
    let reads_unsafe = |path: &str| {
        registry
            .find_pack(path)
            .is_some_and(|p| p.supplies(crate::ast::Fact::UnsafeSites))
    };
    let mut unread: Vec<&str> = Vec::new();
    for ff in files {
        if ff.head.is_none() || exempt.matches(&ff.file.path) {
            continue;
        }
        if reads_unsafe(&ff.file.path) {
            out.examined += 1;
        } else if matches!(
            crate::ast::extension(&ff.file.path),
            Some("go" | "cs" | "swift")
        ) {
            unread.push(&ff.file.path);
        }
    }
    if !unread.is_empty() {
        let sample: Vec<&str> = unread.iter().take(3).copied().collect();
        out.notes.push(format!(
            "{} changed file(s) are in a language with unsafe code this gate does not read (Go, C#, Swift) and were NOT analysed for it (e.g. {})",
            unread.len(),
            sample.join(", ")
        ));
    }

    for ff in files {
        let Some(head) = &ff.head else { continue };
        if exempt.matches(&ff.file.path) || !reads_unsafe(&ff.file.path) {
            continue;
        }
        let undocumented =
            |f: &ParsedFileFacts| f.unsafe_sites.iter().filter(|s| !s.documented).count();
        let base_undocumented = ff.base.as_ref().map(undocumented).unwrap_or(0);
        // A site is in scope when its line was added, or — to catch a SAFETY
        // comment deleted from above an untouched block — when the file now
        // has more undocumented sites than it had on the base side.
        let regressed = undocumented(head) > base_undocumented;
        for site in head.unsafe_sites.iter().filter(|s| !s.documented) {
            if !(ff.file.added_lines.contains(&site.line) || regressed) {
                continue;
            }
            out.push(
                settings.severity(),
                &crate::findings::SAFETY_COMMENT_MISSING,
                Some(&ff.file.path),
                Some(site.line),
                format!("Undocumented {}: `{}`", site.kind, site.snippet),
                "Add a `// SAFETY: <why the invariants hold>` comment directly above the block \
                 or the statement that contains it.",
            );
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::RustFacts;
    use crate::gitctx::{ChangeKind, ChangedFile};

    #[test]
    fn test_unsafe_safety_comment_pure() {
        use crate::ast::UnsafeSite;
        use std::collections::BTreeSet;

        let settings = crate::config::UnsafeSafetyCommentGate::default();
        let mut added_lines = BTreeSet::new();
        added_lines.insert(15);

        let facts_undocumented = [FileFacts {
            file: ChangedFile {
                path: "src/lib.rs".into(),
                old_path: "src/lib.rs".into(),
                kind: ChangeKind::Modified,
                added_lines: added_lines.clone(),
            },
            base: None,
            head: Some(RustFacts {
                tests: vec![],
                unsafe_sites: vec![UnsafeSite {
                    kind: "block",
                    line: 15,
                    documented: false,
                    snippet: "unsafe { *p }".into(),
                }],
                escape_hatches: vec![],
                has_parse_errors: false,
                ..Default::default()
            }),
            newly_added_nul: false,
        }];
        let out_bad = evaluate_unsafe_safety_comment(&facts_undocumented, &settings).unwrap();
        assert_eq!(out_bad.violations.len(), 1);

        let facts_documented = [FileFacts {
            file: ChangedFile {
                path: "src/lib.rs".into(),
                old_path: "src/lib.rs".into(),
                kind: ChangeKind::Modified,
                added_lines,
            },
            base: None,
            head: Some(RustFacts {
                tests: vec![],
                unsafe_sites: vec![UnsafeSite {
                    kind: "block",
                    line: 15,
                    documented: true,
                    snippet: "unsafe { *p }".into(),
                }],
                escape_hatches: vec![],
                has_parse_errors: false,
                ..Default::default()
            }),
            newly_added_nul: false,
        }];
        let out_good = evaluate_unsafe_safety_comment(&facts_documented, &settings).unwrap();
        assert_eq!(out_good.violations.len(), 0);
    }
}
