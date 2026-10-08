//! `golden-output`: committed expected-output and snapshot files changed without the code
//! or the test that produces them.

use crate::config::GateSettings;
use crate::gitctx::ChangeKind;
use crate::guards::{Context, GateOutcome, PathFilter};
use crate::tokens;
use anyhow::Result;

/// The test file a snapshot belongs to, and the test names it records.
#[derive(Debug, PartialEq, Eq)]
pub struct SnapshotOwner {
    pub test_file: String,
    pub tests: Vec<String>,
}

/// Map a snapshot file to its test file and the tests it records: Jest
/// `__snapshots__/<file>.snap` (keys ``exports[`<title> 1`]``), insta
/// `snapshots/<crate>__<module>__<test>.snap` (`<module>.rs` beside the directory),
/// syrupy / pytest-snapshot `__snapshots__/<test_file>.ambr` (`# name: <test>` lines).
pub fn snapshot_owner(path: &str, content: &str) -> Option<SnapshotOwner> {
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    let (parent, snap_dir) = dir.rsplit_once('/').unwrap_or(("", dir));
    let join = |p: &str, f: &str| {
        if p.is_empty() {
            f.to_string()
        } else {
            format!("{p}/{f}")
        }
    };
    if snap_dir == "__snapshots__" {
        if let Some(stem) = name.strip_suffix(".snap") {
            // Jest: `foo.test.ts.snap` -> `foo.test.ts`; the keys carry the titles.
            let tests: Vec<String> = content
                .lines()
                .filter_map(|l| l.strip_prefix("exports[`"))
                // `<describe> <title> 1`: the trailing counter goes, the title stays whole.
                .filter_map(|l| l.split_once("`]").map(|(t, _)| t))
                .map(|t| t.rsplit_once(' ').map_or(t, |(t, _)| t).to_string())
                .collect();
            return Some(SnapshotOwner {
                test_file: join(parent, stem),
                tests,
            });
        }
        if let Some(stem) = name.strip_suffix(".ambr") {
            let tests: Vec<String> = content
                .lines()
                .filter_map(|l| l.strip_prefix("# name: "))
                .map(|t| t.split('[').next().unwrap_or(t).trim().to_string())
                .collect();
            return Some(SnapshotOwner {
                test_file: join(parent, &format!("{stem}.py")),
                tests,
            });
        }
        return None;
    }
    if snap_dir == "snapshots" {
        let stem = name.strip_suffix(".snap")?;
        // insta: `crate__module__test.snap` (a `-N` suffix numbers inline variants).
        let mut parts: Vec<&str> = stem.split("__").collect();
        let test = parts.pop()?;
        let test = test.rsplit_once('-').map_or(test, |(t, n)| {
            if n.chars().all(|c| c.is_ascii_digit()) {
                t
            } else {
                test
            }
        });
        let module = parts.last().copied().unwrap_or("lib");
        let test_file = if std::path::Path::new(&join(parent, &format!("{module}/mod.rs"))).exists()
        {
            join(parent, &format!("{module}/mod.rs"))
        } else {
            join(parent, &format!("{module}.rs"))
        };
        return Some(SnapshotOwner {
            test_file,
            tests: vec![test.to_string()],
        });
    }
    None
}

