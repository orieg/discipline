//! `ci-integrity` exposure checks: workflow shapes that hand secrets or a write token to
//! code the workflow does not control.
//!
//! - an untrusted expression (`github.event.*`, `github.head_ref`, `inputs.*`)
//!   interpolated into a `run:` script (template injection);
//! - `secrets: inherit` on a reusable-workflow call;
//! - `actions/checkout` without `persist-credentials: false` in a job that can write;
//! - a job that reads `secrets.*` and runs a third-party action;
//! - a `schedule:` trigger on a workflow that reads secrets.
//!
//! Everything is read from the parsed YAML: a YAML comment is gone before these run, and
//! an expression bound in `env:` and read as `$VAR` in the script is not interpolation.
//! Expressions are split into property paths by a small lexer of GitHub's expression
//! language, not matched as substrings of the script.

use super::ci_integrity::{find_key_line, find_line_after, workflow_has_trigger};
use crate::findings::FindingKind;

/// One exposure found in a file. `key` identifies it across base and head, so an
/// exposure already on the base side is told from one the change added.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Exposure {
    pub kind: &'static FindingKind,
    pub key: String,
    pub line: Option<usize>,
    pub message: String,
    pub remediation: &'static str,
    /// What a scoped `allow-ci-weakening: <subject> <reason>` names.
    pub subject: String,
}

/// The `${{ ... }}` expressions in `text`, inner text only, in order. A `}}` inside a
/// quoted string literal does not close an expression; an unclosed one runs to the end.
pub(crate) fn expressions(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("${{") {
        let body = &rest[start + 3..];
        let mut in_str = false;
        let mut end = None;
        let bytes = body.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\'' => in_str = !in_str,
                b'}' if !in_str && bytes.get(i + 1) == Some(&b'}') => {
                    end = Some(i);
                    break;
                }
                _ => {}
            }
            i += 1;
        }
        match end {
            Some(e) => {
                out.push(body[..e].trim().to_string());
                rest = &body[e + 2..];
            }
            None => {
                out.push(body.trim().to_string());
                break;
            }
        }
    }
    out
}

/// The property paths an expression reads, lower-cased: `github.event.issue.title`,
/// `inputs.name`. Index syntax (`github['head_ref']`, `x[0]`) is read as a path segment;
/// string literals and function names are not paths.
pub(crate) fn expression_paths(expr: &str) -> Vec<Vec<String>> {
    #[derive(PartialEq)]
    enum T {
        Ident(String),
        Dot,
        Index(Option<String>),
        Other,
    }
    let chars: Vec<char> = expr.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' && chars.get(i + 1) == Some(&'\'') {
                    i += 2;
                } else if chars[i] == '\'' {
                    break;
                } else {
                    i += 1;
                }
            }
            i += 1;
            toks.push(T::Other);
        } else if c == '[' {
            // `['name']` is a segment; any other index (`[0]`, `[*]`) is an unnamed one.
            let close = chars[i..].iter().position(|&ch| ch == ']').map(|p| i + p);
            let inner: String = match close {
                Some(e) => chars[i + 1..e].iter().collect(),
                None => String::new(),
            };
            let inner = inner.trim();
            let name = inner
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
                .map(|s| s.to_ascii_lowercase());
            toks.push(T::Index(name));
            i = close.map_or(chars.len(), |e| e + 1);
        } else if c == '.' {
            toks.push(T::Dot);
            i += 1;
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '-')
            {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            // A function call is not a path.
            let mut j = i;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if chars.get(j) == Some(&'(') {
                toks.push(T::Other);
            } else {
                toks.push(T::Ident(word.to_ascii_lowercase()));
            }
        } else {
            if !c.is_whitespace() {
                toks.push(T::Other);
            }
            i += 1;
        }
    }
    let mut paths = Vec::new();
    let mut k = 0;
    while k < toks.len() {
        let rooted = k == 0 || !matches!(toks[k - 1], T::Dot);
        if let (T::Ident(root), true) = (&toks[k], rooted) {
            let mut path = vec![root.clone()];
            k += 1;
            loop {
                match (toks.get(k), toks.get(k + 1)) {
                    (Some(T::Dot), Some(T::Ident(s))) => {
                        path.push(s.clone());
                        k += 2;
                    }
                    (Some(T::Dot), Some(T::Other)) => {
                        // `.*` (object filter)
                        path.push("*".to_string());
                        k += 2;
                    }
                    (Some(T::Index(name)), _) => {
                        path.push(name.clone().unwrap_or_else(|| "*".to_string()));
                        k += 1;
                    }
                    _ => break,
                }
            }
            paths.push(path);
        } else {
            k += 1;
        }
    }
    paths
}

