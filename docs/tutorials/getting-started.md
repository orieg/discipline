---
layout: default
title: "Tutorial: Getting Started with Discipline Sandbox"
permalink: /tutorials/getting-started/
---

# Getting Started with Discipline: A Hands-on Sandbox Tutorial

This hands-on tutorial guides you through setting up a temporary sandbox git repository, running Discipline against a git diff, watching it catch an AI coding agent assertion weakening, and resolving the violation.

**Diátaxis Type:** Tutorial (Learning by doing; practical steps with a concrete outcome).

---

## Prerequisites

- `git` 2.28+ installed
- `curl` and `bash`
- A Unix terminal (macOS or Linux)

---

## Step 1: Create a Temporary Git Sandbox

Create an isolated directory and initialize a fresh git repository:

```bash
mkdir -p /tmp/discipline-sandbox
cd /tmp/discipline-sandbox
git init -b main
```

Configure local dummy git credentials for this sandbox:

```bash
git config user.name "Sandbox Developer"
git config user.email "developer@example.com"
```

---

## Step 2: Install Discipline

Fetch and install the latest pre-compiled static binary:

```bash
curl -fsSL https://orieg.github.io/discipline/install.sh | bash
```

Verify that Discipline is in your PATH and check its version:

```bash
discipline --version
```

---

## Step 3: Establish Baseline Code on `main`

Create a simple Rust source file and unit test:

```bash
cat << 'EOF' > test_calculator.rs
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[test]
fn test_addition() {
    assert_eq!(add(2, 2), 4);
    assert_eq!(add(-1, 1), 0);
}
EOF
```

Commit this baseline code to the `main` branch:

```bash
git add test_calculator.rs
git commit -m "feat: initial calculator implementation and tests"
```

---

## Step 4: Simulate an Agent Assertion Weakening

Create a new feature branch:

```bash
git checkout -b feat/agent-refactor
```

Simulate an autonomous coding agent that optimizes for a green test suite by weakening the assertion strength from an exact equality check `assert_eq!` to a tautological `assert!(true)`:

```bash
cat << 'EOF' > test_calculator.rs
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[test]
fn test_addition() {
    assert!(true);
}
EOF
```

---

## Step 5: Run Discipline Against the Diff

Run Discipline against the `main` merge base:

```bash
discipline check --base main
```

Discipline uses tree-sitter AST diff traversal to inspect syntactic modifications. Because assertions in `test_addition` dropped from two `assert_eq!` nodes to one tautological `assert!`, Discipline fails closed with exit code `1`:

```text
[ERROR] assertion-reduction: assertion count or strength dropped in test_addition
   File: test_calculator.rs:6
   Remediation: preserve existing assertions or supply a scoped override directive
   Doc: https://orieg.github.io/discipline/gates/#assertion-reduction

Summary: 1 errors, 0 warnings, 0 overrides
Status: FAILED
```

---

## Step 6: Fix the Violation

Restore the rigorous test assertions or add additional boundary checks:

```bash
cat << 'EOF' > test_calculator.rs
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[test]
fn test_addition() {
    assert_eq!(add(2, 2), 4);
    assert_eq!(add(-1, 1), 0);
    assert_eq!(add(100, 200), 300);
}
EOF
```

Rerun Discipline:

```bash
discipline check --base main
```

Discipline reports success:

```text
Summary: 0 errors, 0 warnings, 0 overrides
Status: PASS
```

---

## Step 7: Clean Up

Remove the temporary sandbox directory when done:

```bash
cd /tmp
rm -rf /tmp/discipline-sandbox
```

---

## Next Steps

- **Configuration:** Learn how to customize gate rules, severities, and exemptions in [`docs/CONFIGURATION.md`](../CONFIGURATION.md).
- **Gate Catalog:** Explore normative specifications for all 30 gates in [`docs/GATES.md`](../GATES.md).
- **CI Integration:** Integrate Discipline into GitHub Actions, GitLab CI/CD, Forgejo, or Argo Workflows using the [CI/CD Platform Integration Guide](../guides/ci-platforms.md).