pub fn golden_output(ctx: &Context) -> Result<GateOutcome> {
    const GATE: &str = "golden-output";
    let settings = &ctx.config.gates.golden_output;
    let mut out = GateOutcome::new(GATE);

    let path_filter = PathFilter::new(&settings.paths)?;
    let exempt_filter = PathFilter::new(&settings.exempt_paths)?;

    let changed = ctx.git.changed_files()?;
    let is_golden = |f: &crate::gitctx::ChangedFile| {
        path_filter.matches(&f.path) || (!f.old_path.is_empty() && path_filter.matches(&f.old_path))
    };
    // Expected output rewritten while nothing that produces it changed: the shape of a
    // failing comparison "fixed" by regenerating the expectation. Prose and the gate's own
    // configuration do not produce output.
    let produces_output = |f: &crate::gitctx::ChangedFile| {
        let p = f.path.to_ascii_lowercase();
        !is_golden(f)
            && !p.ends_with(".md")
            && !p.ends_with(".markdown")
            && !p.ends_with(".txt")
            && !p.ends_with(".rst")
            && p != "discipline.toml"
    };
    let snapshot_only = !changed.iter().any(produces_output);
    for file in &changed {
        if file.kind == ChangeKind::Added {
            // An added snapshot is fine for a new test; for a test that already existed it
            // is an expectation written after the fact.
            let matches_target = path_filter.matches(&file.path);
            if !matches_target || exempt_filter.matches(&file.path) {
                continue;
            }
            let Some(head) = ctx.git.head_content(&file.path)? else {
                continue;
            };
            let Some(owner) = snapshot_owner(&file.path, &head) else {
                continue;
            };
            // The owning test file must already exist on the base side.
            if ctx.git.base_content(&owner.test_file)?.is_none() {
                continue;
            }
            out.examined += 1;
            let owner_file = changed.iter().find(|f| f.path == owner.test_file);
            let added_in_owner: Vec<String> = match owner_file {
                Some(f) => ctx
                    .git
                    .head_content(&f.path)?
                    .map(|content| {
                        content
                            .lines()
                            .enumerate()
                            .filter(|(i, _)| f.added_lines.contains(&(i + 1)))
                            .map(|(_, l)| l.to_string())
                            .collect()
                    })
                    .unwrap_or_default(),
                None => Vec::new(),
            };
            let unmatched: Vec<&String> = owner
                .tests
                .iter()
                .filter(|t| !added_in_owner.iter().any(|l| l.contains(t.as_str())))
                .collect();
            if unmatched.is_empty() {
                continue;
            }
            out.lift_or_push(
                ctx.find_override(
                    GATE,
                    &crate::findings::SNAPSHOT_ADDED_FOR_EXISTING_TEST,
                    tokens::ALLOW_GOLDEN_UPDATE,
                    &file.path,
                ),
                ctx.overridable(settings.severity()),
                &crate::findings::SNAPSHOT_ADDED_FOR_EXISTING_TEST,
                (Some(&file.path), None),
                format!(
                    "Snapshot `{}` is new, but the test(s) it records ({}) already existed in `{}` and this change does not add them: the expectation was written after the behaviour.",
                    file.path,
                    unmatched.iter().map(|t| format!("`{t}`")).collect::<Vec<_>>().join(", "),
                    owner.test_file
                ),
                "Confirm the recorded output is the intended one, then record it: `allow-golden-update: <path> <reason>`.",
            );
            continue;
        }

        let matches_target = path_filter.matches(&file.path)
            || (!file.old_path.is_empty() && path_filter.matches(&file.old_path));
        if !matches_target {
            continue;
        }

        let is_exempt = exempt_filter.matches(&file.path)
            || (!file.old_path.is_empty() && exempt_filter.matches(&file.old_path));
        if is_exempt {
            continue;
        }

        out.examined += 1;

        // The finding reported below without a directive.
        let golden = if snapshot_only {
            &crate::findings::GOLDEN_REGENERATED_WITHOUT_SOURCE_CHANGE
        } else {
            &crate::findings::GOLDEN_CHANGED_WITHOUT_DIRECTIVE
        };
        if let Some(ov) = ctx.find_override(GATE, golden, tokens::ALLOW_GOLDEN_UPDATE, &file.path) {
            out.overrides.push(ov);
            continue;
        }
        if !file.old_path.is_empty() && file.old_path != file.path {
            if let Some(ov) =
                ctx.find_override(GATE, golden, tokens::ALLOW_GOLDEN_UPDATE, &file.old_path)
            {
                out.overrides.push(ov);
                continue;
            }
        }

        let action = match file.kind {
            ChangeKind::Deleted => "deleted",
            _ => "modified",
        };

        let (title, context) = if snapshot_only {
            (
                &crate::findings::GOLDEN_REGENERATED_WITHOUT_SOURCE_CHANGE,
                " No file that produces output changed in this diff, so the expectation was rewritten to match existing behaviour.",
            )
        } else {
            (&crate::findings::GOLDEN_CHANGED_WITHOUT_DIRECTIVE, "")
        };
        out.push(
            ctx.overridable(settings.severity()),
            title,
            Some(&file.path),
            None,
            format!(
                "Committed golden/snapshot file `{}` was {action} ({} line(s) rewritten) without an explicit override.{context}",
                file.path,
                file.added_lines.len()
            ),
            "Provide a scoped override on its own line in the PR body or a commit message: \
             `allow-golden-update: <path-or-prefix> <reason>` (or `discipline:allow(golden-output): ...`).",
        );
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_files_map_to_their_test_file_and_tests() {
        let jest = snapshot_owner(
            "web/src/__snapshots__/app.test.tsx.snap",
            "exports[`renders the header 1`] = `<h1/>`;\n\nexports[`renders the footer 1`] = `<p/>`;\n",
        )
        .unwrap();
        assert_eq!(jest.test_file, "web/src/app.test.tsx");
        assert_eq!(jest.tests, vec!["renders the header", "renders the footer"]);
        let insta = snapshot_owner(
            "crates/x/src/snapshots/x__parser__parses_empty-2.snap",
            "---\n",
        )
        .unwrap();
        assert_eq!(insta.test_file, "crates/x/src/parser.rs");
        assert_eq!(insta.tests, vec!["parses_empty"]);
        let ambr = snapshot_owner(
            "tests/__snapshots__/test_api.ambr",
            "# name: test_get\n  'x'\n# ---\n# name: test_put[1]\n  'y'\n",
        )
        .unwrap();
        assert_eq!(ambr.test_file, "tests/test_api.py");
        assert_eq!(ambr.tests, vec!["test_get", "test_put"]);
        assert!(snapshot_owner("tests/golden/out.txt", "").is_none());
    }
}
