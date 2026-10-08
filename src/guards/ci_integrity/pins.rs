//! Pinning of actions, reusable workflows and container images.

use super::{
    child_key_line, docker_image, job_line, record_or_excuse, step_line, step_spans, top_key_line,
    Span, GATE,
};
use crate::guards::{line_allows, Context, GateOutcome};
use std::collections::HashSet;

/// What a pinned reference points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PinKind {
    /// A step's `uses:` action (`owner/repo@ref`), in a workflow or a composite action.
    Action,
    /// A job-level `uses:` calling a reusable workflow (`owner/repo/.github/workflows/x.yml@ref`).
    ReusableWorkflow,
    /// A container image: `container:`, `services.<name>.image` or `uses: docker://`.
    Image,
}

/// One remote reference a workflow or composite action pulls in, and where it is.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PinRef {
    pub kind: PinKind,
    /// The reference as written (`docker://alpine:latest`, `node:20`, `owner/repo@v1`).
    pub value: String,
    /// Where it sits, for the message (`job 'build' container`).
    pub site: String,
    /// 1-based line of the reference in the file, when found.
    pub line: Option<usize>,
    /// The step's own line (its name or id), whose inline exemption also covers the
    /// reference, as it always has for step-level `uses:`.
    pub step_line: Option<usize>,
}

/// How a reference fares against the pinning rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PinVerdict {
    /// Pinned, local, first-party, or not a remote reference: nothing to report.
    Pinned,
    /// A mutable reference: an action or reusable workflow not at a 40-hex commit SHA, or
    /// an image without an `@sha256:<64 hex>` digest.
    Unpinned,
    /// An expression (`${{ matrix.image }}`) the gate cannot resolve: a note, never a pass.
    Expression,
}

/// Whether an image reference carries an immutable `@sha256:<64 hex>` digest.
pub(crate) fn image_has_digest(image: &str) -> bool {
    image.rsplit_once("@sha256:").is_some_and(|(name, digest)| {
        !name.is_empty() && digest.len() == 64 && digest.chars().all(|c| c.is_ascii_hexdigit())
    })
}

/// Whether a git ref is a full 40-character commit SHA.
fn is_commit_sha(r: &str) -> bool {
    r.len() == 40 && r.chars().all(|c| c.is_ascii_hexdigit())
}

/// Judges one reference against the pinning rule. `first_party` prefixes exempt an
/// action or a reusable workflow; local (`./`) references are exempt.
pub(crate) fn pin_verdict(kind: PinKind, value: &str, first_party: &[String]) -> PinVerdict {
    let value = value.trim();
    if value.is_empty() {
        return PinVerdict::Pinned;
    }
    if value.contains("${{") {
        return PinVerdict::Expression;
    }
    match kind {
        PinKind::Image => {
            if image_has_digest(docker_image(value).unwrap_or(value)) {
                PinVerdict::Pinned
            } else {
                PinVerdict::Unpinned
            }
        }
        PinKind::Action | PinKind::ReusableWorkflow => {
            if value.starts_with("./") {
                return PinVerdict::Pinned;
            }
            let Some((target, r)) = value.split_once('@') else {
                return PinVerdict::Pinned;
            };
            if first_party.iter().any(|p| target.starts_with(p.as_str())) || is_commit_sha(r) {
                PinVerdict::Pinned
            } else {
                PinVerdict::Unpinned
            }
        }
    }
}

/// A step's `uses:`, as an action or, for `docker://`, an image. `span` is the step's own
/// lines; without one (steps not written one item each) the reference is reported at
/// `fallback`, the line of what holds the steps.
fn step_ref(
    step: &serde_yaml::Value,
    site: String,
    content: &str,
    span: Option<Span>,
    fallback: Option<usize>,
) -> Option<PinRef> {
    let uses = step.get("uses").and_then(|u| u.as_str())?;
    let name = step.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let id = step.get("id").and_then(|i| i.as_str()).unwrap_or("");
    let run = step.get("run").and_then(|r| r.as_str());
    Some(PinRef {
        kind: if docker_image(uses).is_some() {
            PinKind::Image
        } else {
            PinKind::Action
        },
        value: uses.to_string(),
        site,
        line: span
            .and_then(|s| s.item_key_line(content, "uses"))
            .or(fallback),
        step_line: span.map(|s| step_line(content, s, name, id, Some(uses), run)),
    })
}

