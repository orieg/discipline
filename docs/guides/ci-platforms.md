---
layout: default
title: CI/CD Platform Integration Guide
permalink: /guides/ci-platforms/
---

# CI/CD Platform Integration Guide

This how-to guide walks through integrating `discipline` into major continuous integration, GitOps, and local developer workflows. Discipline runs as a single static binary with no runtime dependencies and communicates with CI runners via standard exit codes, SARIF reports, JUnit XML, and GitLab Code Quality JSON. A pull-request run with a resolvable merge base makes no network requests. Three cases do: the opt-in forge features (issue state, bench citation freshness, `require_approval`, `replay`, `doctor`, `comment`); a push run with no pull-request body, where the default `merged-pr-body` directive source asks the forge API for the merged pull request; and a CI run (`CI`, `GITHUB_ACTIONS`, `GITLAB_CI`, `GITEA_ACTIONS` or `FORGEJO_ACTIONS` set) whose merge base does not resolve, where the binary runs `git fetch` for the base and unshallows the clone. `DISCIPLINE_NO_NETWORK=1` keeps the forge requests off the network; it does not stop that `git fetch`. See [Forge access](../GATES.md#forge-access). A gate only protects a branch when the platform refuses merges that fail it: see [Repository Protection](#8-repository-protection).

---

## 1. GitHub Actions

Discipline provides an official composite GitHub Action at `orieg/discipline@v0`. Because it is implemented as a pure composite action wrapping static native binaries, it requires no Node.js or Docker setup on the runner. The action writes workflow annotations, a step summary and step outputs; a pull-request comment needs `comment: true`, and a SARIF file needs `DISCIPLINE_REPORT_SARIF` plus an upload step (below).

A repository that runs the default `ci-integrity` gate must pin third-party actions by commit SHA, so adding `uses: orieg/discipline@v0` is itself an `Unpinned Third-Party Action` error. Pin the action to a commit; a SHA ref runs the binary of the release that commit's `Cargo.toml` names (`version:` under `with:` picks another):

```yaml
      - uses: orieg/discipline@<commit-sha> # v0.12.3
```

The examples below use `@v0` for readability.

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
      security-events: write # Required to upload SARIF

    steps:
      - name: Checkout Code
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          fetch-depth: 0 # Full history required to reach merge base

      - name: Run Discipline Gatekeeper
        uses: orieg/discipline@v0
        env:
          DISCIPLINE_REPORT_SARIF: results.sarif

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

Adding `advisory: true` is itself a `ci-integrity` finding (`Discipline Action Weakened`), since the step then exits 0 whatever the gates report. Record the decision in the pull-request body with `allow-gate-weakening: ci-integrity <reason>`.

---

### Rollup skip-set wiring (`ci-skip-set`)

A rollup job that accepts `skipped` as passing cannot tell "skipped because the change did not touch it" from "skipped because change detection emitted all-false". `ci-skip-set` checks the skip set against the data the rollup saw. The rollup supplies it; nothing is fetched.

```yaml
jobs:
  detect-changes:
    runs-on: ubuntu-latest
    outputs:
      rust-src: ${{ steps.filter.outputs.rust-src }}
      docs: ${{ steps.filter.outputs.docs }}
    steps:
      - uses: actions/checkout@<sha> # v4
      - id: filter
        uses: dorny/paths-filter@<sha> # v3
        with:
          filters: |
            rust-src: ['src/**', 'Cargo.*']
            docs: ['docs/**']

  test:
    needs: detect-changes
    if: needs.detect-changes.outputs.rust-src == 'true'
    runs-on: ubuntu-latest
    steps: [ ... ]

  docs-lint:
    needs: detect-changes
    if: needs.detect-changes.outputs.docs == 'true'
    runs-on: ubuntu-latest
    steps: [ ... ]

  ci-gate:
    needs: [detect-changes, test, docs-lint]
    if: always()
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@<sha> # v4
        with:
          fetch-depth: 0
      - uses: orieg/discipline@v0
        with:
          suite: integrity
          # The rollup's own view of its dependencies: results and outputs.
          ci_context: ${{ toJson(needs) }}
```

```toml
[gates.ci-skip-set]
workflow = ".github/workflows/ci.yml"   # default: the running workflow
change_job = "detect-changes"           # must have concluded `success`
unconditional_jobs = ["ci-gate"]        # may never be `skipped`
```

What the gate asserts from that context: the change-detection job succeeded; every job in `unconditional_jobs` ran; and for every job in the rollup's `needs`, `skipped` holds exactly when its `if:` evaluates false under the observed outputs and results. The evaluator models `needs.<job>.result`, `needs.<job>.outputs.<key>`, the status functions `always()`, `success()`, `failure()` and `cancelled()`, comparisons (`==`, `!=`), `&&`, `||`, `!` and parentheses, with or without the `${{ }}` wrapper. An `if:` term outside that set is a finding, never a guess; a job whose `if:` mixes literal text and `${{ }}` is one too. `toJson(needs)` carries only the jobs the rollup names in `needs`, so a conditional job the rollup does not depend on is outside the check: add it to `needs`.

### Benchmark job wiring (`bench-regression`)

Measure the merge base and the head in the same job on the same runner, hand both files to the gate, and require a regression override to cite the run it rests on.

```yaml
  bench:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@<sha> # v4
        with:
          fetch-depth: 0
      - uses: dtolnay/rust-toolchain@<sha> # stable
      - run: cargo install iai-callgrind-runner --locked
      - name: Measure the merge base
        run: |
          git worktree add ../base "$(git merge-base origin/main HEAD)"
          (cd ../base && cargo bench --bench compare -- --output-format=json > "$RUNNER_TEMP/base.json")
      - name: Measure the head
        run: cargo bench --bench compare -- --output-format=json > "$RUNNER_TEMP/head.json"
      - uses: orieg/discipline@v0
        env:
          DISCIPLINE_BENCH_BASE_FILE: ${{ runner.temp }}/base.json
          DISCIPLINE_BENCH_HEAD_FILE: ${{ runner.temp }}/head.json
          DISCIPLINE_BENCH_PROVENANCE: ${{ runner.os }}-${{ runner.arch }}
        with:
          suite: bench
```

```toml
[gates.bench-regression]
severity = "error"
tolerance_pct = 2.0
require_sourced_override = true
citation_source_paths = ["src/**", "benches/**"]
citation_measurement_jobs = [{ job = "bench", guard = "Measure the head" }]
```

The two files are compared arm by arm; iai-callgrind's instruction counts are deterministic, so the comparison is exact, and a wall-clock harness (Criterion, pytest-benchmark, Google Benchmark) is compared by interval instead. A regression needs `allow-regression: <arm> <reason>` naming every regressed arm. With `require_sourced_override`, the reason must also cite a CI run URL or a committed artifact path, and **every** citation in the reason is checked for freshness, not the first: a cited run must have a conclusion that carries a measurement (`cancelled`, `timed_out`, `action_required`, `startup_failure`, `stale` and `skipped` do not); a run that concluded `failure` counts only when every job in `citation_measurement_jobs` reached its `guard` step with every earlier step green (a regression trips the guard on numbers it measured; a crashed benchmark leaves none); the cited run's head must be reachable from the head under review; a cited data artifact must be tracked and must not have been committed before the branch's newest change under `citation_source_paths`. A citation the run cannot decide (no token, rate limited, a non-GitHub URL) is named and leaves the gate armed.

## 2. GitLab CI/CD

Discipline integrates with GitLab CI/CD via a shared component template or a standalone container job. It produces native GitLab Code Quality reports that render directly inside Merge Request diff views.

### Remote Pipeline Include

Include the official component in `.gitlab-ci.yml`:

```yaml
include:
  - remote: 'https://raw.githubusercontent.com/orieg/discipline/v0.12.3/templates/discipline.gitlab-ci.yml'
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
    - git fetch origin $CI_MERGE_REQUEST_TARGET_BRANCH_NAME || true
  script:
    - discipline check --report-gitlab gl-codequality.json --report-junit junit.xml
  artifacts:
    reports:
      codequality: gl-codequality.json
    paths:
      - gl-codequality.json
    when: always
```

---

## 3. Forgejo & Gitea Actions

Forgejo and Gitea Actions use runner engines compatible with GitHub Actions workflows. The composite action runs natively under `forgejo-runner` and `act_runner`. It downloads the release binary on every run; on a runner without internet access, pass `binary_path` pointing at a discipline binary already on the runner.

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
                - name: pr-body
                  value: ""
                - name: discipline-image
                  value: "ghcr.io/orieg/discipline:v0"
```

---

## 5. Docker & Container Environments

Official multi-architecture (`linux/amd64`, `linux/arm64`) OCI images are published to GitHub Container Registry:
- `ghcr.io/orieg/discipline:latest`
- `ghcr.io/orieg/discipline:v0`
- `ghcr.io/orieg/discipline:v0.12.3`

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
    rev: v0.12.3
    hooks:
      - id: discipline          # builds via cargo on first run
      # Or: - id: discipline-system # invokes pre-installed binary on PATH
```

### Native Git Hooks

Install Discipline's native hooks directly into `.git/hooks/`:

```bash
# Writes .git/hooks/pre-commit, which runs `discipline check --staged`
# and blocks the commit when a gate fails
discipline install-hooks
```

To run manually during interactive git staging:

```bash
# Check staged changes against HEAD
discipline check --staged

# Staged and unstaged changes against HEAD (untracked files are not included)
discipline diff
```

---

## 7. Other CI Platforms (Templates)

Copy-paste starting points live in [`templates/`](https://github.com/orieg/discipline/tree/main/templates). Each runs the official container image and fetches the base branch. The Azure, Bitbucket, CircleCI and Jenkins templates write `discipline-report.json`; the GitLab component writes `gl-codequality.json` and `junit.xml`; the Argo template writes `junit.xml`, `sarif.sarif` and `discipline.json` under `/workspace/reports/`:

| Platform | Template | Base ref |
|---|---|---|
| GitLab CI/CD (component) | [`discipline.gitlab-ci.yml`](https://github.com/orieg/discipline/blob/main/templates/discipline.gitlab-ci.yml) | `base_ref` input, else `CI_MERGE_REQUEST_TARGET_BRANCH_NAME` |
| Argo Workflows | [`argo-workflow-template.yaml`](https://github.com/orieg/discipline/blob/main/templates/argo-workflow-template.yaml) | `target-branch` parameter |
| Azure Pipelines | [`azure-pipelines.yml`](https://github.com/orieg/discipline/blob/main/templates/azure-pipelines.yml) | `System.PullRequest.TargetBranch`, else `main` |
| Bitbucket Pipelines | [`bitbucket-pipelines.yml`](https://github.com/orieg/discipline/blob/main/templates/bitbucket-pipelines.yml) | `BITBUCKET_PR_DESTINATION_BRANCH`, else `main` |
| CircleCI | [`circleci-config.yml`](https://github.com/orieg/discipline/blob/main/templates/circleci-config.yml) | `CIRCLE_BASE_REVISION`, else `origin/main` |
| Jenkins | [`Jenkinsfile`](https://github.com/orieg/discipline/blob/main/templates/Jenkinsfile) | `CHANGE_TARGET`, else `main` |

The templates reference the `v0` image tag, which tracks the latest `v0.x.y` release. Pin an exact version (`v0.12.3`) or an image digest when a gate verdict must be reproducible from the pipeline file alone. If the merge base cannot be fetched, `discipline check` exits 2 rather than checking against the wrong base.

---

## 8. Repository Protection

A failing gate blocks nothing unless the platform refuses to merge the change. Set these once when adopting discipline, then verify them with `discipline doctor`:

```bash
discipline doctor                # workflows, CODEOWNERS, and the default branch's protection
discipline doctor --local-only   # repository files only, no platform API
discipline doctor --strict       # warnings fail too (exit 1)
```

`doctor` reads which workflow jobs run discipline and which rollup jobs depend on them (Actions workflows under `.github/`, `.gitea/` and `.forgejo/`, or `.gitlab-ci.yml` including the discipline component), then asks the forge whether the default branch requires one of them. On GitHub, Gitea and Forgejo that is a required status check (Gitea and Forgejo name Actions checks `<workflow> / <job> (<event>)`, and their patterns are matched as globs); on GitLab it is "Pipelines must succeed" with a discipline job that is not `allow_failure`. It also reports the up-to-date policy, force-push and deletion blocking, the pull-request requirement and its review rules (`review`: the approving-review count; `code-owner-review`: whether a CODEOWNERS owner must approve; `last-push-approval`: whether an approval survives a later push, satisfied by last-push approval or by dismissing stale reviews; on Gitea and Forgejo these are `required_approvals`, `block_on_official_review_requests` — the two forges request reviews from CODEOWNERS on their own, and that setting makes the requests blocking — and `dismiss_stale_approvals`, readable with an admin token, otherwise reported as not visible), bypass (administrators, allowlists, ruleset bypass actors), workflow triggers and token permissions (including `push-trigger`: a discipline job that runs on push to the default branch while the repository allows squash or rebase merges, which drop a pull request's body from the merge commit — a warning when `merged-pr-body` is not in `directives.sources`, information when it is: give the push run a token that can read pull requests, restrict the step to `pull_request`, or put directives in commit messages), and `CODEOWNERS` coverage of the gate configuration. Exit `0` means nothing failed, `1` a check failed, and `2` a check could not be decided. Access: requests go over HTTPS with an optional token (`GH_TOKEN`/`GITHUB_TOKEN`, `GITLAB_TOKEN`, `GITEA_TOKEN`, `FORGEJO_TOKEN`); GitHub rulesets need read access, classic protection an admin token. Without an admin token, Gitea and Forgejo still show the required checks and report the admin-only settings as not visible. On Gitea below 1.26, which ignores a workflow's `permissions:` (the instance scopes the token), the token finding is reported as information with the upgrade that makes the fix effective; GitLab hides "Pipelines must succeed" from anonymous readers, which leaves that check undecided. See [Forge access](../GATES.md#forge-access) for tokens and forge detection.

### Protection checklist

| Setting | Why |
|---|---|
| **Require the check before merge** on the default branch | Without it a red run is advisory. Require the rollup job (for example `ci-gate`) that `needs:` every verification job, including discipline, rather than each job by name; `ci-integrity` keeps that rollup complete. |
| **Require the branch to be up to date** before merge | Discipline evaluates the diff against the merge base. A stale green run says nothing about the combined result. |
| **Block force pushes and branch deletion** | A rewritten default branch removes the base that ratchets (`test-floor`, `test-budget`, `unsafe-budget`) and config-integrity compare against. |
| **No bypass for administrators** | An admin merge skips every gate. Leave the bypass list empty and fix a stuck check instead of overriding it. |
| **Require pull requests** (and review, with more than one maintainer) | Directives are read from the PR description and commits. A direct push has no PR body, so it cannot carry a reviewed override. |
| **Gate on `pull_request`, not on `push` to the default branch** | A `push` run reads only the pushed commits' messages. A squash or rebase merge drops the PR body (a merge commit's default message does not carry it either), so a waiver that passed review fails the push run that follows the merge. Run the gate on `pull_request` (with `edited`) and `merge_group`; if a push run is wanted, put directives in commit messages too, or keep the `merged-pr-body` source (on by default) with a token that can read pull requests. `ci-integrity` reports a verification step that gains an `if:` (`Verification Step Narrowed`, warning), so restricting the discipline step to `pull_request` is recorded rather than silent. |
| **Protect gate configuration with CODEOWNERS** | Put `discipline.toml`, `discipline-baseline.toml`, `.github/workflows/`, and any registry the gates read (for example a superseded-figure registry) under a code owner, so weakening them needs a named reviewer. `config-integrity` and `ci-integrity` catch the common forms; review catches the rest. |
| **Require signed commits** (optional) | Ties commits to verified identities. Rebase locally (`git rebase origin/main`) when a branch falls behind; a server-side "update branch" rebase rewrites commits unsigned and the merge is then blocked. |
| **Least-privilege workflow token** | `permissions: contents: read` is enough for the action. Run discipline on `pull_request`, not `pull_request_target`, so untrusted code never runs with a write token. |

Add `edited` to the `pull_request` event types so that changing a PR description (adding or removing a directive) re-runs the gate. With a merge queue, also trigger on `merge_group` and require the same check there. Do not add `push: branches: [main]` for the same job unless every directive it relies on is also in a commit message: the push run cannot see the PR body (see the event / source table in [CONFIGURATION.md](../CONFIGURATION.md#override-directives)).

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