/// Whether a path reads a value an outside party controls: any `github.event.*`,
/// `github.head_ref`, or a workflow / action input.
pub(crate) fn untrusted_path(path: &[String]) -> bool {
    match path.first().map(String::as_str) {
        Some("github") => matches!(
            path.get(1).map(String::as_str),
            Some("event") | Some("head_ref")
        ),
        Some("inputs") => path.len() > 1,
        _ => false,
    }
}

/// The untrusted expressions of a `run:` script, as written.
pub(crate) fn untrusted_expressions(run: &str) -> Vec<String> {
    expressions(run)
        .into_iter()
        .filter(|e| expression_paths(e).iter().any(|p| untrusted_path(p)))
        .collect()
}

/// Whether the value mentions a secret: an expression reading `secrets.<name>` or
/// `secrets[...]`, anywhere in it (a mapping, a list or a string).
pub(crate) fn reads_secrets(v: &serde_yaml::Value) -> bool {
    match v {
        serde_yaml::Value::String(s) => expressions(s)
            .iter()
            .any(|e| expression_paths(e).iter().any(|p| p[0] == "secrets")),
        serde_yaml::Value::Sequence(seq) => seq.iter().any(reads_secrets),
        serde_yaml::Value::Mapping(m) => {
            m.iter().any(|(k, v)| reads_secrets(k) || reads_secrets(v))
        }
        serde_yaml::Value::Tagged(t) => reads_secrets(&t.value),
        _ => false,
    }
}

/// Whether a `permissions:` value grants any write scope. `None` when it is absent.
fn grants_write(perm: Option<&serde_yaml::Value>) -> Option<bool> {
    let perm = perm?;
    Some(match perm {
        serde_yaml::Value::String(s) => s.trim() == "write-all",
        serde_yaml::Value::Mapping(m) => m.values().any(|v| v.as_str() == Some("write")),
        _ => false,
    })
}

/// Whether the job's token can write: its own `permissions:`, else the workflow's, else
/// the repository default, which may be write and is treated so.
pub(crate) fn job_can_write(workflow: &serde_yaml::Value, job: &serde_yaml::Value) -> bool {
    grants_write(job.get("permissions"))
        .or_else(|| grants_write(workflow.get("permissions")))
        .unwrap_or(true)
}

/// The action a `uses:` names, lower-cased, without its ref: `actions/checkout`.
fn action_target(uses: &str) -> String {
    uses.trim()
        .split_once('@')
        .map_or(uses.trim(), |(t, _)| t)
        .to_ascii_lowercase()
}

/// Whether a step runs `actions/checkout` without `persist-credentials: false`.
pub(crate) fn checkout_persists(step: &serde_yaml::Value) -> bool {
    let Some(uses) = step.get("uses").and_then(|u| u.as_str()) else {
        return false;
    };
    if action_target(uses) != "actions/checkout" {
        return false;
    }
    let persist = step.get("with").and_then(|w| w.get("persist-credentials"));
    !match persist {
        Some(serde_yaml::Value::Bool(b)) => !*b,
        Some(serde_yaml::Value::String(s)) => s.trim().eq_ignore_ascii_case("false"),
        _ => false,
    }
}

/// Owners whose actions GitHub publishes and runs; not third-party for the secrets rule.
const GITHUB_OWNED: &[&str] = &["actions/", "github/"];

/// Whether a step `uses:` a remote action published outside GitHub's own owners and the
/// repository's `first_party_action_prefixes`. Local and `docker://` steps are not
/// third-party actions; an expression is not judged.
pub(crate) fn third_party_action(uses: &str, first_party: &[String]) -> bool {
    let u = uses.trim();
    if u.starts_with("./") || u.starts_with("docker://") || u.contains("${{") {
        return false;
    }
    let target = action_target(u);
    !GITHUB_OWNED.iter().any(|p| target.starts_with(p))
        && !first_party
            .iter()
            .any(|p| target.starts_with(&p.to_ascii_lowercase()))
}

fn step_name(step: &serde_yaml::Value) -> String {
    step.get("name")
        .or_else(|| step.get("id"))
        .or_else(|| step.get("uses"))
        .and_then(|v| v.as_str())
        .unwrap_or("unnamed step")
        .to_string()
}

