# AGENTS.md — Multi-Agent Engineering & Quality Guide for Discipline

Welcome to **Discipline** (`orieg/discipline`). This document establishes mandatory engineering, architectural, and safety standards for all autonomous AI coding agents interacting with this repository.

> **Canonical source.** `AGENTS.md` is the single canonical agent guide for this repo. `CLAUDE.md` and `GEMINI.md` are **symlinks** to this file. **Make every edit in `AGENTS.md` only** — editing a symlink target elsewhere would fork the guidance.

---

## 1. Project Mission & Identity

**Discipline** is a universal CI/CD gatekeeper and AI coding agent diff sentinel built in Rust. It transforms the verification rigors from high-assurance algorithm repos (such as `orieg/expanse`) into a single, declarative static binary wrapped in a composite GitHub, Gitea, and Forgejo Action.

Its primary mandate is to **prevent agent drift and test erosion**:
- Disallowing assertion weakening (`assert_eq!` -> `assert!` or removal).
- Rejecting vacuous or ghost tests.
- Mandating explicit `// SAFETY:` justifications on all `unsafe` blocks.
- Guarding against stealth deletions of tests and benchmarks.
- Tracking CPU cycle and instruction regressions with statistical rigor.
- Enforcing repository and research hygiene (no time estimates, no PII/host leaks, documented provenance).

### Canonical Documentation Hierarchy
Do not scatter notes into arbitrary files. Update canonical documents; do not proliferate `.md` files.

| Content | Canonical Home | Superseded When |
|---|---|---|
| Product requirements & roadmap | `docs/PRD.md` | Major architecture or scope shift |
| AST & diff engine architecture | `docs/ARCHITECTURE.md` | Engine redesign |
| Agent rules & engineering standards | `AGENTS.md` (this file) | Project-level policy evolution |
| Reference configuration schema | `discipline.toml` | Schema version bump |
| Action runner definition | `action.yml` | Action input/runtime change |
| GitLab CI component | `templates/discipline.gitlab-ci.yml` | Component interface or runner change |
| Argo Workflow template | `templates/argo-workflow-template.yaml` | Task spec or parameter change |
| Documentation site (GitHub Pages) | `docs/index.html` | UI, layout, or documentation updates |
| Pre-commit hook definitions | `.pre-commit-hooks.yaml` | Hook interface change |
| CI and release pipelines | `.github/workflows/`, `.gitea/workflows/`, `.forgejo/workflows/` (described in `docs/PRD.md` §8) | Pipeline redesign |
| Dependency policy | `deny.toml` | License or source policy change |

---

## 2. Core Principles & Strict Rules

### 2.1 No Time Estimates
Never include time estimates, durations, or week/sprint projections in plans, PRDs, READMEs, proposals, review reports, expert-panel outputs, or any documentation or chat response.
- **Banned:** "1-2 days", "3 weeks", "next sprint", "Phase 2 (1 week)", "~10 engineer-days". <!-- discipline:allow(time-estimates) -->
- **Allowed substitutes:** ordering ("Phase 1 ... Phase 2"), dependencies ("blocked on X"), relative size ("smallest of the three"), gate criteria ("ships when all AST tests pass"), parallelism.

### 2.2 Strict Vocabulary: Avoid "Substrate"
Avoid the term "substrate" when discussing architectures, runtimes, or systems. Use the precise technical layer:
- Engine, AST parser, grammar, execution environment, container, runner, virtual machine, memory hierarchy.

### 2.3 AST-Aware, Never RegEx-Naive
Never rely on loose substring regex matches to verify code invariants. A gate searching for `assert!` can be satisfied by a comment or log statement. All diff inspections must use `tree-sitter` AST traversal to ensure the node is an active, executable expression.

### 2.4 Fail-Closed Gates
Any command or gate that fails to execute, encounters a parsing error, or receives malformed configuration MUST exit non-zero. An empty diff result from a failed command is a failure, not a pass.

### 2.5 Label Every Claim RUN or READ
Every statement about behavior carries how it was established:
- **RUN** — you executed it and observed the output. Cite the command and output.
- **READ** — you inferred it from source code.

### 2.6 A Test Must Discriminate
A new test is not evidence until you have watched it fail for the right reason. Demonstrate it failing on the unmodified or inverted condition, then show it passing with the fix.

---

## 3. Rust Engineering Standards

### 3.1 Quality Gates
Every contribution must satisfy:
1. `cargo fmt --check` — Zero formatting deviations.
2. `cargo clippy --all-targets -- -D warnings` — Zero clippy warnings.
3. `cargo test` — 100% pass rate across unit and integration tests.
4. Clean git working copy before and after runs.

### 3.2 Unsafe Code Policy
`orieg/discipline` is a high-assurance tool. Unsafe code is strictly forbidden in core logic unless fundamentally required for low-level FFI bindings (e.g. `libgit2` or C tree-sitter grammars). When required, every `unsafe` block MUST be preceded by a detailed `// SAFETY:` invariant comment.

### 3.3 Static Binary Parity
All CI builds target `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` for container execution, and native macOS targets for local developer workflows. The binary has no network features; do not add a dependency that pulls in openssl or a TLS stack.

### 3.4 Adding or Changing a Gate
A gate is not done until all of these hold (full contract: `docs/PRD.md` §3 and §9):
1. It has a stable kebab-case id in `src/config.rs::GATES` and its own `[gates.<id>]` table with `enabled`, `severity`, `exempt_paths`. A gate that is designed but not implemented is registered with `available: false`; it is never accepted as configuration.
2. It returns a `GateOutcome` with a truthful `examined` count and a `notes` entry for anything it could not verify.
3. **Unit test** with a positive and a negative control; **end-to-end test** in `tests/test_gates_e2e.rs` driving the real binary; a **`self-test` case**.
4. **Mutation evidence:** break the detector, watch a test fail, restore it. State which test killed the mutant in the PR.
5. Any escape hatch goes through `src/tokens.rs` (line-anchored, placeholder-rejecting, scoped). No in-source override comments.
6. If a finding could echo a secret or a user name, report the location only.
7. `docs/PRD.md` §6 and `README.md` list the gate with its true status.

### 3.5 Workflow and Action Rules
- Pin third-party actions by commit SHA; pin downloaded tools by version and checksum.
- No `${{ }}` interpolation inside a `run:` block of `action.yml`; pass values through `env:` (`tests/action/lint-action.py` enforces this).
- An inverted canary asserts the diagnostic (the gate ids in the JSON report, `install_error`), never just a non-zero exit.
- The `ci-gate` rollup is an allow-list: a new job must be added to its `needs` and to the asserted job count.

### 3.6 Dependency Policy
`deny.toml` is an allow-list. A new dependency whose license is outside it is replaced or justified in the PR, not waved through by widening the list.

---

## 4. Git & PR Discipline

- Commit messages follow Conventional Commits: `type(scope): description`.
- Atomic, purposeful commits.
- Never commit agent scratch state (`.claude/`, `.gemini/`, `.antigravity/`, `scratch/`, `*.session.*`).
- Never leak local paths (`/Users/...`, `/home/...`) or LAN IPs in committed files or PR bodies.
- Never name private repositories, internal hostnames, or unreleased projects in committed files.
