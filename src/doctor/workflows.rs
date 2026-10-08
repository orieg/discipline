//! Actions-style workflow files: which jobs run discipline, which jobs roll them up, and
//! the triggers and token permissions of those workflows.

use super::{DisciplineJob, Finding, LocalFacts, Status};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Workflow directories read for each Actions-style platform.
pub const WORKFLOW_DIRS: &[&str] = &[
    ".github/workflows",
    ".gitea/workflows",
    ".forgejo/workflows",
];

/// Whether a workflow step runs discipline.
fn step_runs_discipline(step: &serde_yaml::Value, self_action: bool) -> bool {
    if let Some(uses) = step.get("uses").and_then(|u| u.as_str()) {
        let uses = uses.trim();
        if uses.starts_with("orieg/discipline@")
            || uses.starts_with("docker://ghcr.io/orieg/discipline")
            || (self_action && (uses == "./" || uses == "."))
        {
            return !installs_only(step);
        }
    }
    step.get("run").and_then(|r| r.as_str()).is_some_and(|run| {
        run.lines().any(|l| {
            let l = l.trim_start();
            !l.starts_with('#') && (l.contains("discipline check") || l.contains("discipline diff"))
        })
    })
}

/// Whether an action step sets `install_only` to a literal true: it puts the binary on
/// `PATH` (the Copilot cloud agent's setup steps) and checks nothing. An expression is
/// not resolved, so the step still counts as a check.
fn installs_only(step: &serde_yaml::Value) -> bool {
    match step.get("with").and_then(|w| w.get("install_only")) {
        Some(serde_yaml::Value::Bool(b)) => *b,
        Some(serde_yaml::Value::String(s)) => s.trim() == "true",
        _ => false,
    }
}

fn job_context(job_id: &str, job: &serde_yaml::Value) -> String {
    job.get("name")
        .and_then(|n| n.as_str())
        .filter(|n| !n.contains("${{"))
        .unwrap_or(job_id)
        .to_string()
}