/// Every exposure of a workflow file, located in `content`.
pub(crate) fn workflow_exposures(
    doc: &serde_yaml::Value,
    content: &str,
    first_party: &[String],
) -> Vec<Exposure> {
    let line_of = |needle: &str, from: usize| find_line_after(content, needle, from);
    let jobs_line = find_key_line(content, "jobs", 1).unwrap_or(1);
    let job_line = |job: &str| find_key_line(content, job, jobs_line);
    let mut out = Vec::new();
    let Some(jobs) = doc.get("jobs").and_then(|j| j.as_mapping()) else {
        return out;
    };
    let workflow_reads_secrets = reads_secrets(doc);
    if workflow_reads_secrets && workflow_has_trigger(doc, "schedule") {
        out.push(Exposure {
            kind: &crate::findings::SCHEDULE_TRIGGER_WITH_SECRETS,
            key: "schedule".to_string(),
            line: find_key_line(content, "schedule", 1).or_else(|| line_of("schedule", 1)),
            message: "Workflow reads secrets and runs on a `schedule:` trigger: a scheduled run uses the default branch's workflow and whatever its actions resolve to that day, with no pull request to review it.".to_string(),
            remediation: "Keep secrets out of scheduled workflows, or pin every action by commit SHA and excuse with allow-ci-weakening: schedule <reason>.",
            subject: "schedule".to_string(),
        });
    }
    for (job_k, job) in jobs {
        let job_id = job_k.as_str().unwrap_or("");
        let jl = job_line(job_id).unwrap_or(1);
        if job
            .get("secrets")
            .and_then(|s| s.as_str())
            .is_some_and(|s| s.trim() == "inherit")
        {
            out.push(Exposure {
                kind: &crate::findings::SECRETS_INHERIT,
                key: job_id.to_string(),
                line: find_key_line(content, "secrets", jl).or(Some(jl)),
                message: format!("Job '{job_id}' passes every secret of the caller to the reusable workflow ('secrets: inherit')."),
                remediation: "Pass only the secrets the called workflow needs (`secrets: { name: ${{ secrets.name }} }`), or excuse with allow-ci-weakening: secrets-inherit <reason>.",
                subject: "secrets-inherit".to_string(),
            });
        }
        let Some(steps) = job.get("steps").and_then(|s| s.as_sequence()) else {
            continue;
        };
        let can_write = job_can_write(doc, job);
        let job_secrets = reads_secrets(job);
        let mut cursor = jl;
        for step in steps {
            let name = step_name(step);
            let anchor = step
                .get("uses")
                .or_else(|| step.get("run"))
                .and_then(|v| v.as_str())
                .and_then(|v| v.lines().next().map(str::to_string));
            let sl = anchor
                .as_deref()
                .and_then(|a| line_of(a.trim(), cursor))
                .unwrap_or(cursor);
            cursor = sl + 1;
            if let Some(run) = step.get("run").and_then(|r| r.as_str()) {
                for e in untrusted_expressions(run) {
                    out.push(Exposure {
                        kind: &crate::findings::TEMPLATE_INJECTION,
                        key: format!("{job_id}\u{1f}{e}"),
                        line: line_of(&e, sl).or(Some(sl)),
                        message: format!("Step '{name}' in job '{job_id}' interpolates '${{{{ {e} }}}}' into its run: script; the runner substitutes the value before the shell parses it, so a crafted value runs as code."),
                        remediation: "Bind the value in the step's env: and read it as a quoted shell variable (\"$VALUE\"), or excuse with allow-ci-weakening: template-injection <reason>.",
                        subject: "template-injection".to_string(),
                    });
                }
            }
            if can_write && checkout_persists(step) {
                out.push(Exposure {
                    kind: &crate::findings::CHECKOUT_PERSISTS_CREDENTIALS,
                    key: job_id.to_string(),
                    line: Some(sl),
                    message: format!("Job '{job_id}' can write and checks out without 'persist-credentials: false': the token stays in .git/config for every later step to read."),
                    remediation: "Add `with: persist-credentials: false` to the checkout step, or excuse with allow-ci-weakening: persist-credentials <reason>.",
                    subject: "persist-credentials".to_string(),
                });
            }
            if job_secrets {
                if let Some(uses) = step.get("uses").and_then(|u| u.as_str()) {
                    if third_party_action(uses, first_party) {
                        let target = action_target(uses);
                        out.push(Exposure {
                            kind: &crate::findings::SECRETS_WITH_THIRD_PARTY_ACTION,
                            key: format!("{job_id}\u{1f}{target}"),
                            line: Some(sl),
                            message: format!("Job '{job_id}' reads secrets and runs the third-party action '{target}': the action runs in the same job, where the secrets are."),
                            remediation: "Move the secret-reading steps to a job without third-party actions, or excuse with allow-ci-weakening: <action> <reason>.",
                            subject: target,
                        });
                    }
                }
            }
        }
    }
    out
}

