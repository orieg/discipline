# AGENTS.md — Multi-Agent Engineering & Quality Guide for Discipline

Welcome to **Discipline** (`orieg/discipline`). This document establishes mandatory engineering, architectural, and safety standards for all autonomous AI coding agents interacting with this repository.

> **Canonical source.** `AGENTS.md` is the single canonical agent guide for this repo. `CLAUDE.md` and `GEMINI.md` are **symlinks** to this file. **Make every edit in `AGENTS.md` only** — editing a symlink target elsewhere would fork the guidance.

---

## 1. Project Mission & Identity

**Discipline** is a universal CI/CD gatekeeper and AI coding agent diff sentinel built in Rust. It transforms the verification rigors from high-assurance algorithm repositories into a single, declarative static binary wrapped in a composite GitHub, Gitea, and Forgejo Action.

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
| Gate specifications & rules | `docs/GATES.md` | Gate rule evolution or new gate addition |
| Configuration & integration reference | `docs/CONFIGURATION.md` | Configuration schema version bump or parameter change |
| Engine architecture & fail-closed contract | `docs/ARCHITECTURE.md` | Engine redesign or contract change |
| Project roadmap & milestones | `docs/ROADMAP.md` | Milestone completion or phase evolution |
| Agent rules & engineering standards | `AGENTS.md` (this file) | Project-level policy evolution |
| Reference configuration schema | `discipline.toml` | Schema version bump |
| Output schemas (generated) | `discipline.report.schema.json`, `discipline.replay.schema.json` (from `src/output_schema.rs`; `discipline docs --write`) | Report or replay field change |
| Action runner definition | `action.yml` | Action input/runtime change |
| GitLab CI component | `templates/discipline.gitlab-ci.yml` | Component interface or runner change |
| Argo Workflow template | `templates/argo-workflow-template.yaml` | Task spec or parameter change |
| Documentation site (GitHub Pages) | `docs/index.html` | UI, layout, or documentation updates |
| Pre-commit hook definitions | `.pre-commit-hooks.yaml` | Hook interface change |
| CI and release pipelines | `.github/workflows/`, `.gitea/workflows/`, `.forgejo/workflows/` (described in `docs/ARCHITECTURE.md` §8) | Pipeline redesign |
| Dependency policy | `deny.toml` | License or source policy change |

---

## 2. Core Principles & Strict Rules

### 2.1 No Time Estimates
Never include time estimates, durations, or week/sprint projections in plans, specifications, READMEs, proposals, review reports, expert-panel outputs, or any documentation or chat response.
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
All CI builds target `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` for container execution, and native macOS targets for local developer workflows. No gate needs the network. These features read a forge's REST API: open-issue state, bench citation freshness and `discipline doctor` platform checks (opt-in), the `merged-pr-body` directive source on a push event, and `discipline replay` reading each replayed change's merged pull-request body (the same lookup). One feature writes: `check --comment` (opt-in, off by default) posts and edits one pull-request comment, never follows a redirect on a write, and reports a token that cannot write as a note rather than a failure. They use the in-process HTTPS client in `src/forge.rs` (rustls, no OpenSSL), never an external tool, and fail closed (exit 2) when the forge cannot be reached. Do not add another network path, a dependency that pulls in OpenSSL, or a runtime dependency on an external binary. `DISCIPLINE_NO_NETWORK=1` must keep every request off the network.

### 3.4 Adding or Changing a Gate
A gate is not done until all of these hold (full contract: `docs/ARCHITECTURE.md` §3 and §9):
1. It has a stable kebab-case id in `src/config.rs::GATES` and its own `[gates.<id>]` table with `enabled`, `severity`, `exempt_paths`. A gate that is designed but not implemented is registered with `available: false`; it is never accepted as configuration.
2. It returns a `GateOutcome` with a truthful `examined` count and a `notes` entry for anything it could not verify. Every finding kind it reports is registered in `src/findings.rs` with a kebab-case code that names what it protects; the code is frozen once released, the title is not.
3. **Unit test** with a positive and a negative control; **end-to-end test** in `tests/test_gates_e2e.rs` driving the real binary; a **`self-test` case**.
4. **Mutation evidence:** break the detector, watch a test fail, restore it. State which test killed the mutant in the PR.
5. Any escape hatch goes through `src/tokens.rs` (line-anchored, placeholder-rejecting, scoped). No in-source override comments.
6. If a finding could echo a secret or a user name, report the location only.
7. `docs/GATES.md` and `README.md` list the gate with its true status.
8. Its new interface names (gate id, finding codes, configuration keys, directives) are recorded in `tests/fixtures/v1_surface.json` (`DISCIPLINE_BLESS_SURFACE=1 cargo test --test test_stability_contract`). A name recorded there is never removed or renamed before a major version (`docs/ARCHITECTURE.md` §3.2).

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
- Directives belong in the commit BODY. Never place directives in the commit subject line or PR title.
