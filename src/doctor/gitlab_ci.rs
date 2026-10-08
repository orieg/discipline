//! `.gitlab-ci.yml`: which jobs run discipline and whether one of them can fail without
//! failing the pipeline.

use super::{DisciplineJob, Finding, LocalFacts, Status};

/// GitLab CI keys that are not jobs.
const GITLAB_RESERVED: &[&str] = &[
    "stages",
    "variables",
    "include",
    "default",
    "workflow",
    "image",
    "services",
    "before_script",
    "after_script",
    "cache",
    "spec",
];

/// Analyse `.gitlab-ci.yml`: jobs that run discipline, directly or through the published
/// component or template.
pub fn analyse_gitlab_ci(content: &str) -> LocalFacts {
    let mut facts = LocalFacts::default();
    let path = ".gitlab-ci.yml".to_string();
    let docs: Vec<serde_yaml::Value> = match serde_yaml::Deserializer::from_str(content)
        .map(serde::Deserialize::deserialize)
        .collect::<Result<Vec<serde_yaml::Value>, _>>()
    {
        Ok(d) => d,
        Err(_) => {
            facts.findings.push(
                Finding::new(
                    "workflows",
                    Status::Unknown,
                    ".gitlab-ci.yml is not valid YAML",
                )
                .fix("Fix the pipeline file; doctor cannot tell whether it runs discipline."),
            );
            return facts;
        }
    };
    let as_bool = |v: Option<&serde_yaml::Value>| match v {
        Some(serde_yaml::Value::Bool(b)) => Some(*b),
        Some(serde_yaml::Value::String(s)) => match s.trim() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        Some(serde_yaml::Value::Mapping(_)) => Some(true), // `allow_failure: {exit_codes: ...}`
        _ => None,
    };
    for doc in &docs {
        let Some(map) = doc.as_mapping() else {
            continue;
        };
        // Includes of the published component or template define a `discipline` job.
        let includes: Vec<&serde_yaml::Value> = match map.get("include") {
            Some(serde_yaml::Value::Sequence(seq)) => seq.iter().collect(),
            Some(v) => vec![v],
            None => Vec::new(),
        };
        for inc in includes {
            let target = match inc {
                serde_yaml::Value::String(s) => s.clone(),
                other => ["component", "remote", "file", "template", "local"]
                    .iter()
                    .find_map(|k| other.get(*k).and_then(|v| v.as_str()).map(str::to_string))
                    .unwrap_or_default(),
            };
            if target.contains("discipline") {
                let allow = as_bool(inc.get("inputs").and_then(|i| i.get("allow_failure")))
                    .unwrap_or(false);
                facts.jobs.push(DisciplineJob {
                    workflow: path.clone(),
                    job_id: "discipline".into(),
                    push_branches: None,
                    context: "discipline".into(),
                    rollups: Vec::new(),
                    weak_rollups: Vec::new(),
                    workflow_names: Vec::new(),
                    allow_failure: allow,
                    reads_pull_requests: false,
                });
            }
        }
        for (k, raw) in map {
            let Some(id) = k.as_str() else { continue };
            if id.starts_with('.') || GITLAB_RESERVED.contains(&id) || !raw.is_mapping() {
                continue;
            }
            let resolved = gitlab_resolve_extends(map, raw, 0);
            let job = &resolved;
            let image = match job.get("image") {
                Some(serde_yaml::Value::String(s)) => s.clone(),
                Some(v) => v
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or_default()
                    .to_string(),
                None => String::new(),
            };
            let script_runs = ["script", "before_script"].iter().any(|key| {
                let lines: Vec<String> = match job.get(*key) {
                    Some(serde_yaml::Value::String(s)) => vec![s.clone()],
                    Some(serde_yaml::Value::Sequence(seq)) => seq
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect(),
                    _ => Vec::new(),
                };
                lines
                    .iter()
                    .flat_map(|l| l.lines().map(str::to_string).collect::<Vec<_>>())
                    .any(|l| {
                        let l = l.trim_start();
                        !l.starts_with('#')
                            && (l.contains("discipline check") || l.contains("discipline diff"))
                    })
            });
            if image.contains("orieg/discipline") || script_runs {
                if facts.jobs.iter().any(|j| j.job_id == id) {
                    // A local override of the included job: its settings win.
                    facts.jobs.retain(|j| j.job_id != id);
                }
                facts.jobs.push(DisciplineJob {
                    workflow: path.clone(),
                    job_id: id.to_string(),
                    push_branches: None,
                    context: id.to_string(),
                    rollups: Vec::new(),
                    weak_rollups: Vec::new(),
                    workflow_names: Vec::new(),
                    allow_failure: gitlab_job_can_fail_silently(job),
                    reads_pull_requests: false,
                });
            }
        }
    }
    if facts.jobs.is_empty() {
        facts.findings.push(
            Finding::new(
                "workflows",
                Status::Fail,
                "no .gitlab-ci.yml job runs discipline",
            )
            .fix("Include the discipline component (templates/discipline.gitlab-ci.yml)."),
        );
    } else {
        let names: Vec<&str> = facts.jobs.iter().map(|j| j.job_id.as_str()).collect();
        facts.findings.push(Finding::new(
            "workflows",
            Status::Pass,
            format!(
                "discipline runs in .gitlab-ci.yml job(s): {}",
                names.join(", ")
            ),
        ));
        for j in facts.jobs.iter().filter(|j| j.allow_failure) {
            facts.findings.push(
                Finding::new(
                    "allow-failure",
                    Status::Fail,
                    format!(
                        "job `{}` has allow_failure: its failure cannot fail the pipeline",
                        j.job_id
                    ),
                )
                .fix("Remove `allow_failure` (or set the component input to false)."),
            );
        }
    }
    facts
}

