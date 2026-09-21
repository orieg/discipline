//! `ci-integrity` for GitLab CI pipeline files.
//!
//! A GitLab pipeline is a mapping of job names to jobs, not the `jobs:` / `steps:` shape
//! the Actions rules read, so it is diffed here. Every rule is a base-versus-head delta:
//! what the base side already tolerated is not reported.
//!
//! Not read: pipelines pulled in through `include:` (another file, project or URL), and
//! narrowing of `rules:` / `only:` / `except:`. A weakening made there passes this gate.

use std::collections::BTreeMap;

use serde_yaml::Value;

/// One weakening of a GitLab pipeline file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitlabWeakening {
    pub title: &'static str,
    pub job: String,
    pub message: String,
    /// What an `allow-ci-weakening:` reason must name.
    pub subject: String,
}

/// Top-level keys that configure the pipeline rather than define a job.
const RESERVED: &[&str] = &[
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
    "pages:deploy",
];

/// Whether `path` is a GitLab pipeline file this module reads.
pub fn is_gitlab_ci_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name == ".gitlab-ci.yml"
        || name == ".gitlab-ci.yaml"
        || ((path.starts_with(".gitlab/ci/") || path.starts_with(".gitlab-ci/"))
            && (name.ends_with(".yml") || name.ends_with(".yaml")))
}

/// Jobs by name, merged across the documents of a multi-document file (a `spec:` header
/// is its own document). Hidden jobs (`.name`) are templates and are kept: a masked
/// script in a template masks every job that extends it.
fn jobs(content: &str) -> Result<BTreeMap<String, Value>, String> {
    let mut found = BTreeMap::new();
    for doc in serde_yaml::Deserializer::from_str(content) {
        let doc: Value =
            serde::Deserialize::deserialize(doc).map_err(|e| format!("not valid YAML: {e}"))?;
        let Some(map) = doc.as_mapping() else {
            continue;
        };
        for (k, v) in map {
            let Some(name) = k.as_str() else { continue };
            if RESERVED.contains(&name) || !v.is_mapping() {
                continue;
            }
            found.insert(name.to_string(), v.clone());
        }
    }
    Ok(found)
}

/// Script lines of a job, across `before_script`, `script` and `after_script`. A script
/// entry may itself be a list (YAML anchors are spliced in that way).
fn script_lines(job: &Value) -> Vec<String> {
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.extend(s.lines().map(|l| l.trim().to_string())),
            Value::Sequence(seq) => seq.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut lines = Vec::new();
    for key in ["before_script", "script", "after_script"] {
        if let Some(v) = job.get(key) {
            walk(v, &mut lines);
        }
    }
    lines.retain(|l| !l.is_empty() && !l.starts_with('#'));
    lines
}

/// `allow_failure: true`, or the mapping form (`exit_codes:`), which tolerates failures too.
fn allows_failure(job: &Value) -> bool {
    match job.get("allow_failure") {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.trim().eq_ignore_ascii_case("true"),
        Some(Value::Mapping(_)) => true,
        _ => false,
    }
}

fn is_manual(job: &Value) -> bool {
    job.get("when").and_then(|w| w.as_str()) == Some("manual")
}

fn masks_exit_code(line: &str) -> bool {
    line.contains("|| true") || line.contains("|| :") || line.contains("set +e")
}

const VERIFICATION_WORDS: &[&str] = &[
    "test",
    "lint",
    "gate",
    "check",
    "clippy",
    "audit",
    "verify",
    "discipline",
    "pytest",
    "sast",
    "scan",
];

/// A job that verifies something, by its name or by what its script runs.
fn is_verification_job(name: &str, job: &Value) -> bool {
    let has_word = |text: &str| {
        let t = text.to_ascii_lowercase();
        VERIFICATION_WORDS.iter().any(|w| t.contains(w))
    };
    has_word(name) || script_lines(job).iter().any(|l| has_word(l))
}

/// Weakenings `head` makes to the pipeline `base` defined. `Err` = a side does not parse.
pub fn diff_gitlab_ci(base: &str, head: &str) -> Result<Vec<GitlabWeakening>, String> {
    let base_jobs = jobs(base).map_err(|e| format!("base side: {e}"))?;
    let head_jobs = jobs(head).map_err(|e| format!("head side: {e}"))?;
    let mut found = Vec::new();

    for (name, b) in &base_jobs {
        if !head_jobs.contains_key(name) && !name.starts_with('.') && is_verification_job(name, b) {
            found.push(GitlabWeakening {
                title: "Deletion of Verification Job",
                job: name.clone(),
                message: format!("Verification job '{name}' was deleted from the pipeline."),
                subject: name.clone(),
            });
        }
    }

    for (name, h) in &head_jobs {
        let b = base_jobs.get(name);
        if allows_failure(h) && !b.is_some_and(allows_failure) {
            found.push(GitlabWeakening {
                title: "allow_failure Masks Failure",
                job: name.clone(),
                message: format!(
                    "Job '{name}' carries 'allow_failure', so its failure no longer fails the pipeline."
                ),
                subject: "allow_failure".to_string(),
            });
        }
        // A job that becomes manual stops running on its own (and a manual job allows
        // failure by default). Only an existing verification job is judged.
        if let Some(b) = b {
            if is_manual(h) && !is_manual(b) && is_verification_job(name, b) {
                found.push(GitlabWeakening {
                    title: "Verification Job Made Manual",
                    job: name.clone(),
                    message: format!(
                        "Verification job '{name}' was changed to 'when: manual'; it no longer runs on its own."
                    ),
                    subject: "when: manual".to_string(),
                });
            }
        }
        let base_lines = b.map(script_lines).unwrap_or_default();
        let head_lines = script_lines(h);
        if head_lines.iter().any(|l| masks_exit_code(l))
            && !base_lines.iter().any(|l| masks_exit_code(l))
        {
            found.push(GitlabWeakening {
                title: "Command Masks Exit Code",
                job: name.clone(),
                message: format!(
                    "Job '{name}' gained a script line that masks a command's exit code ('|| true', '|| :' or 'set +e')."
                ),
                subject: "|| true".to_string(),
            });
        }
        let advisory = |lines: &[String]| super::ci_integrity::run_is_advisory(&lines.join("\n"));
        if advisory(&head_lines) && !advisory(&base_lines) {
            found.push(GitlabWeakening {
                title: "Discipline Run Weakened (--advisory)",
                job: name.clone(),
                message: format!(
                    "Job '{name}' runs discipline with '--advisory'; it exits 0 whatever the gates report."
                ),
                subject: "--advisory".to_string(),
            });
        }
    }
    Ok(found)
}

