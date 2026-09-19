---
layout: default
title: Roadmap & Milestones
permalink: /roadmap/
---

# Discipline Roadmap & Milestones

This document establishes the delivery phases, dependency graph, go/no-go gates, shipped gates, planned extensions, and outstanding checks for `discipline`.

**Superseded when:** A milestone completes, a new phase is initiated, or planned gate scope evolves. Update in place; do not fork.

---

## Dependency Graph

```mermaid
flowchart TD
    P0["Phase 0: Fail-closed core"] --> P1["Phase 1: AST sentinel (Rust pack) + hygiene + configurability"]
    P1 --> P2["Phase 2: Packaging, CI, release"]
    P2 --> P3["Phase 3: Language packs I (Python, JS/TS, PHPT)"]
    P2 --> P4["Phase 4: Gate integrity & test floors"]
    P2 --> P5["Phase 5: Command gates & verification presets"]
    P3 --> P7["Phase 7: Language packs II (Java/Kotlin, C/C++, Go)"]
    P4 --> P6["Phase 6: Expanse dogfooding & script retirement"]
    P5 --> P6
```

Phases 3, 4, and 5 depend upon Phase 2 and proceed in parallel.

---

## Shipped Gates

<!-- generated:gates -->
| Gate | Suite | Languages | Rule |
|---|---|---|---|
| `agents-md` | agent-guard | any | AGENTS.md exists; CLAUDE.md / GEMINI.md do not fork it |
| `assertion-reduction` | agent-guard | Rust, Python, JS/TS, PHPT | assertion count / strength must not drop in an existing test |
| `vacuous-tests` | agent-guard | Rust, Python, JS/TS, PHPT | new tests must carry a non-tautological assertion |
| `ignored-tests` | agent-guard | Rust, Python, JS/TS, PHPT | tests must not be newly #[ignore]d |
| `unsafe-safety-comment` | agent-guard | Rust | unsafe blocks / impls carry a // SAFETY: comment |
| `deletion-rationale` | agent-guard | any | deleted files and removed tests need a scoped removes: rationale |
| `time-estimates` | hygiene | any | no calendar / duration estimates in markdown or the PR body |
| `pii` | hygiene | any | no home paths, LAN IPs, or denylisted hostnames in tracked text |
| `agent-scratch` | hygiene | any | agent scratch state is never tracked |
| `config-integrity` | integrity | any | a change cannot weaken its own discipline.toml without a token |
| `golden-output` | integrity | any | prevents stealth edits to committed golden/test output files without explicit override |
| `bench-regression` | bench | Rust, Go, Python, C/C++ | benchmark drift via harness adapters (deterministic counts or BCa intervals) |
<!-- /generated -->

---

## Milestone Phases & Go / No-Go Gates

### Phase 0: Fail-Closed Core
- **Deliverables:** Three-state exit codes (`0`, `1`, `2`), three-state git context, merge-base diff inspection, strict layered configuration resolution, stable gate registry, line-anchored directive parsing, truthful reporting with examined counts.
- **Go / no-go gate:** Every invariant rule in the Fail-Closed Contract (F1–F12) has discriminating tests that fail if the behavior is removed.
- **Status:** **Completed & Verified**.

### Phase 1: Sentinel, Hygiene, Configurability
- **Deliverables:** AST gates on Rust pack, whole-tree hygiene sweeps (`time-estimates`, `pii`, `agent-scratch`), `agents-md` guide sentinel, full 5-layer configurability, unanalysed source reporting.
- **Go / no-go gate:** Positive and negative unit controls, e2e binary tests, embedded self-test cases, and mutation verification.
- **Status:** **Completed & Verified**.

### Phase 2: Packaging, Multi-Platform CI, Release
- **Deliverables:** Composite `action.yml` for GitHub, Gitea, and Forgejo Actions; pre-commit hooks (`.pre-commit-hooks.yaml`); GitLab CI component template; Argo Workflow template; Dockerfile container; automated multi-architecture release pipeline (`release.yml`).
- **Go / no-go gate:** `ci-gate` 100% green across all platforms; release pipeline builds verified static musl and macOS binaries with SHA-256 attestations.
- **Status:** **Completed & Verified**.