/// A GitLab job with its `extends:` templates merged in (parents first, the job last).
fn gitlab_resolve_extends(
    all: &serde_yaml::Mapping,
    job: &serde_yaml::Value,
    depth: usize,
) -> serde_yaml::Value {
    let parents: Vec<String> = match job.get("extends") {
        Some(serde_yaml::Value::String(s)) => vec![s.clone()],
        Some(serde_yaml::Value::Sequence(seq)) => seq
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    };
    let mut merged = serde_yaml::Mapping::new();
    if depth < 10 {
        for parent in parents {
            if let Some(p) = all.get(serde_yaml::Value::String(parent)) {
                if let serde_yaml::Value::Mapping(pm) = gitlab_resolve_extends(all, p, depth + 1) {
                    merged.extend(pm);
                }
            }
        }
    }
    if let Some(own) = job.as_mapping() {
        merged.extend(own.clone());
    }
    serde_yaml::Value::Mapping(merged)
}

/// Whether a GitLab job's failure leaves the pipeline green: `allow_failure: true` (or an
/// `exit_codes` form), `when: manual` without `allow_failure: false`, or a `rules:` entry
/// that sets either.
fn gitlab_job_can_fail_silently(job: &serde_yaml::Value) -> bool {
    let allow = |v: &serde_yaml::Value| match v.get("allow_failure") {
        Some(serde_yaml::Value::Bool(b)) => Some(*b),
        Some(serde_yaml::Value::String(s)) => Some(s.trim() == "true"),
        Some(serde_yaml::Value::Mapping(_)) => Some(true),
        _ => None,
    };
    let manual = |v: &serde_yaml::Value| v.get("when").and_then(|w| w.as_str()) == Some("manual");
    let silent = |v: &serde_yaml::Value| match allow(v) {
        Some(a) => a,
        None => manual(v),
    };
    silent(job)
        || job
            .get("rules")
            .and_then(|r| r.as_sequence())
            .is_some_and(|rules| {
                rules
                    .iter()
                    .any(|r| allow(r) == Some(true) || (manual(r) && allow(r) != Some(false)))
            })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::test_support::*;
    use crate::doctor::{gitlab_protection, protection_findings, Protection};
    use crate::forge::{CannedApi, ForgeKind};

    const GITLAB_CI: &str = r#"
include:
  - component: gitlab.com/orieg/discipline/discipline@v0
    inputs:
      allow_failure: false
test:
  script: [cargo test]
"#;

    #[test]
    fn gitlab_pipeline_gate_and_protected_branch() {
        let local = analyse_gitlab_ci(GITLAB_CI);
        assert_eq!(local.jobs.len(), 1);
        assert!(!local.jobs[0].allow_failure);
        let mut api = CannedApi::default();
        api.responses.insert(
            "gitlab:projects/o%2Fr".into(),
            serde_json::json!({"default_branch": "main", "only_allow_merge_if_pipeline_succeeds": true,
                               "merge_method": "ff"}),
        );
        // Shape recorded from gitlab.com (protected_branches/main of a public project).
        api.responses.insert(
            "gitlab:projects/o%2Fr/protected_branches/main".into(),
            serde_json::json!({"name": "main", "allow_force_push": false,
              "push_access_levels": [{"access_level": 0, "access_level_description": "No one"}],
              "merge_access_levels": [{"access_level": 40}], "code_owner_approval_required": true}),
        );
        let p = gitlab_protection(&api, &forge(ForgeKind::GitLab), "main").unwrap();
        let f = protection_findings(ForgeKind::GitLab, &p, &local.jobs);
        assert!(
            f.iter()
                .all(|x| matches!(x.status, Status::Pass | Status::Info)),
            "{f:?}"
        );

        // Anonymous: the pipeline setting is hidden (gitlab.com returns null).
        api.responses.insert(
            "gitlab:projects/o%2Fr".into(),
            serde_json::json!({"default_branch": "main", "only_allow_merge_if_pipeline_succeeds": null}),
        );
        api.responses.insert(
            "gitlab:projects/o%2Fr/protected_branches/main".into(),
            serde_json::Value::Null,
        );
        let p = gitlab_protection(&api, &forge(ForgeKind::GitLab), "main").unwrap();
        let f = protection_findings(ForgeKind::GitLab, &p, &local.jobs);
        let get = |id: &str| f.iter().find(|x| x.id == id).unwrap().status;
        assert_eq!(get("required-check"), Status::Unknown);
        assert_eq!(get("force-push"), Status::Fail);
    }

    #[test]
    fn gitlab_local_jobs_and_allow_failure() {
        let direct = "check:\n  image: ghcr.io/orieg/discipline:v0\n  script: [discipline check]\n  allow_failure: true\n.hidden:\n  script: [discipline check]\n";
        let facts = analyse_gitlab_ci(direct);
        assert_eq!(facts.jobs.len(), 1);
        assert!(facts.jobs[0].allow_failure);
        assert!(facts
            .findings
            .iter()
            .any(|f| f.id == "allow-failure" && f.status == Status::Fail));
        for silent in [
            "check:\n  script: [discipline check]\n  when: manual\n",
            "check:\n  script: [discipline check]\n  rules:\n    - if: $CI_PIPELINE_SOURCE == \"merge_request_event\"\n      allow_failure: true\n",
            ".base:\n  allow_failure: true\ncheck:\n  extends: .base\n  script: [discipline check]\n",
        ] {
            let f = analyse_gitlab_ci(silent);
            assert!(f.jobs[0].allow_failure, "{silent}");
        }
        let blocking = analyse_gitlab_ci(
            ".base:\n  allow_failure: true\ncheck:\n  extends: .base\n  allow_failure: false\n  script: [discipline check]\n  when: manual\n",
        );
        assert!(
            !blocking.jobs[0].allow_failure,
            "an explicit allow_failure: false wins"
        );
        let none = analyse_gitlab_ci("test:\n  script: [make]\n");
        assert_eq!(none.findings[0].status, Status::Fail);

        let jobs = facts.jobs.clone();
        let p = Protection {
            pipeline_must_succeed: Some(Some(true)),
            ..Protection::default()
        };
        let f = protection_findings(ForgeKind::GitLab, &p, &jobs);
        assert_eq!(
            f[0].status,
            Status::Fail,
            "an allow_failure job gates nothing"
        );
    }
}
