//! Workflow triggers and token permissions: `pull_request_target` and a token widened to
//! write.

use super::{find_line_number, record_or_excuse, WorkflowFile};
use crate::guards::{Context, GateOutcome};

/// Workflow-level checks: a `pull_request_target` trigger introduced, permissions
/// widened, `timeout-minutes` removed.
pub(super) fn check_triggers_and_permissions(
    ctx: &Context,
    wf: &WorkflowFile,
    head_doc: &serde_yaml::Value,
    out: &mut GateOutcome,
) {
    let settings = &ctx.config.gates.ci_integrity;
    let path = wf.path;
    let head_content = wf.head_content;
    let base_val = wf.base;
    // Check pull_request_target
    let head_has_pr_target = workflow_has_trigger(head_doc, "pull_request_target");
    let base_has_pr_target = base_val
        .map(|b| workflow_has_trigger(b, "pull_request_target"))
        .unwrap_or(false);

    if head_has_pr_target && !base_has_pr_target {
        let line_no = find_line_number(head_content, "pull_request_target");
        record_or_excuse(
            ctx,
            Some(head_content),
            out,
            settings.severity,
            &crate::findings::PULL_REQUEST_TARGET_TRIGGER,
            Some(path.to_string()),
            line_no,
            "Workflow introduces 'pull_request_target' trigger, which executes with repository write access and secrets.".to_string(),
            "Use 'pull_request' instead, or excuse with allow-gate-weakening: ci-integrity <reason>.",
            "pull_request_target",
        );
    }

    // Check permissions widening
    if let Some(base_doc) = base_val {
        let head_perm_level = workflow_permission_level(head_doc);
        let base_perm_level = workflow_permission_level(base_doc);
        if head_perm_level > base_perm_level {
            let line_no = find_line_number(head_content, "permissions:");
            record_or_excuse(
                ctx,
                Some(head_content),
                out,
                settings.severity,
                &crate::findings::WORKFLOW_PERMISSIONS_WIDENED,
                Some(path.to_string()),
                line_no,
                "Workflow permissions were widened from base ref (e.g. gained write privileges).".to_string(),
                "Keep permissions minimal (e.g. read-all or specific read scopes), or excuse with allow-gate-weakening: ci-integrity <reason>.",
                "permissions",
            );
        }
    }

    // Check workflow-level timeout-minutes
    if let Some(base_doc) = base_val {
        if base_doc.get("timeout-minutes").is_some() && head_doc.get("timeout-minutes").is_none() {
            record_or_excuse(
                ctx,
                Some(head_content),
                out,
                settings.severity,
                &crate::findings::WORKFLOW_TIMEOUT_REMOVED,
                Some(path.to_string()),
                None,
                "Workflow-level 'timeout-minutes' was removed.".to_string(),
                "Restore timeout-minutes or excuse with allow-gate-weakening: ci-integrity <reason>.",
                "timeout-minutes",
            );
        }
    }
}

pub(crate) fn workflow_has_trigger(val: &serde_yaml::Value, trigger: &str) -> bool {
    if let Some(on_val) = val.get("on") {
        if let Some(s) = on_val.as_str() {
            return s == trigger;
        }
        if let Some(seq) = on_val.as_sequence() {
            return seq.iter().any(|item| item.as_str() == Some(trigger));
        }
        if on_val.get(trigger).is_some() {
            return true;
        }
    }
    false
}

fn workflow_permission_level(val: &serde_yaml::Value) -> u8 {
    if let Some(perm) = val.get("permissions") {
        if let Some(s) = perm.as_str() {
            if s == "write-all" {
                return 2;
            }
            if s == "read-all" {
                return 1;
            }
        }
        if let Some(map) = perm.as_mapping() {
            if map.is_empty() {
                return 0;
            }
            let has_write = map.values().any(|v| v.as_str() == Some("write"));
            if has_write {
                return 2;
            }
            return 1;
        }
    } else {
        // Omitting permissions defaults to permissive / repository-default (which includes write access)
        return 2;
    }
    0
}