### Phase 3: Language Packs I (Fact Model Split)
- **Deliverables:** Tree-sitter extractor trait and language pack abstraction; Python pack (`test_*`, `unittest`, pytest markers); JavaScript / TypeScript pack (Jest, Vitest, Mocha matchers and callbacks); Golden (PHPT) pack.
- **Go / no-go gate:** Each language pack satisfies 4-point test discipline in its own language, including callback test renaming and assertion weakening detection.
- **Status:** **Completed & Verified**.

### Phase 4: Gate Integrity & Test Floors
- **Deliverables:**
  - `ci-integrity`: Workflow diff inspection detecting dropped jobs from `needs`, addition of `continue-on-error`, unpinned third-party actions, or stripped `-D warnings`.
  - `test-floor`: Test-count ratchet with baseline count read from merge-base ref; `allow-test-shrink:` directive.
  - `suppression-delta`: Catching net increases in compiler/linter suppression annotations (`#[allow]`, `@ts-ignore`, `# type: ignore`, `// NOLINT`).
  - `scope-confinement`: Restricting agent file modifications strictly within authorized directory boundaries.
- **Go / no-go gate:** Incident replays of historical workflow weakening and all-skipped test rollups are rejected with exit `1`.
- **Status:** Planned.

### Phase 5: Verification Presets & Benchmark Adapters
- **Deliverables:**
  - `command`: Fail-closed execution wrapper for arbitrary external tools with canary checks, zero-test guards, and timeout limits.
  - Verification presets: `sanitizers` (ASan/TSan), `miri`, `msrv`, `unsafe-budget`.
  - Benchmark regression sentinel (`bench-regression` shipped, adding additional adapters for Go benchmarks, Google Benchmark, pytest-benchmark, Criterion, and Callgrind).
- **Go / no-go gate:** Zero-test runs, missing baseline artifacts, and unprovenanced runs fail closed; deterministic counts and conservative bootstrap intervals discriminate regressions without false positives.
- **Status:** `bench-regression` core shipped; command gates and remaining presets planned.

### Phase 6: Expanse Dogfooding
- **Deliverables:** Deployment of `discipline.toml` in `orieg/expanse`; phased retirement of bespoke shell verification scripts.
- **Go / no-go gate:** Incident replay fixtures reproducing historical bypasses are caught by Discipline without regressing verification fidelity.
- **Status:** Planned.

### Phase 7: Language Packs II
- **Deliverables:** Java / Kotlin pack (JUnit 5, AssertJ, Hamcrest); C / C++ pack (GoogleTest, Catch2); Go pack (`testing.T`, `testify`).
- **Go / no-go gate:** 100% discriminating test coverage per language pack.
- **Status:** Planned.

---

## Outstanding Checks & Known Limitations

- **No external review:** The architecture and test coverage were established through internal pairing and rigorous self-tests. External review by independent systems engineers is an outstanding verification check.
- **Runner environment testing:**
  - GitHub Actions: verified on hosted Linux and macOS runners in CI.
  - Gitea Actions: tested via `act` runner images; testing on a physical production Gitea server is outstanding.
  - Forgejo Actions: verified under local and CI runner environments; testing against enterprise Forgejo clusters is outstanding.
  - GitLab CI: reusable component template linted and schema-validated; live GitLab runner execution is outstanding.
  - Argo Workflows: template linted; live Kubernetes cluster DAG execution is outstanding.
- **Macro opacity:** Tests generated dynamically inside complex macro bodies (`proptest! { ... }`, `quickcheck! { ... }`) are invisible to tree-sitter AST fact extractors without compilation expansion. Use `extra_assert_macros` and `assert_helper_fns` to configure macro vocabulary.
- **Grammar lag:** Source syntax newer than the bundled tree-sitter grammars is treated as a parse error, failing closed by design. Use `exempt_paths` until grammars are updated.
- **Workflow-level weakening:** Edits to `.github/workflows/` made within the PR branch itself are unguarded until `ci-integrity` ships.
