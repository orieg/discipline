//! What the branch rules mean for discipline: whether a required check runs it and
//! whether the rules around that check can be bypassed.

use super::{DisciplineJob, Finding, Protection, Status};
use crate::forge::ForgeKind;
use std::collections::BTreeSet;

/// Status-check names a forge reports for the jobs that run discipline and their rollups.
/// With `enforcing`, only blocking discipline jobs and rollups that fail with them;
/// otherwise the names that look related but would not block a merge.
pub fn candidate_contexts(kind: ForgeKind, jobs: &[DisciplineJob], enforcing: bool) -> Vec<String> {
    let mut out = BTreeSet::new();
    for j in jobs {
        let names: Vec<&str> = if enforcing {
            if j.allow_failure {
                continue;
            }
            std::iter::once(j.context.as_str())
                .chain(j.rollups.iter().map(|s| s.as_str()))
                .collect()
        } else {
            let own = j.allow_failure.then_some(j.context.as_str());
            own.into_iter()
                .chain(j.weak_rollups.iter().map(|s| s.as_str()))
                .chain(if j.allow_failure {
                    j.rollups.iter().map(|s| s.as_str()).collect()
                } else {
                    Vec::new()
                })
                .collect()
        };
        match kind {
            ForgeKind::Gitea | ForgeKind::Forgejo => {
                // "<workflow display name> / <job name> (<event>)", as both forges format it.
                for wf in &j.workflow_names {
                    for job in &names {
                        for event in ["pull_request", "pull_request_target", "push"] {
                            out.insert(format!("{wf} / {job} ({event})"));
                        }
                    }
                }
            }
            _ => out.extend(names.iter().map(|s| s.to_string())),
        }
    }
    out.into_iter().collect()
}

fn context_matches(required: &str, candidate: &str, patterns: bool) -> bool {
    if !patterns {
        return required == candidate;
    }
    required == candidate || crate::doctor_settings::glob_matches(required, candidate, false)
}

/// A Gitea / Forgejo protected file pattern as the forge applies it (#428): lowercased and
/// compiled by gobwas/glob with `.` and `/` both separators, so `*` and `?` stop at either
/// and `.gitea/workflows/*` does not match `ci.yml`; `**` crosses both. `[...]` / `[!...]`
/// classes and `{a,b}` alternatives keep their meaning. The caller lowercases the path. A
/// pattern the forge could not compile protects nothing (the forge skips it): `None`.
fn forge_file_glob(pattern: &str) -> Option<regex::Regex> {
    let mut re = String::from("^");
    let lowered = pattern.trim().to_lowercase();
    let mut chars = lowered.chars().peekable();
    let mut braces = 0usize;
    while let Some(c) = chars.next() {
        match c {
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                re.push_str(".*");
            }
            '*' => re.push_str("[^./]*"),
            '?' => re.push_str("[^./]"),
            '[' => {
                re.push('[');
                if chars.peek() == Some(&'!') {
                    chars.next();
                    re.push('^');
                }
                loop {
                    match chars.next()? {
                        ']' => break,
                        d @ ('\\' | '[' | '^' | '&' | '~') => {
                            re.push('\\');
                            re.push(d);
                        }
                        d => re.push(d),
                    }
                }
                re.push(']');
            }
            '{' => {
                braces += 1;
                re.push_str("(?:");
            }
            '}' if braces > 0 => {
                braces -= 1;
                re.push(')');
            }
            ',' if braces > 0 => re.push('|'),
            '\\' => re.push_str(&regex::escape(&chars.next()?.to_string())),
            c => re.push_str(&regex::escape(&c.to_string())),
        }
    }
    if braces > 0 {
        return None;
    }
    re.push('$');
    regex::Regex::new(&re).ok()
}

