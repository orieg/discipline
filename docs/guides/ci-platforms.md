---
layout: default
title: CI/CD Platform Integration Guide
permalink: /guides/ci-platforms/
---

# CI/CD Platform Integration Guide

This how-to guide walks through integrating `discipline` into major continuous integration, GitOps, and local developer workflows. Discipline runs as a single static binary with zero external dependencies and communicates natively with CI runners via standard exit codes, SARIF reports, JUnit XML, and GitLab Code Quality JSON.

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
  - remote: 'https://raw.githubusercontent.com/orieg/discipline/main/templates/discipline.gitlab-ci.yml'
```

### Custom Container Job

To customize runner parameters, define a standalone job using the official minimal container image:

```yaml
discipline:gate:
  stage: test
  image:
    name: ghcr.io/orieg/discipline:latest
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
        uses: actions/checkout@v4
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
- `ghcr.io/orieg/discipline:v0.5.1`

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
    rev: v0.5.1
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
