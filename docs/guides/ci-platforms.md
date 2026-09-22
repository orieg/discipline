---
layout: default
title: CI/CD Platform Integration Guide
permalink: /guides/ci-platforms/
---

# CI/CD Platform Integration Guide

This how-to guide walks through integrating `discipline` into major continuous integration, GitOps, and local developer workflows. Discipline runs as a single static binary with no runtime dependencies (the opt-in forge checks make their own HTTPS requests; see [Forge access](../GATES.md#forge-access)) and communicates with CI runners via standard exit codes, SARIF reports, JUnit XML, and GitLab Code Quality JSON. A gate only protects a branch when the platform refuses merges that fail it: see [Repository Protection](#8-repository-protection).

---

## 1. GitHub Actions

Discipline provides an official composite GitHub Action at `orieg/discipline@v0`. Because it is implemented as a pure composite action wrapping static native binaries, it requires no Node.js or Docker setup on the runner.

### Basic Pull Request Sentinel

Create `.github/workflows/discipline.yml`:

```yaml
name: Discipline Sentinel

on:
  pull_request:
    types: [opened, synchronize, reopened, edited]

jobs:
  discipline:
    name: Verify Invariants
    runs-on: ubuntu-latest
    permissions:
      contents: read
      pull-requests: read
      security-events: write # Required if uploading SARIF

    steps:
      - name: Checkout Code
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          fetch-depth: 0 # Full history required to reach merge base

      - name: Run Discipline Gatekeeper
        uses: orieg/discipline@v0
        with:
          fail_on_warnings: false
          sarif: results.sarif

      - name: Upload SARIF Security Findings
        if: always()
        uses: github/codeql-action/upload-sarif@v3
        with:
          sarif_file: results.sarif
```

### Advisory Mode for Brownfield Adoption

When evaluating Discipline on high-velocity repositories with legacy technical debt, run in advisory mode to generate reports without breaking builds:

```yaml
      - name: Run Discipline (Advisory Mode)
        uses: orieg/discipline@v0
        with:
          advisory: true
```

---

## 2. GitLab CI/CD

Discipline integrates with GitLab CI/CD via a shared component template or a standalone container job. It produces native GitLab Code Quality reports that render directly inside Merge Request diff views.

### Remote Pipeline Include

Include the official component in `.gitlab-ci.yml`:

```yaml
include:
  - remote: 'https://raw.githubusercontent.com/orieg/discipline/v0.9.0/templates/discipline.gitlab-ci.yml'
```

### Custom Container Job

To customize runner parameters, define a standalone job using the official minimal container image:

```yaml
discipline:gate:
  stage: test
  image:
    name: ghcr.io/orieg/discipline:v0
    entrypoint: [""]
  variables:
    GIT_STRATEGY: clone
    GIT_DEPTH: 0
  rules:
    - if: $CI_PIPELINE_SOURCE == "merge_request_event"
    - if: $CI_COMMIT_BRANCH == $CI_DEFAULT_BRANCH
  before_script:
    - git fetch origin $CI_MERGE_REQUEST_TARGET_BRANCH_NAME --depth=100 || true
  script:
    - discipline check --output-format gitlab-codequality > gl-codequality.json
  artifacts:
    reports:
      codequality: gl-codequality.json
    paths:
      - gl-codequality.json
    when: always
```

---

## 3. Forgejo & Gitea Actions

Forgejo and Gitea Actions use runner engines compatible with GitHub Actions workflows. The composite action runs natively under `forgejo-runner` and `act_runner` without requiring internet access if the binary is cached.

### Workflow Configuration

Create `.forgejo/workflows/discipline.yaml` (or `.gitea/workflows/discipline.yaml`):

```yaml
name: Invariant Verification

on:
  pull_request:
    types: [opened, synchronize, reopened]

jobs:
  gate:
    runs-on: docker
    steps:
      - name: Checkout Repository
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          fetch-depth: 0

      - name: Run Gatekeeper
        uses: https://github.com/orieg/discipline@v0
        with:
          fail_on_warnings: true
```

---

## 4. Argo Workflows (GitOps)

In Kubernetes GitOps environments, Discipline can run as a pre-sync or pre-promotion validation step. Use the official Argo template from `templates/argo-workflow-template.yaml`:

```yaml
apiVersion: argoproj.io/v1alpha1
kind: Workflow
metadata:
  generateName: discipline-gitops-check-
spec:
  entrypoint: validate-diff
  templates:
    - name: validate-diff
      steps:
        - - name: run-discipline-gate
            templateRef:
              name: discipline-sentinel
              template: discipline-gate
            arguments:
              parameters:
                - name: repo-url
                  value: "https://github.com/my-org/my-repo.git"
                - name: target-branch
                  value: "main"
                - name: source-branch
                  value: "feat/my-feature"
```

---

## 5. Docker & Container Environments

Official multi-architecture (`linux/amd64`, `linux/arm64`) OCI images are published to GitHub Container Registry:
- `ghcr.io/orieg/discipline:latest`
- `ghcr.io/orieg/discipline:v0`
- `ghcr.io/orieg/discipline:v0.9.0`

### Running Locally via Docker

Mount your local repository and execute against the merge base:

```bash
docker run --rm \
  -v "$(pwd)":/work:ro \
  -w /work \
  ghcr.io/orieg/discipline:latest \
  discipline check --base origin/main
```

### Security & Non-Root Execution

The container executes as unprivileged user `10001:10001` to meet strict Kubernetes PodSecurityStandards and CIS benchmarks. The image automatically sets `safe.directory = "*"` in the container global git configuration so volume checkouts owned by different host UIDs function without manual permission adjustments.

---

## 6. Pre-Commit & Git Hooks

### pre-commit Framework

Add Discipline to `.pre-commit-config.yaml`:

```yaml
repos:
  - repo: https://github.com/orieg/discipline
    rev: v0.9.0
    hooks:
      - id: discipline          # builds via cargo on first run
      # Or: - id: discipline-system # invokes pre-installed binary on PATH
```

### Native Git Hooks

Install Discipline's native hooks directly into `.git/hooks/`:

```bash
# Installs non-blocking pre-commit and pre-push hooks
discipline install-hooks
```

To run manually during interactive git staging:

```bash
# Check staged changes against HEAD
discipline check --staged

# Fast diff inspection of unstaged working tree changes
discipline diff
```

---

## 7. Other CI Platforms (Templates)

Copy-paste starting points live in [`templates/`](https://github.com/orieg/discipline/tree/main/templates). Each runs the official container image, fetches the merge base, and writes `discipline-report.json`:

| Platform | Template | Base ref |
|---|---|---|
| GitLab CI/CD (component) | [`discipline.gitlab-ci.yml`](https://github.com/orieg/discipline/blob/main/templates/discipline.gitlab-ci.yml) | `base_ref` input, else `CI_MERGE_REQUEST_TARGET_BRANCH_NAME` |
| Argo Workflows | [`argo-workflow-template.yaml`](https://github.com/orieg/discipline/blob/main/templates/argo-workflow-template.yaml) | `target-branch` parameter |
| Azure Pipelines | [`azure-pipelines.yml`](https://github.com/orieg/discipline/blob/main/templates/azure-pipelines.yml) | `System.PullRequest.TargetBranch`, else `main` |
| Bitbucket Pipelines | [`bitbucket-pipelines.yml`](https://github.com/orieg/discipline/blob/main/templates/bitbucket-pipelines.yml) | `BITBUCKET_PR_DESTINATION_BRANCH`, else `main` |
| CircleCI | [`circleci-config.yml`](https://github.com/orieg/discipline/blob/main/templates/circleci-config.yml) | `CIRCLE_BASE_REVISION`, else `origin/main` |
| Jenkins | [`Jenkinsfile`](https://github.com/orieg/discipline/blob/main/templates/Jenkinsfile) | `CHANGE_TARGET`, else `main` |

The templates reference the `v0` image tag, which tracks the latest `v0.x.y` release. Pin an exact version (`v0.9.0`) or an image digest when a gate verdict must be reproducible from the pipeline file alone. If the merge base cannot be fetched, `discipline check` exits 2 rather than checking against the wrong base.

---

## 8. Repository Protection

A failing gate blocks nothing unless the platform refuses to merge the change. Set these once when adopting discipline, then verify them with `discipline doctor`:

```bash
discipline doctor                # workflows, CODEOWNERS, and the default branch's protection
discipline doctor --local-only   # repository files only, no platform API
discipline doctor --strict       # warnings fail too (exit 1)
```

`doctor` reads which workflow jobs run discipline and which rollup jobs depend on them (Actions workflows under `.github/`, `.gitea/` and `.forgejo/`, or `.gitlab-ci.yml` including the discipline component), then asks the forge whether the default branch requires one of them. On GitHub, Gitea and Forgejo that is a required status check (Gitea and Forgejo name Actions checks `<workflow> / <job> (<event>)`, and their patterns are matched as globs); on GitLab it is "Pipelines must succeed" with a discipline job that is not `allow_failure`. It also reports the up-to-date policy, force-push and deletion blocking, the pull-request requirement and its review rules (`review`: the approving-review count; `code-owner-review`: whether a CODEOWNERS owner must approve; `last-push-approval`: whether an approval survives a later push, satisfied by last-push approval or by dismissing stale reviews), bypass (administrators, allowlists, ruleset bypass actors), workflow triggers and token permissions, and `CODEOWNERS` coverage of the gate configuration. Exit `0` means nothing failed, `1` a check failed, and `2` a check could not be decided. Access: requests go over HTTPS with an optional token (`GH_TOKEN`/`GITHUB_TOKEN`, `GITLAB_TOKEN`, `GITEA_TOKEN`, `FORGEJO_TOKEN`); GitHub rulesets need read access, classic protection an admin token. Without an admin token, Gitea and Forgejo still show the required checks and report the admin-only settings as not visible. On Gitea below 1.26, which ignores a workflow's `permissions:` (the instance scopes the token), the token finding is reported as information with the upgrade that makes the fix effective; GitLab hides "Pipelines must succeed" from anonymous readers, which leaves that check undecided. See [Forge access](../GATES.md#forge-access) for tokens and forge detection.

### Protection checklist

| Setting | Why |
|---|---|
| **Require the check before merge** on the default branch | Without it a red run is advisory. Require the rollup job (for example `ci-gate`) that `needs:` every verification job, including discipline, rather than each job by name; `ci-integrity` keeps that rollup complete. |
| **Require the branch to be up to date** before merge | Discipline evaluates the diff against the merge base. A stale green run says nothing about the combined result. |
| **Block force pushes and branch deletion** | A rewritten default branch removes the base that ratchets (`test-floor`, `test-budget`, `unsafe-budget`) and config-integrity compare against. |
| **No bypass for administrators** | An admin merge skips every gate. Leave the bypass list empty and fix a stuck check instead of overriding it. |
| **Require pull requests** (and review, with more than one maintainer) | Directives are read from the PR description and commits. A direct push has no PR body, so it cannot carry a reviewed override. |
| **Protect gate configuration with CODEOWNERS** | Put `discipline.toml`, `discipline-baseline.toml`, `.github/workflows/`, and any registry the gates read (for example a superseded-figure registry) under a code owner, so weakening them needs a named reviewer. `config-integrity` and `ci-integrity` catch the common forms; review catches the rest. |
| **Require signed commits** (optional) | Ties commits to verified identities. Rebase locally (`git rebase origin/main`) when a branch falls behind; a server-side "update branch" rebase rewrites commits unsigned and the merge is then blocked. |
| **Least-privilege workflow token** | `permissions: contents: read` is enough for the action. Run discipline on `pull_request`, not `pull_request_target`, so untrusted code never runs with a write token. |

Add `edited` to the `pull_request` event types so that changing a PR description (adding or removing a directive) re-runs the gate. With a merge queue, also trigger on `merge_group` and require the same check there.

### GitHub: ruleset example

A ruleset on the default branch with a required rollup check, up-to-date policy, no force pushes or deletions, and no bypass:

```bash
gh api --method POST repos/OWNER/REPO/rulesets --input - <<'JSON'
{
  "name": "default branch protection",
  "target": "branch",
  "enforcement": "active",
  "bypass_actors": [],
  "conditions": { "ref_name": { "include": ["~DEFAULT_BRANCH"], "exclude": [] } },
  "rules": [
    { "type": "required_status_checks",
      "parameters": {
        "strict_required_status_checks_policy": true,
        "required_status_checks": [ { "context": "ci-gate" } ] } },
    { "type": "pull_request",
      "parameters": {
        "required_approving_review_count": 1,
        "dismiss_stale_reviews_on_push": true,
        "require_code_owner_review": true,
        "require_last_push_approval": false,
        "required_review_thread_resolution": false } },
    { "type": "non_fast_forward" },
    { "type": "deletion" }
  ]
}
JSON
```

A single-maintainer repository cannot approve its own pull requests: drop the `pull_request` rule's review count to `0` (keep the rule so changes still arrive as pull requests), or leave it out. Add `{ "type": "required_signatures" }` to require signed commits. Replace `ci-gate` with the name of your rollup job, or with `discipline` if the gate is its own workflow.

Minimal `CODEOWNERS` (`.github/CODEOWNERS`):

```text
/discipline.toml             @OWNER
/discipline-baseline.toml    @OWNER
/.github/workflows/          @OWNER
```

### GitLab

Under **Settings → Repository → Protected branches**, allow no one to push or force push to the default branch. Under **Settings → Merge requests**, enable **Pipelines must succeed**. Add approval rules and `CODEOWNERS` with **Code owner approval** on the protected branch. Use the [component](#2-gitlab-ci-cd) in the merge request pipeline so the check runs against the target branch.

### Gitea & Forgejo

Under **Settings → Branches → Branch protection** for the default branch, enable **Status check** with the discipline job (or its rollup) as a required context, disable force push, require approvals as the team allows, and list `discipline.toml` and the workflow directory under **Protected file patterns**.