fn job_needs(job: &serde_yaml::Value) -> Vec<String> {
    match job.get("needs") {
        Some(serde_yaml::Value::String(s)) => vec![s.clone()],
        Some(serde_yaml::Value::Sequence(seq)) => seq
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// Whether `from` depends on `target` through a chain of `needs:`.
fn depends_transitively(jobs: &[(String, &serde_yaml::Value)], from: &str, target: &str) -> bool {
    let needs_of = |id: &str| {
        jobs.iter()
            .find(|(j, _)| j == id)
            .map(|(_, job)| job_needs(job))
            .unwrap_or_default()
    };
    let mut stack = needs_of(from);
    let mut seen = BTreeSet::new();
    while let Some(j) = stack.pop() {
        if j == target {
            return true;
        }
        if seen.insert(j.clone()) {
            stack.extend(needs_of(&j));
        }
    }
    false
}

/// A rollup fails when a job it needs failed: it runs regardless of that result and
/// a step reads the results. Without `if: always()` (or `!cancelled()` / `failure()`)
/// the rollup is skipped, and a skipped required check counts as passed.
pub(crate) fn rollup_enforces(job: &serde_yaml::Value) -> bool {
    let runs_after_failure = job
        .get("if")
        .map(|v| match v {
            serde_yaml::Value::String(s) => s.clone(),
            other => serde_yaml::to_string(other).unwrap_or_default(),
        })
        .is_some_and(|c| {
            c.contains("always()") || c.contains("!cancelled()") || c.contains("failure()")
        });
    let reads_results = job
        .get("steps")
        .and_then(|s| s.as_sequence())
        .is_some_and(|steps| {
            steps.iter().any(|st| {
                serde_yaml::to_string(st)
                    .unwrap_or_default()
                    .contains("needs")
            })
        });
    runs_after_failure && reads_results
}

/// `# discipline:advisory <reason>` markers on a workflow's action steps, keyed by the
/// step's address in the parsed workflow. A marker is a YAML comment on the step's
/// `uses:` line or in the comment block directly above the step. Parsing drops
/// comments, so each `uses:` line of the text is paired with the parsed step of the
/// same `uses:` value in document order; when the counts differ (flow-style YAML, a
/// `uses:` inside a script) no marker for that value is read and the FAIL stays.
fn advisory_markers(
    content: &str,
    jobs: &[(String, &serde_yaml::Value)],
) -> BTreeMap<*const serde_yaml::Value, String> {
    let lines: Vec<&str> = content.lines().collect();
    let indent = |l: &str| l.len() - l.trim_start().len();
    let marker = |comment: &str| {
        crate::tokens::workflow_marker_reason(comment, crate::tokens::ADVISORY_MARKER)
    };
    // `uses:` lines of the text: value -> (line, key column, whether the line opens the step).
    let mut text_uses: BTreeMap<String, Vec<(usize, usize, bool)>> = BTreeMap::new();
    for (i, line) in lines.iter().enumerate() {
        let mut rest = line.trim_start();
        let mut col = indent(line);
        let opens = rest.starts_with("- ");
        if opens {
            let after = rest[1..].trim_start();
            col += rest.len() - after.len();
            rest = after;
        }
        let Some(value) = rest.strip_prefix("uses:") else {
            continue;
        };
        let value = value.trim_start();
        let value = match value.chars().next() {
            Some(q @ ('"' | '\'')) => value[1..].split(q).next().unwrap_or(""),
            _ => value.split([' ', '\t']).next().unwrap_or(""),
        };
        text_uses
            .entry(value.to_string())
            .or_default()
            .push((i, col, opens));
    }
    let mut parsed_uses: BTreeMap<String, Vec<&serde_yaml::Value>> = BTreeMap::new();
    for (_, job) in jobs {
        for step in job
            .get("steps")
            .and_then(|s| s.as_sequence())
            .into_iter()
            .flatten()
        {
            if let Some(uses) = step.get("uses").and_then(|u| u.as_str()) {
                parsed_uses.entry(uses.to_string()).or_default().push(step);
            }
        }
    }
    let mut out = BTreeMap::new();
    for (value, steps) in parsed_uses {
        let Some(at) = text_uses.get(&value).filter(|t| t.len() == steps.len()) else {
            continue;
        };
        for (step, &(i, col, opens)) in steps.into_iter().zip(at) {
            // A trailing comment on the `uses:` line.
            let trailing = lines[i].find(" #").and_then(|p| marker(&lines[i][p + 2..]));
            // The line that opens the step: this one, or the `- ` item line above it
            // whose key sits in the same column.
            let mut start = opens.then_some(i);
            let mut j = i;
            while start.is_none() && j > 0 {
                j -= 1;
                let l = lines[j];
                let t = l.trim_start();
                if t.is_empty() || t.starts_with('#') {
                    continue;
                }
                if let Some(after) = t.strip_prefix('-') {
                    if indent(l) + 1 + (after.len() - after.trim_start().len()) == col {
                        start = Some(j);
                    }
                }
                if indent(l) < col {
                    break;
                }
            }
            // The comment block directly above the step, with no blank line between.
            let above = start.and_then(|s| {
                lines[..s]
                    .iter()
                    .rev()
                    .map(|l| l.trim_start())
                    .take_while(|t| t.starts_with('#'))
                    .find_map(|t| marker(&t[1..]))
            });
            if let Some(reason) = trailing.or(above) {
                out.insert(step as *const serde_yaml::Value, reason);
            }
        }
    }
    out
}

/// Why a job that runs discipline cannot fail.
struct NonBlocking {
    why: String,
    /// Set when every discipline step that cannot fail is an `advisory: true` action
    /// step marked `# discipline:advisory <reason>`: the first such reason. The job
    /// still enforces nothing; the marker only says the advisory run is on purpose.
    intended: Option<String>,
}

const ADVISORY_STEP: &str = "the action runs with `advisory: true`";

/// Why a job that runs discipline cannot fail, if it cannot.
fn nonblocking_reason(
    job: &serde_yaml::Value,
    self_action: bool,
    markers: &BTreeMap<*const serde_yaml::Value, String>,
) -> Option<NonBlocking> {
    let truthy = |v: Option<&serde_yaml::Value>| match v {
        Some(serde_yaml::Value::Bool(b)) => *b,
        Some(serde_yaml::Value::String(s)) => s.trim() == "true",
        _ => false,
    };
    if truthy(job.get("continue-on-error")) {
        return Some(NonBlocking {
            why: "the job has `continue-on-error: true`".to_string(),
            intended: None,
        });
    }
    let steps: Vec<&serde_yaml::Value> = job
        .get("steps")
        .and_then(|s| s.as_sequence())
        .map(|s| s.iter().collect())
        .unwrap_or_default();
    // The job can fail on discipline if any discipline step can: one that is not masked,
    // or a masked canary whose outcome a later step checks (`steps.<id>.outcome`).
    let mut first_reason = None;
    let mut first_intended: Option<&String> = None;
    for (i, step) in steps.iter().enumerate() {
        if !step_runs_discipline(step, self_action) {
            continue;
        }
        let reason = if truthy(step.get("continue-on-error")) {
            Some("its discipline step has `continue-on-error: true`")
        } else if truthy(step.get("with").and_then(|w| w.get("advisory"))) {
            Some(ADVISORY_STEP)
        } else if step.get("run").and_then(|r| r.as_str()).is_some_and(|run| {
            run.lines().any(|l| {
                let l = l.trim();
                (l.contains("discipline check") || l.contains("discipline diff"))
                    && (l.contains("|| true") || l.contains("|| :") || l.contains("--advisory"))
            })
        }) {
            Some("its `discipline check` exit status is masked")
        } else {
            None
        };
        let reason = reason?;
        let checked_later = step.get("id").and_then(|v| v.as_str()).is_some_and(|id| {
            steps[i + 1..].iter().any(|later| {
                let text = serde_yaml::to_string(later).unwrap_or_default();
                text.contains(&format!("steps.{id}.outcome"))
                    || text.contains(&format!("steps.{id}.conclusion"))
                    || text.contains(&format!("steps.{id}.outputs.status"))
            })
        });
        if checked_later {
            return None;
        }
        if reason == ADVISORY_STEP {
            if let Some(why) = markers.get(&(*step as *const serde_yaml::Value)) {
                first_intended.get_or_insert(why);
                continue;
            }
        }
        first_reason.get_or_insert(reason);
    }
    match (first_reason, first_intended) {
        (Some(why), _) => Some(NonBlocking {
            why: why.to_string(),
            intended: None,
        }),
        (None, Some(reason)) => Some(NonBlocking {
            why: ADVISORY_STEP.to_string(),
            intended: Some(reason.clone()),
        }),
        (None, None) => None,
    }
}

/// `on:` of a workflow as a map from event name to its configuration.
/// Whether an `if:` keeps a job or step off push events.
fn restricted_to_pull_requests(v: Option<&serde_yaml::Value>) -> bool {
    v.and_then(|i| i.as_str()).is_some_and(|i| {
        let l = i.to_ascii_lowercase();
        l.contains("pull_request") && !l.contains("push")
    })
}

/// The branches a `push` event runs this discipline job on (see `DisciplineJob`).
fn push_branches(
    events: &BTreeMap<String, serde_yaml::Value>,
    job: &serde_yaml::Value,
    self_action: bool,
) -> Option<Vec<String>> {
    let cfg = events.get("push")?;
    if restricted_to_pull_requests(job.get("if")) {
        return None;
    }
    let steps = job.get("steps").and_then(|s| s.as_sequence())?;
    let discipline_steps_run_on_push = steps
        .iter()
        .filter(|st| step_runs_discipline(st, self_action))
        .any(|st| !restricted_to_pull_requests(st.get("if")));
    if !discipline_steps_run_on_push {
        return None;
    }
    let branches = cfg
        .get("branches")
        .and_then(|b| b.as_sequence())
        .map(|b| {
            b.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Some(branches)
}

/// Whether a push-trigger branch list (globs as written) covers `branch`.
pub fn push_covers(branches: &[String], branch: &str) -> bool {
    branches.is_empty()
        || branches
            .iter()
            .any(|g| g == branch || crate::doctor_settings::glob_matches(g, branch, false))
}

fn triggers(wf: &serde_yaml::Value) -> BTreeMap<String, serde_yaml::Value> {
    // YAML 1.1 reads a bare `on` key as boolean true.
    let on = wf
        .get("on")
        .or_else(|| {
            wf.as_mapping()
                .and_then(|m| m.get(serde_yaml::Value::Bool(true)))
        })
        .cloned()
        .unwrap_or(serde_yaml::Value::Null);
    let mut out = BTreeMap::new();
    match on {
        serde_yaml::Value::String(s) => {
            out.insert(s, serde_yaml::Value::Null);
        }
        serde_yaml::Value::Sequence(seq) => {
            for v in seq {
                if let Some(s) = v.as_str() {
                    out.insert(s.to_string(), serde_yaml::Value::Null);
                }
            }
        }
        serde_yaml::Value::Mapping(m) => {
            for (k, v) in m {
                if let Some(s) = k.as_str() {
                    out.insert(s.to_string(), v);
                }
            }
        }
        _ => {}
    }
    out
}

/// Token permission of a job: 0 none, 1 read-only, 2 write (or unset, which inherits the
/// repository default and may be write).
fn permission_level(value: Option<&serde_yaml::Value>) -> Option<u8> {
    let v = value?;
    Some(match v {
        serde_yaml::Value::String(s) if s == "write-all" => 2,
        serde_yaml::Value::String(s) if s == "read-all" => 1,
        serde_yaml::Value::Mapping(m) if m.is_empty() => 0,
        serde_yaml::Value::Mapping(m) => {
            if m.values().any(|v| v.as_str() == Some("write")) {
                2
            } else {
                1
            }
        }
        _ => 2,
    })
}

/// Whether a `permissions:` value lets the token read pull requests.
fn grants_pull_requests_read(value: Option<&serde_yaml::Value>) -> bool {
    match value {
        Some(serde_yaml::Value::String(s)) => s == "read-all" || s == "write-all",
        Some(serde_yaml::Value::Mapping(m)) => m
            .get("pull-requests")
            .and_then(|v| v.as_str())
            .is_some_and(|v| v == "read" || v == "write"),
        _ => false,
    }
}

/// Analyse workflow files: `(path, content)` pairs. `self_action` is true when the
/// repository is discipline itself (a `uses: ./` step runs it).
pub fn analyse_workflows(files: &[(String, String)], self_action: bool) -> LocalFacts {
    let mut facts = LocalFacts::default();
    let mut unparsable = Vec::new();
    let mut pr_workflows = 0usize;
    for (path, content) in files {
        let wf: serde_yaml::Value = match serde_yaml::from_str(content) {
            Ok(v) => v,
            Err(_) => {
                unparsable.push(path.clone());
                continue;
            }
        };
        let Some(jobs) = wf.get("jobs").and_then(|j| j.as_mapping()) else {
            continue;
        };
        let jobs: Vec<(String, &serde_yaml::Value)> = jobs
            .iter()
            .filter_map(|(k, v)| k.as_str().map(|k| (k.to_string(), v)))
            .collect();
        let runs: BTreeSet<String> = jobs
            .iter()
            .filter(|(_, job)| {
                job.get("steps")
                    .and_then(|s| s.as_sequence())
                    .is_some_and(|steps| steps.iter().any(|s| step_runs_discipline(s, self_action)))
            })
            .map(|(id, _)| id.clone())
            .collect();
        if runs.is_empty() {
            continue;
        }

        let events = triggers(&wf);
        let markers = advisory_markers(content, &jobs);
        let wf_perm = permission_level(wf.get("permissions"));
        // Worst token level across this workflow's discipline jobs (None = inherited).
        let mut worst: Option<Option<u8>> = None;
        for (id, job) in &jobs {
            if !runs.contains(id) {
                continue;
            }
            let mut rollups = Vec::new();
            let mut weak_rollups = Vec::new();
            for (other, oj) in &jobs {
                if other == id {
                    continue;
                }
                if job_needs(oj).contains(id) {
                    if rollup_enforces(oj) {
                        rollups.push(job_context(other, oj));
                    } else {
                        weak_rollups.push(job_context(other, oj));
                    }
                } else if depends_transitively(&jobs, other, id) {
                    weak_rollups.push(job_context(other, oj));
                }
            }
            let nonblocking = nonblocking_reason(job, self_action, &markers);
            match &nonblocking {
                Some(NonBlocking {
                    intended: Some(reason),
                    ..
                }) => facts.findings.push(Finding::new(
                    "non-blocking",
                    Status::Info,
                    format!(
                        "{path} job `{id}` runs discipline with `advisory: true` on purpose \
                         (`# {} {reason}`); it cannot fail, so it enforces nothing",
                        crate::tokens::ADVISORY_MARKER
                    ),
                )),
                Some(NonBlocking { why, .. }) => facts.findings.push(
                    Finding::new(
                        "non-blocking",
                        Status::Fail,
                        format!("{path} job `{id}` runs discipline but cannot fail: {why}"),
                    )
                    .fix("Remove continue-on-error, `|| true` and `advisory: true` from the discipline job. A deliberate shadow step keeps `advisory: true` with `# discipline:advisory <reason>` on the line above it."),
                ),
                None => {}
            }
            let mut workflow_names = Vec::new();
            if let Some(n) = wf.get("name").and_then(|n| n.as_str()) {
                workflow_names.push(n.trim().to_string());
            }
            if let Some(file) = Path::new(path).file_name().and_then(|f| f.to_str()) {
                workflow_names.push(file.to_string());
            }
            facts.jobs.push(DisciplineJob {
                workflow: path.clone(),
                job_id: id.clone(),
                push_branches: push_branches(&events, job, self_action),
                context: job_context(id, job),
                rollups,
                weak_rollups,
                workflow_names,
                allow_failure: nonblocking.is_some(),
                // A job's `permissions:` replaces the workflow's, it does not add to it.
                reads_pull_requests: grants_pull_requests_read(
                    job.get("permissions").or(wf.get("permissions")),
                ),
            });
            let level = permission_level(job.get("permissions")).or(wf_perm);
            let rank = |l: Option<u8>| l.unwrap_or(3);
            if worst.is_none_or(|w| rank(level) > rank(w)) {
                worst = Some(level);
            }
        }

        match events.get("pull_request") {
            Some(cfg) => {
                pr_workflows += 1;
                let types: Vec<&str> = cfg
                    .get("types")
                    .and_then(|t| t.as_sequence())
                    .map(|s| s.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();
                facts.findings.push(if types.contains(&"edited") {
                    Finding::new(
                        "trigger",
                        Status::Pass,
                        format!("{path} runs on pull requests, including description edits"),
                    )
                } else {
                    Finding::new(
                        "trigger",
                        Status::Warn,
                        format!("{path} does not re-run when a PR description changes"),
                    )
                    .fix("Use `pull_request: types: [opened, synchronize, reopened, edited]` so adding or removing a directive re-runs the gate.")
                });
            }
            None if events.contains_key("pull_request_target") => {
                pr_workflows += 1;
                facts.findings.push(
                    Finding::new(
                        "trigger",
                        Status::Warn,
                        format!("{path} runs discipline on `pull_request_target`, which executes with a write token"),
                    )
                    .fix("Run discipline on `pull_request`; it needs only `contents: read`."),
                );
            }
            None => {}
        }

        if let Some(level) = worst {
            facts.findings.push(match level {
                Some(0 | 1) => Finding::new(
                    "token",
                    Status::Pass,
                    format!("{path}: discipline jobs run with a read-only token"),
                ),
                _ => Finding::new(
                    "token",
                    Status::Warn,
                    format!(
                        "{path}: {}",
                        if level.is_none() {
                            "discipline jobs inherit the repository's default token permissions"
                        } else {
                            "a discipline job runs with a write token"
                        }
                    ),
                )
                .fix("Set `permissions: contents: read` on the workflow or job."),
            });
        }
    }
    if !facts.jobs.is_empty() && pr_workflows == 0 {
        facts.findings.push(
            Finding::new(
                "trigger",
                Status::Warn,
                "no workflow runs discipline on pull requests",
            )
            .fix("Add a `pull_request` trigger so the gate runs before merge."),
        );
    }
    for path in unparsable {
        facts.findings.push(
            Finding::new(
                "workflows",
                Status::Unknown,
                format!("{path} is not valid YAML"),
            )
            .fix("Fix the workflow file; doctor cannot tell whether it runs discipline."),
        );
    }
    if facts.jobs.is_empty() {
        facts.findings.insert(
            0,
            Finding::new("workflows", Status::Fail, "no workflow job runs discipline").fix(
                "Add a job with `uses: orieg/discipline@v0` (see docs/guides/ci-platforms.md).",
            ),
        );
    } else {
        let names: Vec<String> = facts
            .jobs
            .iter()
            .map(|j| format!("{} ({})", j.context, j.workflow))
            .collect();
        facts.findings.insert(
            0,
            Finding::new(
                "workflows",
                Status::Pass,
                format!("discipline runs in: {}", names.join(", ")),
            ),
        );
    }
    facts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::test_support::*;

    #[test]
    fn finds_discipline_jobs_and_transitive_rollups() {
        let facts = analyse_workflows(&wf(WF), false);
        assert_eq!(facts.jobs.len(), 1);
        let j = &facts.jobs[0];
        assert_eq!(j.job_id, "gate");
        assert_eq!(j.context, "Discipline");
        // `ci-gate` fails when `gate` fails; `report` is skipped and would count as passed.
        assert_eq!(j.rollups, vec!["ci-gate".to_string()]);
        assert_eq!(j.weak_rollups, vec!["report".to_string()]);
        let status = |id: &str| facts.findings.iter().find(|f| f.id == id).unwrap().status;
        assert_eq!(status("workflows"), Status::Pass);
        assert_eq!(status("trigger"), Status::Pass);
        assert_eq!(status("token"), Status::Pass);
    }

    #[test]
    fn an_install_only_step_is_not_a_discipline_job() {
        let wf = |with: &str| {
            format!(
                "on:\n  push:\njobs:\n  copilot-setup-steps:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: orieg/discipline@v0\n{with}"
            )
        };
        let jobs = |src: String| {
            analyse_workflows(
                &[(".github/workflows/copilot-setup-steps.yml".into(), src)],
                false,
            )
            .jobs
            .len()
        };
        assert_eq!(
            jobs(wf("        with:\n          install_only: 'true'\n")),
            0
        );
        assert_eq!(jobs(wf("        with:\n          install_only: true\n")), 0);
        // Checks: no `install_only`, a false one, or an expression the doctor cannot resolve.
        assert_eq!(jobs(wf("")), 1);
        assert_eq!(
            jobs(wf("        with:\n          install_only: 'false'\n")),
            1
        );
        assert_eq!(
            jobs(wf(
                "        with:\n          install_only: ${{ inputs.x }}\n"
            )),
            1
        );
    }

    #[test]
    fn push_triggers_are_read_per_job_and_step() {
        let wf = |extra_job_if: &str, step_if: &str, on: &str| {
            format!(
                "on:\n{on}jobs:\n  gate:\n{extra_job_if}    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n{step_if}        uses: orieg/discipline@v0\n"
            )
        };
        let facts =
            |src: String| analyse_workflows(&[(".github/workflows/ci.yml".into(), src)], false);
        // Push to main only.
        let f = facts(wf(
            "",
            "      - name: gate\n",
            "  pull_request:\n  push:\n    branches: [main]\n",
        ));
        assert_eq!(f.jobs[0].push_branches, Some(vec!["main".to_string()]));
        // Every branch.
        let f = facts(wf("", "      - name: gate\n", "  push:\n"));
        assert_eq!(f.jobs[0].push_branches, Some(vec![]));
        // No push trigger.
        let f = facts(wf("", "      - name: gate\n", "  pull_request:\n"));
        assert_eq!(f.jobs[0].push_branches, None);
        // The job, or the discipline step, is restricted to pull requests.
        let f = facts(wf(
            "    if: github.event_name == 'pull_request'\n",
            "      - name: gate\n",
            "  push:\n",
        ));
        assert_eq!(f.jobs[0].push_branches, None);
        let f = facts(wf(
            "",
            "      - name: gate\n        if: github.event_name == 'pull_request'\n",
            "  push:\n",
        ));
        assert_eq!(f.jobs[0].push_branches, None);
        assert!(push_covers(&[], "main"));
        assert!(push_covers(&["release/*".into()], "release/1.0"));
        assert!(!push_covers(&["develop".into()], "main"));
    }

    #[test]
    fn run_steps_and_self_action_count_but_comments_do_not() {
        let run = "on: pull_request\njobs:\n  a:\n    steps:\n      - run: |\n          # discipline check is disabled\n          echo hi\n  b:\n    steps:\n      - run: ./bin/discipline check --base origin/main\n  c:\n    steps:\n      - uses: ./\n";
        let ids: Vec<String> = analyse_workflows(&wf(run), false)
            .jobs
            .into_iter()
            .map(|j| j.job_id)
            .collect();
        assert_eq!(ids, vec!["b"]);
        let ids: Vec<String> = analyse_workflows(&wf(run), true)
            .jobs
            .into_iter()
            .map(|j| j.job_id)
            .collect();
        assert_eq!(ids, vec!["b", "c"]);
    }

    #[test]
    fn missing_edited_trigger_write_token_and_no_job_are_reported() {
        let loose = WF
            .replace("    types: [opened, synchronize, reopened, edited]\n", "")
            .replace("permissions:\n  contents: read\n", "");
        let facts = analyse_workflows(&wf(&loose), false);
        let status = |id: &str| facts.findings.iter().find(|f| f.id == id).unwrap().status;
        assert_eq!(status("trigger"), Status::Warn);
        assert_eq!(status("token"), Status::Warn);

        // An explicit `types:` list without `edited` is the same gap.
        let typed = WF.replace("reopened, edited]", "reopened]");
        let facts = analyse_workflows(&wf(&typed), false);
        let trigger = facts.findings.iter().find(|f| f.id == "trigger").unwrap();
        assert_eq!(trigger.status, Status::Warn);

        let none = analyse_workflows(
            &wf("on: push\njobs:\n  a:\n    steps: [{run: make}]\n"),
            false,
        );
        assert!(none.jobs.is_empty());
        assert_eq!(none.findings[0].status, Status::Fail);

        let broken = analyse_workflows(&wf("jobs: [unclosed"), false);
        assert!(broken.findings.iter().any(|f| f.status == Status::Unknown));
    }

    #[test]
    fn a_masked_canary_whose_outcome_is_checked_still_blocks() {
        let canary = WF.replace(
            "      - uses: orieg/discipline@v0\n",
            "      - id: bad\n        continue-on-error: true\n        uses: orieg/discipline@v0\n      - run: test \"${{ steps.bad.outcome }}\" = failure\n",
        );
        let facts = analyse_workflows(&wf(&canary), false);
        assert!(
            facts.findings.iter().all(|f| f.id != "non-blocking"),
            "{:?}",
            facts.findings
        );
        assert!(!facts.jobs[0].allow_failure);
    }

    /// `WF` with a second, advisory discipline job `shadow` that the rollup also needs.
    fn with_shadow(step: &str) -> String {
        WF.replace(
            "  report:\n",
            &format!("  shadow:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n{step}  report:\n"),
        )
        .replace("needs: [lint, gate, report]", "needs: [lint, gate, shadow, report]")
    }

    #[test]
    fn a_marked_advisory_shadow_step_is_reported_as_info() {
        for step in [
            // The comment line directly above the step.
            "      # discipline:advisory shadow of scripts/lint.sh until it is retired\n      - uses: orieg/discipline@v0\n        with:\n          advisory: true\n",
            // In a comment block above a step that opens with `name:`.
            "      # Compared on live runs before it replaces the script.\n      # discipline:advisory shadow of scripts/lint.sh until it is retired\n      - name: Shadow\n        uses: orieg/discipline@v0\n        with:\n          advisory: true\n",
            // On the `uses:` line.
            "      - name: Shadow\n        uses: orieg/discipline@v0 # discipline:advisory shadow of scripts/lint.sh until it is retired\n        with:\n          advisory: true\n",
        ] {
            let facts = analyse_workflows(&wf(&with_shadow(step)), false);
            let nb = non_blocking(&facts);
            assert_eq!(nb.len(), 1, "{step}\n{nb:?}");
            assert_eq!(nb[0].status, Status::Info, "{step}\n{nb:?}");
            assert!(nb[0].summary.contains("job `shadow`"), "{}", nb[0].summary);
            assert!(
                nb[0].summary.contains("shadow of scripts/lint.sh until it is retired"),
                "{}",
                nb[0].summary
            );
            // The marker changes the report only: the shadow job still enforces nothing.
            let shadow = facts.jobs.iter().find(|j| j.job_id == "shadow").unwrap();
            assert!(shadow.allow_failure);
            let gate = facts.jobs.iter().find(|j| j.job_id == "gate").unwrap();
            assert!(!gate.allow_failure);
        }
    }

    #[test]
    fn an_advisory_step_without_a_usable_marker_still_fails() {
        let advisory =
            "      - uses: orieg/discipline@v0\n        with:\n          advisory: true\n";
        for (label, step) in [
            ("no marker", advisory.to_string()),
            ("no reason", format!("      # discipline:advisory\n{advisory}")),
            ("placeholder", format!("      # discipline:advisory <reason>\n{advisory}")),
            ("TODO", format!("      # discipline:advisory TODO\n{advisory}")),
            (
                "blank line between",
                format!("      # discipline:advisory shadow run\n\n{advisory}"),
            ),
            (
                "not at the start of the comment",
                format!("      # see discipline:advisory shadow run\n{advisory}"),
            ),
            (
                "another step cannot fail",
                format!("      # discipline:advisory shadow run\n{advisory}      - uses: orieg/discipline@v0\n        continue-on-error: true\n"),
            ),
        ] {
            let facts = analyse_workflows(&wf(&with_shadow(&step)), false);
            let nb = non_blocking(&facts);
            assert_eq!(nb.len(), 1, "{label}: {nb:?}");
            assert_eq!(nb[0].status, Status::Fail, "{label}: {nb:?}");
        }
    }
}