/// The exposures of a composite action's metadata file: template injection in its
/// nested `run:` steps (its `inputs.*` come from the calling workflow).
pub(crate) fn action_exposures(doc: &serde_yaml::Value, content: &str) -> Vec<Exposure> {
    let line_of = |needle: &str, from: usize| find_line_after(content, needle, from);
    let mut out = Vec::new();
    let Some(steps) = doc
        .get("runs")
        .and_then(|r| r.get("steps"))
        .and_then(|s| s.as_sequence())
    else {
        return out;
    };
    let mut cursor = find_key_line(content, "runs", 1).unwrap_or(1);
    for step in steps {
        let Some(run) = step.get("run").and_then(|r| r.as_str()) else {
            continue;
        };
        let name = step_name(step);
        let sl = run
            .lines()
            .next()
            .and_then(|l| line_of(l.trim(), cursor))
            .unwrap_or(cursor);
        cursor = sl + 1;
        for e in untrusted_expressions(run) {
            out.push(Exposure {
                kind: &crate::findings::TEMPLATE_INJECTION,
                key: format!("composite\u{1f}{e}"),
                line: line_of(&e, sl).or(Some(sl)),
                message: format!("Composite step '{name}' interpolates '${{{{ {e} }}}}' into its run: script; the runner substitutes the value before the shell parses it, so a crafted value runs as code."),
                remediation: "Bind the value in the step's env: and read it as a quoted shell variable (\"$VALUE\"), or excuse with allow-ci-weakening: template-injection <reason>.",
                subject: "template-injection".to_string(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(wf: &str) -> Vec<&'static str> {
        let doc: serde_yaml::Value = serde_yaml::from_str(wf).unwrap();
        let mut c: Vec<_> = workflow_exposures(&doc, wf, &[])
            .into_iter()
            .map(|x| x.kind.code)
            .collect();
        c.sort();
        c
    }

    #[test]
    fn untrusted_expressions_are_read_from_the_expression_not_the_script() {
        // Positive controls.
        for run in [
            "echo \"${{ github.event.issue.title }}\"",
            "git checkout ${{ github.head_ref }}",
            "./build ${{ inputs.target }}",
            "echo ${{ github['head_ref'] }}",
            "echo ${{ github.event['pull_request'].body }}",
            "echo ${{ format('{0}', github.event.comment.body) }}",
            "echo ${{ toJSON(github.event) }}",
            "echo ${{ github . head_ref }}",
        ] {
            assert_eq!(untrusted_expressions(run).len(), 1, "{run}");
        }
        // Negative controls.
        for run in [
            "echo ${{ github.sha }}",
            "echo ${{ github.event_name }}",
            "echo ${{ steps.x.outputs.inputs }}",
            "echo ${{ matrix.os }} ${{ env.TITLE }} ${{ secrets.TOKEN }}",
            "echo ${{ 'github.event.issue.title' }}",
            "echo \"$TITLE\" # github.event.issue.title",
            "echo ${{ inputs }}",
            // A property of a parsed output is not the `inputs` context.
            "echo ${{ fromJSON(steps.meta.outputs.json).inputs.name }}",
        ] {
            assert!(untrusted_expressions(run).is_empty(), "{run}");
        }
        assert_eq!(
            expressions("a ${{ x }} b ${{ '}}' }} c ${{ y"),
            vec!["x", "'}}'", "y"]
        );
    }

    #[test]
    fn a_value_bound_in_env_is_not_injection() {
        let wf = "on: issues\npermissions: read-all\njobs:\n  t:\n    runs-on: x\n    steps:\n      - env:\n          TITLE: ${{ github.event.issue.title }}\n        run: echo \"$TITLE\"\n      # run: echo ${{ github.event.issue.title }}\n";
        assert!(codes(wf).is_empty(), "{:?}", codes(wf));
        let wf = wf.replace(
            "run: echo \"$TITLE\"",
            "run: echo ${{ github.event.issue.title }}",
        );
        assert_eq!(codes(&wf), vec!["template-injection"]);
    }

    #[test]
    fn secrets_inherit_is_reported_and_named_secrets_are_not() {
        let base = "on: push\npermissions: read-all\njobs:\n  call:\n    uses: ./.github/workflows/r.yml\n";
        assert!(codes(base).is_empty());
        assert_eq!(
            codes(&format!("{base}    secrets: inherit\n")),
            vec!["secrets-inherit"]
        );
        assert!(codes(&format!(
            "{base}    secrets:\n      token: ${{{{ secrets.T }}}}\n"
        ))
        .is_empty());
    }

    #[test]
    fn checkout_persists_credentials_only_where_the_token_can_write() {
        let wf = |perm: &str, with: &str| {
            format!("on: push\n{perm}jobs:\n  t:\n    runs-on: x\n    steps:\n      - uses: actions/checkout@v4\n{with}")
        };
        let off = "        with:\n          persist-credentials: false\n";
        let write = "permissions:\n  contents: write\n";
        assert_eq!(codes(&wf(write, "")), vec!["checkout-persists-credentials"]);
        assert_eq!(
            codes(&wf("permissions: write-all\n", "")),
            vec!["checkout-persists-credentials"]
        );
        // No permissions block: the repository default may write.
        assert_eq!(codes(&wf("", "")), vec!["checkout-persists-credentials"]);
        assert!(codes(&wf(write, off)).is_empty());
        assert!(codes(&wf(
            write,
            "        with:\n          persist-credentials: 'false'\n"
        ))
        .is_empty());
        assert!(codes(&wf("permissions:\n  contents: read\n", "")).is_empty());
        assert!(codes(&wf("permissions: read-all\n", "")).is_empty());
        // A job-level read-only block overrides a write workflow.
        let job_read = "on: push\npermissions: write-all\njobs:\n  t:\n    permissions:\n      contents: read\n    runs-on: x\n    steps:\n      - uses: actions/checkout@v4\n";
        assert!(codes(job_read).is_empty());
    }

    #[test]
    fn a_job_reading_secrets_with_a_third_party_action_is_reported() {
        let job = |steps: &str| {
            format!(
                "on: push\npermissions: read-all\njobs:\n  t:\n    runs-on: x\n    steps:\n{steps}"
            )
        };
        let secret =
            "      - run: ./deploy\n        env:\n          TOKEN: ${{ secrets.DEPLOY }}\n";
        let third = "      - uses: evil/uploader@b4ffde65f46336ab88eb53be808477a3936bae11\n";
        let github =
            "      - uses: actions/upload-artifact@b4ffde65f46336ab88eb53be808477a3936bae11\n";
        assert_eq!(
            codes(&job(&format!("{secret}{third}"))),
            vec!["secrets-with-third-party-action"]
        );
        assert!(codes(&job(&format!("{secret}{github}"))).is_empty());
        assert!(codes(&job(third)).is_empty());
        assert!(codes(&job(&format!("{secret}      - uses: ./local\n"))).is_empty());
        // Listed first-party prefixes are not third-party.
        let wf = job(&format!("{secret}{third}"));
        let doc: serde_yaml::Value = serde_yaml::from_str(&wf).unwrap();
        assert!(workflow_exposures(&doc, &wf, &["evil/".to_string()]).is_empty());
    }

    #[test]
    fn a_schedule_is_reported_only_on_a_workflow_that_reads_secrets() {
        let wf = |on: &str, env: &str| {
            format!("on:\n{on}permissions: read-all\njobs:\n  t:\n    runs-on: x\n    steps:\n      - run: ./x\n{env}")
        };
        let sched = "  schedule:\n    - cron: '0 0 * * *'\n";
        let secret = "        env:\n          T: ${{ secrets.T }}\n";
        assert_eq!(
            codes(&wf(sched, secret)),
            vec!["schedule-trigger-with-secrets"]
        );
        assert!(codes(&wf(sched, "")).is_empty());
        assert!(codes(&wf("  push:\n", secret)).is_empty());
    }

    #[test]
    fn composite_action_run_steps_are_checked_for_injection() {
        let a = "runs:\n  using: composite\n  steps:\n    - run: echo ${{ inputs.name }}\n      shell: bash\n    - run: echo \"$NAME\"\n      shell: bash\n      env:\n        NAME: ${{ inputs.name }}\n";
        let doc: serde_yaml::Value = serde_yaml::from_str(a).unwrap();
        let x = action_exposures(&doc, a);
        assert_eq!(x.len(), 1, "{x:?}");
        assert_eq!(x[0].kind.code, "template-injection");
        assert_eq!(x[0].line, Some(4));
    }
}
