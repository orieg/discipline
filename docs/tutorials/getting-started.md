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

Add a one-line agent guide (discipline's `agents-md` gate expects every repository to have one), then commit this baseline to the `main` branch:

```bash
echo "# Agent guide" > AGENTS.md
git add test_calculator.rs AGENTS.md
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

Discipline reads both versions of the test with a tree-sitter parser. The two `assert_eq!` calls are gone and `assert!(true)` cannot fail, so the test's effective assertions dropped from 2 to 0, and Discipline fails with exit code `1`. The report ends with (the full output also lists every gate):

```text
error [assertion-reduction] Assertion Reduction In Existing Test [test_calculator.rs:6]
   Test `test_addition`: effective assertions dropped from 2 to 0.
   Remediation: Restore the assertions, or justify the drop on its own line in the PR body or a commit message: `allow-assertion-drop: test_addition <reason>`.
   Doc: https://orieg.github.io/discipline/gates/#assertion-reduction

errors: 1  warnings: 0  overrides: 0
Status: FAILED

Tip: fix what this change introduced. Findings in code it did not touch (existing debt when adopting Discipline) can be recorded with 'discipline baseline --write'.
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
errors: 0  warnings: 0  overrides: 0
Status: PASS
```

---

## Step 7: Let Your Coding Agent Hear It

CI catches the weakening after the fact. An agent can be told while it is still editing. Install the hook for Claude Code (or `codex`, `cursor`, `aider`):

```bash
discipline hook install --agent claude-code
```

This writes `.claude/settings.json`, which runs `discipline hook run --agent claude-code` after every edit and before the agent stops. Weaken the test again, then run the hook the way Claude Code does, with its event on stdin:

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
echo '{"hook_event_name":"PostToolUse","tool_name":"Edit"}' | discipline hook run --agent claude-code
echo "exit: $?"
```

The hook exits `2`, which blocks the edit, and prints on stderr what the agent reads:

```text
Discipline gatekeeper detected violations in your changes. Please fix each issue:

### Issue 1 [assertion-reduction]: Assertion Reduction In Existing Test
- Location: test_calculator.rs:6
- Problem: Test `test_addition`: effective assertions dropped from 2 to 0.
- Repair: Restore the assertions that were removed or weakened to match or exceed the original assertion count.

exit: 2
```

The agent is told how to repair the test, not how to waive the finding, and the check is judged by `main`'s configuration: editing `discipline.toml` in the branch does not switch it off. The [Agent Hooks](../CONFIGURATION.md#agent-hooks) reference covers the other agents and the MCP server.

---

## Step 8: Clean Up

Remove the temporary sandbox directory when done:

```bash
cd /tmp
rm -rf /tmp/discipline-sandbox
```

---

## Next Steps

- **Configuration:** Learn how to customize gate rules, severities, and exemptions in [`docs/CONFIGURATION.md`](../CONFIGURATION.md).
- **Gate Catalog:** Explore the specification of every gate in [`docs/GATES.md`](../GATES.md).
- **CI Integration:** Integrate Discipline into GitHub Actions, GitLab CI/CD, Forgejo, or Argo Workflows using the [CI/CD Platform Integration Guide](../guides/ci-platforms.md).