/// Every remote reference of a workflow: step and job-level `uses:`, `container:`
/// (string or `image:`) and `services.<name>.image`.
pub(crate) fn workflow_pin_refs(doc: &serde_yaml::Value, content: &str) -> Vec<PinRef> {
    let mut refs = Vec::new();
    let Some(jobs) = doc.get("jobs").and_then(|j| j.as_mapping()) else {
        return refs;
    };
    for (job_k, job) in jobs {
        let job_id = job_k.as_str().unwrap_or("");
        let job_line = job_line(content, job_id);
        // A key of the job itself, and a key below one.
        let own = |key: &str| job_line.and_then(|l| child_key_line(content, l, key));
        let below =
            |parent: Option<usize>, key: &str| parent.and_then(|l| child_key_line(content, l, key));
        if let Some(uses) = job.get("uses").and_then(|u| u.as_str()) {
            refs.push(PinRef {
                kind: PinKind::ReusableWorkflow,
                value: uses.to_string(),
                site: format!("job '{job_id}' reusable workflow"),
                line: own("uses").or(job_line),
                step_line: None,
            });
        }
        let container_line = own("container");
        let container = match job.get("container") {
            Some(serde_yaml::Value::String(s)) => Some((s.as_str(), container_line)),
            Some(m) => m
                .get("image")
                .and_then(|i| i.as_str())
                .map(|i| (i, below(container_line, "image").or(container_line))),
            None => None,
        };
        if let Some((image, line)) = container {
            refs.push(PinRef {
                kind: PinKind::Image,
                value: image.to_string(),
                site: format!("job '{job_id}' container"),
                line: line.or(job_line),
                step_line: None,
            });
        }
        if let Some(services) = job.get("services").and_then(|s| s.as_mapping()) {
            let services_line = own("services");
            for (svc_k, svc) in services {
                let svc_id = svc_k.as_str().unwrap_or("");
                if let Some(image) = svc.get("image").and_then(|i| i.as_str()) {
                    let svc_line = below(services_line, svc_id);
                    refs.push(PinRef {
                        kind: PinKind::Image,
                        value: image.to_string(),
                        site: format!("job '{job_id}' service '{svc_id}'"),
                        line: below(svc_line, "image")
                            .or(svc_line)
                            .or(services_line)
                            .or(job_line),
                        step_line: None,
                    });
                }
            }
        }
        if let Some(steps) = job.get("steps").and_then(|s| s.as_sequence()) {
            let spans = job_line.and_then(|l| step_spans(content, l, steps.len()));
            for (i, step) in steps.iter().enumerate() {
                let site = format!("job '{job_id}' step");
                let span = spans.as_ref().map(|s| s[i]);
                if let Some(r) = step_ref(step, site, content, span, job_line) {
                    refs.push(r);
                }
            }
        }
    }
    refs
}

/// The nested `runs.steps[*].uses` of a composite action's metadata file.
pub(crate) fn action_pin_refs(doc: &serde_yaml::Value, content: &str) -> Vec<PinRef> {
    let Some(steps) = doc
        .get("runs")
        .and_then(|r| r.get("steps"))
        .and_then(|s| s.as_sequence())
    else {
        return Vec::new();
    };
    let runs_line = top_key_line(content, "runs");
    let spans = runs_line.and_then(|l| step_spans(content, l, steps.len()));
    steps
        .iter()
        .enumerate()
        .filter_map(|(i, step)| {
            let span = spans.as_ref().map(|s| s[i]);
            step_ref(step, "composite step".to_string(), content, span, runs_line)
        })
        .collect()
}

/// The base side's references, for telling an added reference from a pre-existing one.
pub(super) fn base_pin_set(refs: Option<Vec<PinRef>>) -> HashSet<(PinKind, String)> {
    refs.unwrap_or_default()
        .into_iter()
        .map(|r| (r.kind, r.value))
        .collect()
}