/// Findings for a branch's protection against the jobs that run discipline.
pub fn protection_findings(
    kind: ForgeKind,
    p: &Protection,
    jobs: &[DisciplineJob],
) -> Vec<Finding> {
    let mut out = Vec::new();
    let hidden = |id: &'static str| {
        Finding::new(id, Status::Warn, "not visible to this token")
            .fix("Re-run with a token that has admin access to the repository.")
    };

    if let Some(gate) = p.pipeline_must_succeed {
        // GitLab: the pipeline is the check; a discipline job that cannot fail it gates nothing.
        let blocking: Vec<&str> = jobs
            .iter()
            .filter(|j| !j.allow_failure)
            .map(|j| j.job_id.as_str())
            .collect();
        out.push(match gate {
            None => Finding::new(
                "required-check",
                Status::Unknown,
                "whether merges require a successful pipeline is not visible to this token",
            )
            .fix("Set GITLAB_TOKEN (or DISCIPLINE_FORGE_TOKEN) with Maintainer access and re-run."),
            Some(false) => Finding::new(
                "required-check",
                Status::Fail,
                "merge requests can merge with a failed pipeline",
            )
            .fix("Enable Settings → Merge requests → Pipelines must succeed."),
            Some(true) if blocking.is_empty() => Finding::new(
                "required-check",
                Status::Fail,
                "pipelines must succeed, but no blocking job runs discipline",
            )
            .fix("Run the discipline job without allow_failure."),
            Some(true) => Finding::new(
                "required-check",
                Status::Pass,
                format!(
                    "merges require a successful pipeline, which runs {}",
                    blocking.join(", ")
                ),
            ),
        });
    } else {
        let candidates = candidate_contexts(kind, jobs, true);
        let weak = candidate_contexts(kind, jobs, false);
        let enforcing: Vec<&String> = p
            .required_contexts
            .iter()
            .filter(|r| {
                candidates
                    .iter()
                    .any(|c| context_matches(r, c, p.context_patterns))
            })
            .collect();
        if p.required_contexts.is_empty() {
            out.push(
                Finding::new("required-check", Status::Fail, "no status check is required before merge")
                    .fix("Require the rollup job (or the discipline job) as a status check on the branch."),
            );
        } else if enforcing.is_empty()
            && p.required_contexts.iter().any(|r| {
                weak.iter()
                    .any(|c| context_matches(r, c, p.context_patterns))
            })
        {
            let named: Vec<&str> = p
                .required_contexts
                .iter()
                .filter(|r| {
                    weak.iter()
                        .any(|c| context_matches(r, c, p.context_patterns))
                })
                .map(|s| s.as_str())
                .collect();
            out.push(
                Finding::new(
                    "required-check",
                    Status::Fail,
                    format!(
                        "required check {} depends on discipline but passes (or is skipped) when discipline fails",
                        named.join(", ")
                    ),
                )
                .fix("Give the rollup `if: always()`, list the discipline job in its `needs:` directly, and fail a step when any `needs.*.result` is not success; or require the discipline job itself."),
            );
        } else if enforcing.is_empty() {
            let shown: Vec<&str> = candidates
                .iter()
                .filter(|c| !c.ends_with("(push)") && !c.ends_with("(pull_request_target)"))
                .map(|s| s.as_str())
                .collect();
            out.push(
                Finding::new(
                    "required-check",
                    Status::Fail,
                    format!(
                        "required checks ({}) do not include a job that runs discipline",
                        p.required_contexts
                            .iter()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
                .fix(if shown.is_empty() {
                    "Add a discipline job, then require it or a rollup that needs it.".to_string()
                } else {
                    format!("Require one of: {}.", shown.join(", "))
                }),
            );
        } else {
            out.push(Finding::new(
                "required-check",
                Status::Pass,
                format!(
                    "merge requires {}, which runs discipline",
                    enforcing
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
    }

    out.push(if p.hidden.contains("up-to-date") {
        hidden("up-to-date")
    } else if p.strict {
        Finding::new(
            "up-to-date",
            Status::Pass,
            "branches must be up to date before merge",
        )
    } else {
        Finding::new(
            "up-to-date",
            Status::Warn,
            "a branch can merge without being up to date",
        )
        .fix(match kind {
            ForgeKind::GitLab => {
                "Use fast-forward or semi-linear merges, or merged results pipelines."
            }
            ForgeKind::Gitea | ForgeKind::Forgejo => {
                "Enable \"Block merge if pull request is outdated\" on the branch rule."
            }
            ForgeKind::GitHub => {
                "Enable the up-to-date (strict) policy so the gate judges the combined result."
            }
        })
    });
    out.push(if p.hidden.contains("force-push") {
        hidden("force-push")
    } else if p.force_push_blocked {
        Finding::new("force-push", Status::Pass, "force pushes are blocked")
    } else {
        Finding::new("force-push", Status::Fail, "force pushes are allowed")
            .fix("Protect the branch and block force pushes; a rewritten base voids the ratchets.")
    });
    out.push(if p.deletion_blocked {
        Finding::new("deletion", Status::Pass, "branch deletion is blocked")
    } else {
        Finding::new("deletion", Status::Warn, "the branch can be deleted")
            .fix("Protect the branch (GitHub: ruleset `deletion`).")
    });
    out.push(if p.pull_request_required {
        Finding::new(
            "pull-request",
            Status::Pass,
            "changes must arrive through a pull request",
        )
    } else {
        Finding::new(
            "pull-request",
            Status::Warn,
            "direct pushes are allowed when the required checks pass",
        )
        .fix("Disallow direct pushes to the branch; directives are read from the PR description.")
    });
    if p.pull_request_required && kind != ForgeKind::GitLab {
        // Review rules: what a pull request needs before the change under review can
        // merge. Each is what makes an override or an agent commit answer to a person.
        out.push(match p.required_approvals {
            Some(n) if n >= 1 => Finding::new(
                "review",
                Status::Pass,
                format!("a pull request needs {n} approving review(s)"),
            ),
            Some(_) => Finding::new(
                "review",
                Status::Warn,
                "a pull request needs no approving review",
            )
            .fix("Require at least one approving review; `directives.require_approval` reads it."),
            None => Finding::new(
                "review",
                Status::Info,
                "the approving-review count is not visible to this token",
            ),
        });
        out.push(if p.code_owner_review {
            Finding::new(
                "code-owner-review",
                Status::Pass,
                "code owners must approve changes to the paths they own",
            )
        } else if p.hidden.contains("code-owner-review") {
            Finding::new(
                "code-owner-review",
                Status::Info,
                "whether code owners must approve is not visible to this token",
            )
        } else {
            Finding::new(
                "code-owner-review",
                Status::Warn,
                "a CODEOWNERS entry does not require the owner's approval",
            )
            .fix("Require code-owner review; without it CODEOWNERS on discipline.toml and the workflows is a notification, not a gate.")
        });
        out.push(if p.last_push_approval {
            Finding::new(
                "last-push-approval",
                Status::Pass,
                "the most recent push needs an approval of its own",
            )
        } else if p.dismiss_stale_reviews {
            Finding::new(
                "last-push-approval",
                Status::Pass,
                "a push dismisses earlier approvals",
            )
        } else if p.hidden.contains("last-push-approval") {
            Finding::new(
                "last-push-approval",
                Status::Info,
                "whether a push dismisses earlier approvals is not visible to this token",
            )
        } else {
            Finding::new(
                "last-push-approval",
                Status::Warn,
                "an approval survives a later push; a commit added after the review merges on the review",
            )
            .fix("Require approval of the last push, or dismiss stale reviews on push.")
        });
    }
    out.push(if kind == ForgeKind::GitLab {
        Finding::new(
            "bypass",
            Status::Info,
            "merge rights are not checked on GitLab; with \"Pipelines must succeed\" nobody merges a failed pipeline",
        )
    } else if p.hidden.contains("bypass") {
        hidden("bypass")
    } else {
        match (&p.bypass, p.classic_enforce_admins) {
            (None, _) => Finding::new("bypass", Status::Warn, "the bypass list is not visible to this token")
                .fix("Re-run with an admin token to confirm nobody can bypass the rules."),
            (Some(list), _) if !list.is_empty() => Finding::new(
                "bypass",
                Status::Warn,
                format!("rules can be bypassed by: {}", list.join(", ")),
            )
            .fix(match kind {
                ForgeKind::Gitea => "Enable `block_admin_merge_override` and clear the merge allowlist.",
                ForgeKind::Forgejo => "Enable \"Apply to admins\" on the branch rule and clear the merge allowlist.",
                _ => "Empty the bypass list; fix a stuck check instead of overriding it.",
            }),
            (Some(_), Some(false)) => {
                Finding::new("bypass", Status::Warn, "classic protection does not apply to administrators")
                    .fix("Enable `enforce_admins` or move the rules into a ruleset without bypass.")
            }
            (Some(_), _) => Finding::new("bypass", Status::Pass, "no bypass actors"),
        }
    });
    out.push(Finding::new(
        "signed-commits",
        Status::Info,
        if p.signatures_required {
            "signed commits are required"
        } else {
            "signed commits are not required (optional)"
        },
    ));
    match p.thread_resolution_required {
        Some(true) => out.push(Finding::new(
            "thread-resolution",
            Status::Pass,
            "merges require every review conversation to be resolved",
        )),
        Some(false) => out.push(
            Finding::new(
                "thread-resolution",
                Status::Info,
                "merges do not require resolved review conversations (optional)",
            )
            .fix(match kind {
                ForgeKind::GitLab => "Settings → Merge requests → All threads must be resolved (`only_allow_merge_if_all_discussions_are_resolved`).",
                _ => "Require conversation resolution in the branch ruleset (`required_review_thread_resolution`) or classic protection.",
            }),
        ),
        None => {}
    }
    if matches!(kind, ForgeKind::Gitea | ForgeKind::Forgejo) {
        let mut workflows: Vec<&str> = jobs.iter().map(|j| j.workflow.as_str()).collect();
        workflows.sort_unstable();
        workflows.dedup();
        match &p.protected_file_patterns {
            _ if workflows.is_empty() => {}
            None if p.hidden.contains("workflow-protection") => {
                out.push(hidden("workflow-protection"))
            }
            None => {}
            Some(patterns) => {
                let covered = |path: &str| {
                    let path = path.to_lowercase();
                    patterns
                        .iter()
                        .filter_map(|pat| forge_file_glob(pat))
                        .any(|g| g.is_match(&path))
                };
                let open: Vec<&str> = workflows.iter().copied().filter(|w| !covered(w)).collect();
                out.push(if open.is_empty() {
                    Finding::new(
                        "workflow-protection",
                        Status::Pass,
                        "the workflows that run discipline are protected files",
                    )
                } else {
                    Finding::new(
                        "workflow-protection",
                        Status::Warn,
                        format!(
                            "{} can be changed by the pull request it judges: a pull-request workflow runs from the head",
                            open.join(", ")
                        ),
                    )
                    .fix("Add the workflow directories (`.gitea/workflows/**`, `.forgejo/workflows/**`, `.github/workflows/**`) to the branch rule's protected file patterns. A single `*` stops at a `.`, so `.gitea/workflows/*` does not match `ci.yml`.")
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::test_support::*;
    use crate::doctor::{
        analyse_workflows, gitea_protection, github_protection, gitlab_protection,
    };
    use crate::forge::CannedApi;

    #[test]
    fn review_rules_are_read_from_rulesets_and_classic_protection() {
        // A bare `pull_request` rule: reviews required, but nothing about them.
        let bare = serde_json::json!([{"type": "pull_request", "ruleset_id": 7, "parameters": {}}]);
        let gh = github(bare, serde_json::json!([]));
        let p = github_protection(&gh, &forge(ForgeKind::GitHub), "main").unwrap();
        assert!(p.pull_request_required && !p.code_owner_review && !p.last_push_approval);
        let jobs = analyse_workflows(&wf(WF), false).jobs;
        let f = protection_findings(ForgeKind::GitHub, &p, &jobs);
        let status = |id: &str| f.iter().find(|f| f.id == id).unwrap().status;
        assert_eq!(status("review"), Status::Info);
        assert_eq!(status("code-owner-review"), Status::Warn);
        assert_eq!(status("last-push-approval"), Status::Warn);

        // Classic protection carries the same three under other key names.
        let mut gh = github(serde_json::json!([]), serde_json::json!([]));
        gh.responses.insert(
            "github:repos/o/r/branches/main".into(),
            serde_json::json!({"protected": true, "protection": {"enabled": true}}),
        );
        gh.responses.insert(
            "github:repos/o/r/branches/main/protection".into(),
            serde_json::json!({"required_pull_request_reviews": {"required_approving_review_count": 2, "require_code_owner_reviews": true, "dismiss_stale_reviews": true}}),
        );
        let p = github_protection(&gh, &forge(ForgeKind::GitHub), "main").unwrap();
        assert_eq!(p.required_approvals, Some(2));
        assert!(p.code_owner_review && p.dismiss_stale_reviews && !p.last_push_approval);
        let f = protection_findings(ForgeKind::GitHub, &p, &jobs);
        let status = |id: &str| f.iter().find(|f| f.id == id).unwrap().status;
        assert_eq!(status("review"), Status::Pass);
        assert_eq!(status("code-owner-review"), Status::Pass);
        assert_eq!(status("last-push-approval"), Status::Pass);
    }

    #[test]
    fn github_rules_that_enforce_the_rollup_pass() {
        let gh = github(full_rules(), serde_json::json!([]));
        let p = github_protection(&gh, &forge(ForgeKind::GitHub), "main").unwrap();
        assert!(p.strict && p.force_push_blocked && p.deletion_blocked && p.pull_request_required);
        let jobs = analyse_workflows(&wf(WF), false).jobs;
        let f = protection_findings(ForgeKind::GitHub, &p, &jobs);
        assert!(
            f.iter()
                .all(|f| matches!(f.status, Status::Pass | Status::Info)),
            "{f:?}"
        );
    }

    #[test]
    fn required_check_that_does_not_run_discipline_fails() {
        let rules = serde_json::json!([
          {"type": "required_status_checks", "ruleset_id": 7, "parameters": {
             "strict_required_status_checks_policy": false,
             "required_status_checks": [{"context": "lint"}]}}
        ]);
        let gh = github(
            rules,
            serde_json::json!([{"actor_type": "RepositoryRole", "bypass_mode": "always"}]),
        );
        let p = github_protection(&gh, &forge(ForgeKind::GitHub), "main").unwrap();
        let jobs = analyse_workflows(&wf(WF), false).jobs;
        let f = protection_findings(ForgeKind::GitHub, &p, &jobs);
        let get = |id: &str| f.iter().find(|x| x.id == id).unwrap();
        assert_eq!(get("required-check").status, Status::Fail);
        assert!(get("required-check")
            .remediation
            .as_deref()
            .unwrap()
            .contains("ci-gate"));
        assert_eq!(get("up-to-date").status, Status::Warn);
        assert_eq!(get("force-push").status, Status::Fail);
        assert_eq!(get("deletion").status, Status::Warn);
        assert_eq!(get("bypass").status, Status::Warn);

        let none = protection_findings(ForgeKind::GitHub, &Protection::default(), &jobs);
        assert_eq!(none[0].status, Status::Fail);
        assert!(none[0].summary.contains("no status check"));
    }

    #[test]
    fn a_rollup_that_skips_or_ignores_results_does_not_enforce_discipline() {
        let gh = github(full_rules(), serde_json::json!([]));
        let p = github_protection(&gh, &forge(ForgeKind::GitHub), "main").unwrap();
        for weak in [
            // No `if: always()`: skipped when `gate` fails, and skipped counts as passed.
            WF.replace(
                "    if: always()\n    needs: [lint, gate, report]",
                "    needs: [lint, gate, report]",
            ),
            // Runs always but never reads the results.
            WF.replace(
                "        run: echo \"$NEEDS\" | jq -e 'all(.[]; .result == \"success\")'",
                "        run: echo done",
            )
            .replace(
                "      - env:\n          NEEDS: ${{ toJson(needs) }}\n",
                "      - ",
            ),
        ] {
            let jobs = analyse_workflows(&wf(&weak), false).jobs;
            assert!(jobs[0].rollups.is_empty(), "{weak}");
            let f = protection_findings(ForgeKind::GitHub, &p, &jobs);
            let rc = f.iter().find(|x| x.id == "required-check").unwrap();
            assert_eq!(rc.status, Status::Fail, "{weak}");
            assert!(
                rc.summary.contains("passes (or is skipped)"),
                "{}",
                rc.summary
            );
        }
    }

    #[test]
    fn a_marked_advisory_step_alone_leaves_the_job_not_enforcing() {
        let variant = WF.replace(
            "      - uses: orieg/discipline@v0\n",
            "      # discipline:advisory shadow of scripts/lint.sh until it is retired\n      - uses: orieg/discipline@v0\n        with:\n          advisory: true\n",
        );
        let facts = analyse_workflows(&wf(&variant), false);
        let nb = non_blocking(&facts);
        assert_eq!(nb.len(), 1, "{nb:?}");
        assert_eq!(nb[0].status, Status::Info, "{nb:?}");
        assert!(facts.jobs[0].allow_failure);
        let gh = github(full_rules(), serde_json::json!([]));
        let p = github_protection(&gh, &forge(ForgeKind::GitHub), "main").unwrap();
        let f = protection_findings(ForgeKind::GitHub, &p, &facts.jobs);
        assert_eq!(f[0].id, "required-check", "{f:?}");
        assert_eq!(f[0].status, Status::Fail, "{f:?}");
    }

    #[test]
    fn a_discipline_job_that_cannot_fail_does_not_count() {
        for (variant, why) in [
            (
                WF.replace(
                    "  gate:\n    name: Discipline\n",
                    "  gate:\n    name: Discipline\n    continue-on-error: true\n",
                ),
                "continue-on-error",
            ),
            (
                WF.replace(
                    "      - uses: orieg/discipline@v0\n",
                    "      - uses: orieg/discipline@v0\n        with:\n          advisory: true\n",
                ),
                "advisory",
            ),
            (
                WF.replace(
                    "      - uses: orieg/discipline@v0\n",
                    "      - run: discipline check --base origin/main || true\n",
                ),
                "masked",
            ),
        ] {
            let facts = analyse_workflows(&wf(&variant), false);
            assert!(
                facts.findings.iter().any(|f| f.id == "non-blocking"
                    && f.status == Status::Fail
                    && f.summary.contains(why)),
                "{why}: {:?}",
                facts.findings
            );
            let gh = github(full_rules(), serde_json::json!([]));
            let p = github_protection(&gh, &forge(ForgeKind::GitHub), "main").unwrap();
            let f = protection_findings(ForgeKind::GitHub, &p, &facts.jobs);
            assert_eq!(f[0].status, Status::Fail, "{why}: {f:?}");
        }
    }

    #[test]
    fn classic_protection_is_merged_and_unreadable_classic_is_an_error() {
        let mut gh = github(serde_json::json!([]), serde_json::json!([]));
        gh.responses.insert(
            "github:repos/o/r/branches/main".into(),
            serde_json::json!({"protected": true, "protection": {"enabled": true,
              "required_status_checks": {"contexts": ["Discipline"]}}}),
        );
        gh.responses.insert(
            "github:repos/o/r/branches/main/protection".into(),
            serde_json::json!({"__error": "HTTP 403"}),
        );
        assert!(github_protection(&gh, &forge(ForgeKind::GitHub), "main").is_err());
        gh.responses.insert(
            "github:repos/o/r/branches/main/protection".into(),
            serde_json::json!({
              "required_status_checks": {"strict": true, "contexts": ["Discipline"]},
              "allow_force_pushes": {"enabled": false},
              "allow_deletions": {"enabled": false},
              "enforce_admins": {"enabled": false}}),
        );
        let p = github_protection(&gh, &forge(ForgeKind::GitHub), "main").unwrap();
        assert!(p.required_contexts.contains("Discipline") && p.strict && p.force_push_blocked);
        let jobs = analyse_workflows(&wf(WF), false).jobs;
        let f = protection_findings(ForgeKind::GitHub, &p, &jobs);
        assert_eq!(
            f.iter().find(|x| x.id == "required-check").unwrap().status,
            Status::Pass
        );
        assert_eq!(
            f.iter().find(|x| x.id == "bypass").unwrap().status,
            Status::Warn
        );
    }

    /// The branch rule recorded from a Forgejo 12 instance (Gitea 1.24 returns the same
    /// fields plus `enable_force_push` and `block_admin_merge_override`).
    fn forgejo_rule() -> serde_json::Value {
        serde_json::json!([{"branch_name":"main","rule_name":"main","enable_push":false,
          "enable_push_whitelist":false,"enable_merge_whitelist":false,"enable_status_check":true,
          "status_check_contexts":["CI / ci-gate (pull_request)"],"required_approvals":1,
          "block_on_outdated_branch":true,"require_signed_commits":false,"apply_to_admins":false}])
    }

    #[test]
    fn gitea_and_forgejo_rules_match_actions_status_contexts() {
        let mut api = CannedApi::default();
        api.responses.insert(
            "forgejo:repos/o/r/branch_protections".into(),
            forgejo_rule(),
        );
        let p = gitea_protection(&api, &forge(ForgeKind::Forgejo), "main").unwrap();
        assert!(p.protected && p.strict && p.force_push_blocked && p.pull_request_required);
        let jobs = analyse_workflows(&wf(WF), false).jobs;
        // WF is named "CI"; its rollup `ci-gate` reports "CI / ci-gate (pull_request)".
        let f = protection_findings(ForgeKind::Forgejo, &p, &jobs);
        let get = |id: &str| f.iter().find(|x| x.id == id).unwrap().status;
        assert_eq!(get("required-check"), Status::Pass, "{f:?}");
        // `apply_to_admins: false` lets administrators merge past the checks.
        assert_eq!(get("bypass"), Status::Warn);

        // A glob pattern, as Gitea and Forgejo allow, also matches.
        let mut rule = forgejo_rule();
        rule[0]["status_check_contexts"] = serde_json::json!(["CI / *"]);
        rule[0]["apply_to_admins"] = serde_json::json!(true);
        api.responses
            .insert("forgejo:repos/o/r/branch_protections".into(), rule);
        let p = gitea_protection(&api, &forge(ForgeKind::Forgejo), "main").unwrap();
        let f = protection_findings(ForgeKind::Forgejo, &p, &jobs);
        let get = |id: &str| f.iter().find(|x| x.id == id).unwrap().status;
        assert_eq!(get("required-check"), Status::Pass, "{f:?}");
        assert_eq!(get("bypass"), Status::Pass);

        // Gitea can allow force pushes on a protected branch.
        let mut rule = forgejo_rule();
        rule[0]["enable_force_push"] = serde_json::json!(true);
        rule[0]["block_admin_merge_override"] = serde_json::json!(true);
        rule[0]["status_check_contexts"] = serde_json::json!(["CI / lint (pull_request)"]);
        api.responses
            .insert("gitea:repos/o/r/branch_protections".into(), rule);
        let p = gitea_protection(&api, &forge(ForgeKind::Gitea), "main").unwrap();
        let f = protection_findings(ForgeKind::Gitea, &p, &jobs);
        let get = |id: &str| f.iter().find(|x| x.id == id).unwrap().status;
        assert_eq!(get("force-push"), Status::Fail);
        assert_eq!(get("required-check"), Status::Fail);
    }

    /// Each row is a merge the lab ran against Gitea 1.24.7 or Forgejo 12 (#428): `true` when
    /// the forge refused the pull request for changing protected files.
    #[test]
    fn protected_file_patterns_match_as_gitea_and_forgejo_apply_them() {
        let protects = |pat: &str, path: &str| {
            forge_file_glob(pat).is_some_and(|g| g.is_match(&path.to_lowercase()))
        };
        for (pat, path, refused) in [
            (".gitea/workflows/*", ".gitea/workflows/ci.yml", false),
            (".gitea/workflows/**", ".gitea/workflows/ci.yml", true),
            (".gitea/workflows/*.yml", ".gitea/workflows/ci.yml", true),
            (".gitea/**", ".gitea/workflows/ci.yml", true),
            (".forgejo/workflows/*", ".forgejo/workflows/ci.yml", false),
            (".forgejo/workflows/**", ".forgejo/workflows/ci.yml", true),
            (
                ".forgejo/workflows/*.yml",
                ".forgejo/workflows/ci.yml",
                true,
            ),
            (".forgejo/workflows/ci.*", ".forgejo/workflows/ci.yml", true),
            ("*.toml", "discipline.toml", true),
            ("**.toml", "discipline.toml", true),
        ] {
            assert_eq!(protects(pat, path), refused, "{pat} on {path}");
        }
        // Read from the forge's rules rather than run: case folds, `?` stops at a
        // separator, classes and alternatives keep their meaning, a malformed pattern
        // protects nothing.
        assert!(protects(".GITEA/Workflows/**", ".gitea/workflows/CI.yml"));
        assert!(!protects(
            ".gitea/workflows/c?.yml",
            ".gitea/workflows/c..yml"
        ));
        assert!(protects(
            ".gitea/workflows/c?.yml",
            ".gitea/workflows/ci.yml"
        ));
        assert!(protects(
            ".{gitea,forgejo}/workflows/**",
            ".forgejo/workflows/ci.yml"
        ));
        assert!(protects(
            ".gitea/workflows/[a-c]i.yml",
            ".gitea/workflows/ci.yml"
        ));
        assert!(!protects(
            ".gitea/workflows/[!c]i.yml",
            ".gitea/workflows/ci.yml"
        ));
        assert!(forge_file_glob(".gitea/{workflows/**").is_none());
        assert!(forge_file_glob(".gitea/[workflows/**").is_none());
    }

    #[test]
    fn workflow_files_must_be_protected_on_gitea_and_forgejo() {
        let jobs = analyse_workflows(&wf(WF), false).jobs;
        assert!(!jobs.is_empty());
        let workflow = jobs[0].workflow.clone();
        let status = |patterns: &str| {
            let mut api = CannedApi::default();
            let mut rule = forgejo_rule();
            rule[0]["protected_file_patterns"] = serde_json::json!(patterns);
            api.responses
                .insert("gitea:repos/o/r/branch_protections".into(), rule);
            let p = gitea_protection(&api, &forge(ForgeKind::Gitea), "main").unwrap();
            let f = protection_findings(ForgeKind::Gitea, &p, &jobs);
            f.iter()
                .find(|x| x.id == "workflow-protection")
                .map(|x| x.status)
        };
        let dir = workflow.rsplit_once('/').unwrap().0.to_string();
        assert_eq!(status(""), Some(Status::Warn), "{workflow}");
        assert_eq!(status("docs/*"), Some(Status::Warn));
        assert_eq!(status(&format!("{dir}/**;docs/*")), Some(Status::Pass));
        // `*` does not cross `/` on these forges, nor `.` (#428): `{dir}/*` leaves
        // `ci.yml` open, and the PR that edits it merges.
        assert_eq!(status("*"), Some(Status::Warn));
        assert_eq!(status(&format!("{dir}/*")), Some(Status::Warn));
        // GitHub has no such setting: no finding.
        let p = Protection::default();
        assert!(!protection_findings(ForgeKind::GitHub, &p, &jobs)
            .iter()
            .any(|x| x.id == "workflow-protection"));
    }

    #[test]
    fn a_native_thread_resolution_rule_is_reported_on_github_and_gitlab() {
        let status = |kind: ForgeKind, required: Option<bool>| {
            let p = Protection {
                thread_resolution_required: required,
                ..Protection::default()
            };
            protection_findings(kind, &p, &[])
                .iter()
                .find(|x| x.id == "thread-resolution")
                .map(|x| x.status)
        };
        assert_eq!(status(ForgeKind::GitHub, Some(true)), Some(Status::Pass));
        assert_eq!(status(ForgeKind::GitLab, Some(false)), Some(Status::Info));
        assert_eq!(status(ForgeKind::Gitea, None), None);
        // GitLab reads it from the project.
        let mut api = CannedApi::default();
        api.responses.insert(
            "gitlab:projects/o%2Fr".into(),
            serde_json::json!({"only_allow_merge_if_all_discussions_are_resolved": true, "merge_method": "merge"}),
        );
        api.responses.insert(
            "gitlab:projects/o%2Fr/protected_branches/main".into(),
            serde_json::Value::Null,
        );
        let p = gitlab_protection(&api, &forge(ForgeKind::GitLab), "main").unwrap();
        assert_eq!(p.thread_resolution_required, Some(true));
    }

    #[test]
    fn gitea_review_rules_are_read_from_the_branch_protection() {
        let mut api = CannedApi::default();
        let mut rule = forgejo_rule();
        rule[0]["required_approvals"] = serde_json::json!(1);
        rule[0]["dismiss_stale_approvals"] = serde_json::json!(true);
        rule[0]["block_on_official_review_requests"] = serde_json::json!(true);
        api.responses
            .insert("gitea:repos/o/r/branch_protections".into(), rule.clone());
        let jobs = analyse_workflows(&wf(WF), false).jobs;
        let p = gitea_protection(&api, &forge(ForgeKind::Gitea), "main").unwrap();
        let f = protection_findings(ForgeKind::Gitea, &p, &jobs);
        let get = |id: &str| f.iter().find(|x| x.id == id).unwrap().status;
        assert_eq!(get("review"), Status::Pass);
        assert_eq!(get("code-owner-review"), Status::Pass);
        assert_eq!(get("last-push-approval"), Status::Pass);
        // The rule without them: each is a warning, not a guess.
        rule[0]["required_approvals"] = serde_json::json!(0);
        rule[0]["dismiss_stale_approvals"] = serde_json::json!(false);
        rule[0]["block_on_official_review_requests"] = serde_json::json!(false);
        api.responses
            .insert("gitea:repos/o/r/branch_protections".into(), rule);
        let p = gitea_protection(&api, &forge(ForgeKind::Gitea), "main").unwrap();
        let f = protection_findings(ForgeKind::Gitea, &p, &jobs);
        let get = |id: &str| f.iter().find(|x| x.id == id).unwrap().status;
        assert_eq!(get("review"), Status::Warn);
        assert_eq!(get("code-owner-review"), Status::Warn);
        assert_eq!(get("last-push-approval"), Status::Warn);
    }

    #[test]
    fn gitea_without_admin_token_falls_back_to_the_branch_summary() {
        let mut api = CannedApi::default();
        api.responses.insert(
            "gitea:repos/o/r/branch_protections".into(),
            serde_json::json!({"__error": "HTTP 401"}),
        );
        // Recorded anonymously from Gitea 1.24.
        api.responses.insert(
            "gitea:repos/o/r/branches/main".into(),
            serde_json::json!({"name":"main","protected":true,"required_approvals":1,
              "enable_status_check":true,"status_check_contexts":["CI / ci-gate (pull_request)"],
              "user_can_push":false,"user_can_merge":false}),
        );
        let p = gitea_protection(&api, &forge(ForgeKind::Gitea), "main").unwrap();
        let jobs = analyse_workflows(&wf(WF), false).jobs;
        let f = protection_findings(ForgeKind::Gitea, &p, &jobs);
        let get = |id: &str| f.iter().find(|x| x.id == id).unwrap();
        assert_eq!(get("required-check").status, Status::Pass);
        for id in ["force-push", "up-to-date", "bypass"] {
            assert_eq!(get(id).status, Status::Warn, "{id}");
            assert!(get(id).summary.contains("not visible"), "{id}");
        }
        // The summary carries the approval count; the review rules stay unseen.
        assert_eq!(get("review").status, Status::Pass);
        for id in ["code-owner-review", "last-push-approval"] {
            assert_eq!(get(id).status, Status::Info, "{id}");
            assert!(get(id).summary.contains("not visible"), "{id}");
        }
        // A `*` rule does not cover a branch with a `/` in it.
        api.responses.insert(
            "gitea:repos/o/r/branch_protections".into(),
            serde_json::json!([{"rule_name": "*", "enable_status_check": true,
              "status_check_contexts": ["CI / ci-gate (pull_request)"]}]),
        );
        let p = gitea_protection(&api, &forge(ForgeKind::Gitea), "release/1.0").unwrap();
        assert!(!p.protected, "{p:?}");
        let p = gitea_protection(&api, &forge(ForgeKind::Gitea), "main").unwrap();
        assert!(p.protected);
        // Unprotected branch, readable rules: nothing required.
        api.responses.insert(
            "gitea:repos/o/r/branch_protections".into(),
            serde_json::json!([]),
        );
        let p = gitea_protection(&api, &forge(ForgeKind::Gitea), "main").unwrap();
        let f = protection_findings(ForgeKind::Gitea, &p, &jobs);
        assert_eq!(f[0].status, Status::Fail);
        assert_eq!(
            f.iter().find(|x| x.id == "force-push").unwrap().status,
            Status::Fail
        );
    }
}
