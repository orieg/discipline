//! `banned_actions`: references no directive lifts.

use super::{action_pin_refs, docker_image, is_action_metadata_path, workflow_pin_refs, PinKind};
use crate::guards::{Context, GateOutcome};
use anyhow::Result;

/// The `banned_actions` entry a `uses:` value falls under, if any. An entry without a ref
/// bans every ref; `owner/repo` also covers the paths under it (`owner/repo/sub`, a
/// reusable workflow's `owner/repo/.github/workflows/x.yml`). Owner, repository and ref
/// compare without case. Local (`./`), `docker://` and expression values match nothing.
pub(crate) fn banned_entry_for<'a>(
    uses: &str,
    banned: &'a [crate::config::BannedAction],
) -> Option<&'a crate::config::BannedAction> {
    let uses = uses.trim();
    if uses.starts_with("./") || docker_image(uses).is_some() || uses.contains("${{") {
        return None;
    }
    let (target, r) = match uses.split_once('@') {
        Some((t, r)) => (t.to_ascii_lowercase(), Some(r.to_ascii_lowercase())),
        None => (uses.to_ascii_lowercase(), None),
    };
    banned.iter().find(|b| {
        let (bt, br) = b.target_and_ref();
        let bt = bt.to_ascii_lowercase();
        let target_hit = target == bt || target.starts_with(&format!("{bt}/"));
        let ref_hit = br.is_none_or(|br| r.as_deref() == Some(br.to_ascii_lowercase().as_str()));
        target_hit && ref_hit
    })
}

/// Reports every reference to a `banned_actions` entry in the scanned files of the head
/// tree: steps, job-level reusable workflows and composite actions' nested steps. Never
/// lifted by a directive or an inline allow; `examined` grows by the references compared.
pub(super) fn check_banned(
    ctx: &Context,
    filter: &crate::guards::PathFilter,
    workflow_globs: &crate::guards::PathFilter,
    out: &mut GateOutcome,
) -> Result<()> {
    let settings = &ctx.config.gates.ci_integrity;
    let mut compared = 0usize;
    for path in ctx.git.tracked_files()? {
        if filter.matches(&path)
            || !workflow_globs.matches(&path)
            || super::ci_gitlab::is_gitlab_ci_path(&path)
        {
            continue;
        }
        let Some(content) = ctx.git.head_content(&path)? else {
            continue;
        };
        let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(&content) else {
            out.notes.push(format!(
                "{path}: does not parse as YAML; its references were not compared with banned_actions"
            ));
            continue;
        };
        let refs = if is_action_metadata_path(&path) {
            action_pin_refs(&doc, &content)
        } else {
            workflow_pin_refs(&doc, &content)
        };
        for r in refs.iter().filter(|r| r.kind != PinKind::Image) {
            compared += 1;
            let at = r.line.map(|l| format!(":{l}")).unwrap_or_default();
            if r.value.contains("${{") {
                out.notes.push(format!(
                    "{path}{at}: {} reference '{}' is an expression; it cannot be compared with banned_actions (not a pass)",
                    r.site, r.value
                ));
                continue;
            }
            let Some(entry) = banned_entry_for(&r.value, &settings.banned_actions) else {
                continue;
            };
            let reason = entry
                .reason
                .as_deref()
                .map(|why| format!(": {why}"))
                .unwrap_or_default();
            out.push(
                settings.severity,
                &crate::findings::BANNED_ACTION,
                Some(&path),
                r.line,
                format!(
                    "'{}' ({}) matches the banned_actions entry '{}'{reason}.",
                    r.value, r.site, entry.uses
                ),
                "Remove the reference. No directive lifts this finding; only removing the reference, or the banned_actions entry (which config-integrity reports as a loosening), clears it.",
            );
            if r.line.is_none() {
                out.anchor_last(format!("{}:{}", r.site, r.value));
            }
        }
    }
    out.examined += compared;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guards::ci_integrity::test_support::*;

    fn banned(entries: &[&str]) -> Vec<crate::config::BannedAction> {
        entries
            .iter()
            .map(|e| crate::config::BannedAction {
                uses: e.to_string(),
                reason: None,
            })
            .collect()
    }

    #[test]
    fn banned_entries_match_every_ref_one_ref_or_the_paths_under_a_repo() {
        let list = banned(&[
            "actions-cool/issues-helper",
            "evil/thing@v2",
            &format!("x/y@{SHA}"),
        ]);
        let hit = |u: &str| banned_entry_for(u, &list).map(|b| b.uses.clone());
        // Positive controls.
        assert_eq!(
            hit("actions-cool/issues-helper@v3").as_deref(),
            Some("actions-cool/issues-helper")
        );
        assert_eq!(
            hit(&format!("actions-cool/issues-helper@{SHA}")).as_deref(),
            Some("actions-cool/issues-helper")
        );
        assert_eq!(
            hit("Actions-Cool/Issues-Helper@v3").as_deref(),
            Some("actions-cool/issues-helper")
        );
        assert_eq!(
            hit("actions-cool/issues-helper/sub@v3").as_deref(),
            Some("actions-cool/issues-helper")
        );
        assert_eq!(hit("evil/thing@v2").as_deref(), Some("evil/thing@v2"));
        assert_eq!(
            hit(&format!("x/y@{}", SHA.to_uppercase())).as_deref(),
            Some(format!("x/y@{SHA}").as_str())
        );
        // Negative controls.
        assert_eq!(hit("actions-cool/issues-helper-v2@v3"), None);
        assert_eq!(hit("actions-cool/other@v3"), None);
        assert_eq!(hit("evil/thing@v3"), None);
        assert_eq!(hit("x/y@v1"), None);
        assert_eq!(hit("./actions-cool/issues-helper"), None);
        assert_eq!(hit("docker://actions-cool/issues-helper:v3"), None);
        assert_eq!(hit("${{ matrix.action }}"), None);
    }

    #[test]
    fn banned_entries_read_from_strings_and_tables_and_malformed_ones_are_refused() {
        let cfg = crate::config::DisciplineConfig::from_toml_str(
            "[gates.ci-integrity]\nbanned_actions = [\"a/b\", { uses = \"c/d@v1\", reason = \"compromised\" }]\n",
        )
        .unwrap();
        let list = &cfg.gates.ci_integrity.banned_actions;
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].uses, "a/b");
        assert_eq!(list[1].reason.as_deref(), Some("compromised"));
        for bad in [
            "\"nosla\"",
            "\"a/b@\"",
            "\"./local\"",
            "\"docker://alpine\"",
            "\"a/${{ x }}\"",
            "{ uses = \"a/b\", note = \"x\" }",
            "{ reason = \"x\" }",
        ] {
            let src = format!("[gates.ci-integrity]\nbanned_actions = [{bad}]\n");
            assert!(
                crate::config::DisciplineConfig::from_toml_str(&src).is_err(),
                "{bad} was accepted"
            );
        }
    }
}