/// Reports the unpinned references of one file. With `diff_only` (the default) only a
/// reference new relative to the base side is judged; with `diff_only = false` every
/// reference is, and the message says whether this change added it or it was already
/// there (existing ones are adopted through the baseline).
pub(super) fn check_pins(
    ctx: &Context,
    path: &str,
    content: &str,
    refs: &[PinRef],
    base: &HashSet<(PinKind, String)>,
    out: &mut GateOutcome,
) {
    let settings = &ctx.config.gates.ci_integrity;
    for r in refs {
        let pre_existing = base.contains(&(r.kind, r.value.clone()));
        if pre_existing && settings.diff_only {
            continue;
        }
        let verdict = pin_verdict(r.kind, &r.value, &settings.first_party_action_prefixes);
        if verdict == PinVerdict::Pinned {
            continue;
        }
        if r.step_line
            .and_then(|l| content.lines().nth(l.saturating_sub(1)))
            .is_some_and(|l| line_allows(l, GATE))
        {
            continue;
        }
        let at = r.line.map(|l| format!(":{l}")).unwrap_or_default();
        if verdict == PinVerdict::Expression {
            out.notes.push(format!(
                "{path}{at}: {} reference '{}' is an expression; its pin cannot be checked (not a pass)",
                r.site, r.value
            ));
            continue;
        }
        let origin = if pre_existing {
            "pre-existing: already on the base side"
        } else {
            "added by this change"
        };
        let (kind, message, remediation) = match r.kind {
            PinKind::Image => (
                &crate::findings::UNPINNED_CONTAINER_IMAGE,
                format!(
                    "Container image '{}' ({}) has no digest ({origin}). Must be pinned by '@sha256:<64-hex digest>'.",
                    r.value, r.site
                ),
                "Pin the image by its immutable digest (`image:tag@sha256:<digest>`), or excuse with allow-gate-weakening: ci-integrity <reason>.",
            ),
            PinKind::Action | PinKind::ReusableWorkflow => {
                let (target, rf) = r.value.split_once('@').unwrap_or((r.value.as_str(), ""));
                let what = if r.kind == PinKind::Action {
                    "Third-party action"
                } else {
                    "Reusable workflow"
                };
                (
                    &crate::findings::UNPINNED_ACTION,
                    format!(
                        "{what} '{target}' ({}) is unpinned ('@{rf}', {origin}). Must be pinned by a 40-character commit SHA.",
                        r.site
                    ),
                    "Pin the reference by its immutable 40-character commit SHA, or excuse with allow-gate-weakening: ci-integrity <reason>.",
                )
            }
        };
        let before = out.violations.len();
        record_or_excuse(
            ctx,
            Some(content),
            out,
            settings.severity,
            kind,
            Some(path.to_string()),
            r.line,
            message,
            remediation,
            &r.value,
        );
        if r.line.is_none() && out.violations.len() > before {
            out.anchor_last(format!("{}:{}", r.site, r.value));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guards::ci_integrity::test_support::*;
    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn first_party() -> Vec<String> {
        vec!["actions/".to_string(), "github/".to_string()]
    }

    #[test]
    fn reusable_workflow_needs_a_commit_sha_unless_local_or_first_party() {
        let v = |r: &str| pin_verdict(PinKind::ReusableWorkflow, r, &first_party());
        assert_eq!(
            v("evil/reusable/.github/workflows/x.yml@main"),
            PinVerdict::Unpinned
        );
        assert_eq!(
            v("evil/reusable/.github/workflows/x.yml@v1.2.3"),
            PinVerdict::Unpinned
        );
        assert_eq!(
            v(&format!("evil/reusable/.github/workflows/x.yml@{SHA}")),
            PinVerdict::Pinned
        );
        assert_eq!(v("./.github/workflows/local.yml"), PinVerdict::Pinned);
        assert_eq!(
            v("actions/reusable/.github/workflows/x.yml@main"),
            PinVerdict::Pinned
        );
        // A SHA one character short is not a SHA.
        assert_eq!(
            v(&format!("evil/r/.github/workflows/x.yml@{}", &SHA[1..])),
            PinVerdict::Unpinned
        );
    }

    #[test]
    fn default_prefixes_exempt_no_owner_from_the_sha_rule() {
        let defaults = crate::config::CiIntegrityGate::default();
        assert!(defaults.first_party_action_prefixes.is_empty());
        let v = |r: &str| pin_verdict(PinKind::Action, r, &defaults.first_party_action_prefixes);
        assert_eq!(v("actions/checkout@v4"), PinVerdict::Unpinned);
        assert_eq!(v("github/codeql-action/init@v3"), PinVerdict::Unpinned);
        assert_eq!(v(&format!("actions/checkout@{SHA}")), PinVerdict::Pinned);
        // A repository that opts back in still exempts the prefix it lists.
        assert_eq!(
            pin_verdict(PinKind::Action, "actions/checkout@v4", &first_party()),
            PinVerdict::Pinned
        );
    }

    #[test]
    fn container_image_needs_a_sha256_digest() {
        let v = |r: &str| pin_verdict(PinKind::Image, r, &first_party());
        assert_eq!(v("node:latest"), PinVerdict::Unpinned);
        assert_eq!(v("node"), PinVerdict::Unpinned);
        assert_eq!(v("docker://alpine:latest"), PinVerdict::Unpinned);
        assert_eq!(v(&format!("node:20@sha256:{DIGEST}")), PinVerdict::Pinned);
        assert_eq!(
            v(&format!("docker://alpine@sha256:{DIGEST}")),
            PinVerdict::Pinned
        );
        assert_eq!(
            v(&format!("ghcr.io/o/i:1@sha256:{}", &DIGEST[1..])),
            PinVerdict::Unpinned
        );
        assert_eq!(v("node@sha256:not-a-digest"), PinVerdict::Unpinned);
        // A first-party prefix does not exempt an image.
        assert_eq!(v("actions/runner:latest"), PinVerdict::Unpinned);
        assert_eq!(v("${{ matrix.image }}"), PinVerdict::Expression);
        assert_eq!(v("node:${{ matrix.v }}"), PinVerdict::Expression);
    }

    #[test]
    fn workflow_refs_cover_steps_jobs_containers_services_and_docker_steps() {
        let wf = format!(
            "on: push\njobs:\n  triage:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions-cool/issues-helper@v2.2.1\n  r:\n    uses: evil/reusable/.github/workflows/x.yml@main\n  build:\n    runs-on: ubuntu-latest\n    container:\n      image: node:latest\n    services:\n      db:\n        image: postgres@sha256:{DIGEST}\n    steps:\n      - uses: docker://alpine:latest\n      - uses: docker://alpine:latest\n"
        );
        let doc: serde_yaml::Value = serde_yaml::from_str(&wf).unwrap();
        let refs = workflow_pin_refs(&doc, &wf);
        let got: Vec<(PinKind, &str, Option<usize>)> = refs
            .iter()
            .map(|r| (r.kind, r.value.as_str(), r.line))
            .collect();
        let db = format!("postgres@sha256:{DIGEST}");
        assert_eq!(
            got,
            vec![
                (
                    PinKind::Action,
                    "actions-cool/issues-helper@v2.2.1",
                    Some(6)
                ),
                (
                    PinKind::ReusableWorkflow,
                    "evil/reusable/.github/workflows/x.yml@main",
                    Some(8)
                ),
                (PinKind::Image, "node:latest", Some(12)),
                (PinKind::Image, db.as_str(), Some(15)),
                // A repeated reference is located at its own line.
                (PinKind::Image, "docker://alpine:latest", Some(17)),
                (PinKind::Image, "docker://alpine:latest", Some(18)),
            ]
        );
        // The string form of `container:`.
        let wf = "jobs:\n  b:\n    container: node:20\n";
        let doc: serde_yaml::Value = serde_yaml::from_str(wf).unwrap();
        let refs = workflow_pin_refs(&doc, wf);
        assert_eq!(refs.len(), 1);
        assert_eq!((refs[0].kind, refs[0].line), (PinKind::Image, Some(3)));
        assert_eq!(refs[0].value, "node:20");
        // No jobs: nothing.
        let doc: serde_yaml::Value = serde_yaml::from_str("name: x\n").unwrap();
        assert!(workflow_pin_refs(&doc, "name: x\n").is_empty());
    }

    #[test]
    fn composite_action_refs_are_its_nested_step_uses() {
        let action = "name: setup\nruns:\n  using: composite\n  steps:\n    - uses: evil/thing@v1\n    - run: echo hi\n      shell: bash\n    - uses: docker://alpine:3\n";
        let doc: serde_yaml::Value = serde_yaml::from_str(action).unwrap();
        let got: Vec<(PinKind, String, Option<usize>)> = action_pin_refs(&doc, action)
            .into_iter()
            .map(|r| (r.kind, r.value, r.line))
            .collect();
        assert_eq!(
            got,
            vec![
                (PinKind::Action, "evil/thing@v1".to_string(), Some(5)),
                (PinKind::Image, "docker://alpine:3".to_string(), Some(8)),
            ]
        );
        let docker = "runs:\n  using: docker\n  image: Dockerfile\n";
        let doc: serde_yaml::Value = serde_yaml::from_str(docker).unwrap();
        assert!(action_pin_refs(&doc, docker).is_empty());
    }

    #[test]
    fn key_lines_match_whole_keys_only() {
        use crate::guards::ci_integrity::{child_key_line, job_key_line, top_key_line};
        let c = "jobs:\n  rr:\n    runs-on: x\n  r:\n    uses: a/b@v1\n  'q':\n";
        assert_eq!(job_key_line(c, "r"), Some(4));
        assert_eq!(job_key_line(c, "q"), Some(6));
        assert_eq!(top_key_line(c, "jobs"), Some(1));
        // `rr` is a job, not a key of the job on line 4.
        assert_eq!(child_key_line(c, 4, "rr"), None);
    }
}