/// Verification jobs a deleted pipeline file defined.
pub fn verification_jobs(content: &str) -> Result<Vec<String>, String> {
    Ok(jobs(content)?
        .into_iter()
        .filter(|(name, job)| !name.starts_with('.') && is_verification_job(name, job))
        .map(|(name, _)| name)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "stages: [test]\nvariables:\n  allow_failure: \"true\"\n\
        .tmpl:\n  script:\n    - echo setup\n\
        unit-tests:\n  stage: test\n  script:\n    - cargo test --locked\n\
        discipline:\n  script:\n    - discipline check --suite all\n\
        docs:\n  script:\n    - mkdocs build\n  allow_failure: true\n";

    fn titles(head: &str) -> Vec<(&'static str, String)> {
        diff_gitlab_ci(BASE, head)
            .unwrap()
            .into_iter()
            .map(|w| (w.title, w.job))
            .collect()
    }

    #[test]
    fn an_unchanged_or_tightened_pipeline_is_silent() {
        assert!(titles(BASE).is_empty());
        // A job that already allowed failure, a new non-verification job, a new stage.
        let head = BASE.replace("  allow_failure: true\n", "").replace(
            "stages: [test]",
            "stages: [test, deploy]\npages:\n  script:\n    - echo publish",
        );
        assert!(titles(&head).is_empty(), "{:?}", titles(&head));
    }

    #[test]
    fn each_gitlab_weakening_is_reported_against_its_job() {
        let head = BASE
            .replace(
                "    - cargo test --locked\n",
                "    - cargo test --locked || true\n  allow_failure:\n    exit_codes: [1]\n",
            )
            .replace(
                "    - discipline check --suite all\n",
                "    - discipline check --suite all --advisory\n  when: manual\n",
            );
        assert_eq!(
            titles(&head),
            vec![
                ("Verification Job Made Manual", "discipline".to_string()),
                (
                    "Discipline Run Weakened (--advisory)",
                    "discipline".to_string()
                ),
                ("allow_failure Masks Failure", "unit-tests".to_string()),
                ("Command Masks Exit Code", "unit-tests".to_string()),
            ]
        );
    }

    #[test]
    fn a_deleted_verification_job_is_reported_and_a_deleted_docs_job_is_not() {
        let no_tests = BASE.replace(
            "unit-tests:\n  stage: test\n  script:\n    - cargo test --locked\n",
            "",
        );
        assert_eq!(
            titles(&no_tests),
            vec![("Deletion of Verification Job", "unit-tests".to_string())]
        );
        let no_docs = BASE.replace(
            "docs:\n  script:\n    - mkdocs build\n  allow_failure: true\n",
            "",
        );
        assert!(titles(&no_docs).is_empty());
    }

    #[test]
    fn a_masked_line_in_a_template_counts_and_a_comment_does_not() {
        let template = BASE.replace("    - echo setup\n", "    - set +e\n    - echo setup\n");
        assert_eq!(
            titles(&template),
            vec![("Command Masks Exit Code", ".tmpl".to_string())]
        );
        let comment = BASE.replace(
            "    - cargo test --locked\n",
            "    - |\n      # never append || true here\n      cargo test --locked\n",
        );
        assert!(titles(&comment).is_empty(), "{:?}", titles(&comment));
    }

    #[test]
    fn a_side_that_does_not_parse_is_an_error_and_paths_are_recognised() {
        assert!(diff_gitlab_ci(BASE, "unit-tests: [").is_err());
        assert!(diff_gitlab_ci("a: [", BASE).is_err());
        assert!(is_gitlab_ci_path(".gitlab-ci.yml"));
        assert!(is_gitlab_ci_path("sub/.gitlab-ci.yml"));
        assert!(is_gitlab_ci_path(".gitlab/ci/test.yml"));
        assert!(!is_gitlab_ci_path(".github/workflows/ci.yml"));
        assert!(!is_gitlab_ci_path("docs/gitlab-ci.yml"));
    }
}
