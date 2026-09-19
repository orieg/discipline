# discipline

[![CI](https://github.com/orieg/discipline/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/orieg/discipline/actions/workflows/ci.yml)
[![Documentation](https://img.shields.io/badge/docs-orieg.github.io%2Fdiscipline-blue.svg)](https://orieg.github.io/discipline/)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-blue.svg)](#license)
[![Rust 1.90+](https://img.shields.io/badge/rustc-1.90%2B-orange.svg)](Cargo.toml)
[![Status](https://img.shields.io/badge/status-pre--release-yellow.svg)](docs/ROADMAP.md)
[![pre-commit](https://img.shields.io/badge/pre--commit-enabled-brightgreen?logo=pre-commit&logoColor=white)](.pre-commit-hooks.yaml)

**Universal CI/CD diff sentinel and AI coding agent gatekeeper built in Rust.** One static binary, identical in GitHub Actions, GitLab CI/CD, Forgejo Actions, Gitea Actions, Argo Workflows, pre-commit hooks, and an agent's local inner loop. Interactive guides and documentation: [orieg.github.io/discipline](https://orieg.github.io/discipline/).

## Why Discipline?

Autonomous coding agents operating in iterate-until-green loops optimize for passing tests, not preserving invariants:
- Weakening assertions (`assert_eq!(a, b)` &rarr; `assert!(a == b)` or `assert!(true)`).
- Adding vacuous "ghost tests" that compile and run but assert nothing.
- Marking failing or flaky tests `#[ignore]` / `@pytest.mark.skip` / `it.skip`.
- Stealthily deleting tests, fixtures, or benchmarks that stand in the way of a green build.
- Dropping `// SAFETY:` justifications from `unsafe` blocks.
- Editing the gate configuration (`discipline.toml`) to disable failing checks.
- Introducing unverified calendar estimates or leaking developer workstation paths and LAN IPs.

Discipline inspects the **diff** against the merge base using `tree-sitter` AST parsing and fail-closed engineering distilled from [`orieg/expanse`](https://github.com/orieg/expanse). It rejects erosion patterns before they reach review.

> **Status: pre-release.** No version tag is published yet. Code examples pin the verified commit SHA `d77059689f7fda42cc52a2b17ea8dd118af2e6f6` or use `binary_path: ...`. Upon release, snippets will track `@v0` / `v0.1.0`.

## Gates

`discipline gates` prints this table with each gate's effective state. Detailed rules, detection boundaries, and what gates do not catch are documented in [`docs/GATES.md`](docs/GATES.md).

<!-- generated:gates -->
| Gate | Suite | Languages | Rule |
|---|---|---|---|
| `agents-md` | agent-guard | any | AGENTS.md exists; CLAUDE.md / GEMINI.md do not fork it |
| `assertion-reduction` | agent-guard | Rust, Python, JS/TS, PHPT, Java, Go | assertion count / strength must not drop in an existing test |
| `vacuous-tests` | agent-guard | Rust, Python, JS/TS, PHPT, Java, Go | new tests must carry a non-tautological assertion |
| `ignored-tests` | agent-guard | Rust, Python, JS/TS, PHPT, Java, Go | tests must not be newly #[ignore]d |
| `unsafe-safety-comment` | agent-guard | Rust | unsafe blocks / impls carry a // SAFETY: comment |
| `deletion-rationale` | agent-guard | any | deleted files and removed tests need a scoped removes: rationale |
| `time-estimates` | hygiene | any | no calendar / duration estimates in markdown or the PR body |
| `pii` | hygiene | any | no home paths, LAN IPs, or denylisted hostnames in tracked text |
| `agent-scratch` | hygiene | any | agent scratch state is never tracked |
| `config-integrity` | integrity | any | a change cannot weaken its own discipline.toml without a token |
| `golden-output` | integrity | any | prevents stealth edits to committed golden/test output files without explicit override |
| `bench-regression` | bench | Rust, Go, Python, C/C++ | benchmark drift via harness adapters (deterministic counts or BCa intervals) |
<!-- /generated -->

Seven gates inspect text, diffs, or repository metadata across any language. The four AST gates use per-language packs (Rust, Python, JavaScript / TypeScript, PHPT) with Go, Java/Kotlin, and C/C++ planned. `bench-regression` tracks micro-benchmarks with interval degradation when sampling distributions lack confidence bounds.

## Quickstart

Add Discipline as a pre-merge check in GitHub Actions:

```yaml
name: CI Sentinel

on:
  pull_request:
    types: [opened, synchronize, reopened, edited]

jobs:
  discipline:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          fetch-depth: 0 # merge base must be reachable
      - uses: orieg/discipline@d77059689f7fda42cc52a2b17ea8dd118af2e6f6
        with:
          fail_on_warnings: true
```

> **Note on `edited`:** GitHub Actions does not trigger workflows on PR description edits by default. Specifying `types: [opened, synchronize, reopened, edited]` ensures that updating the PR body (such as adding a `removes:` directive or resolving a PR-body hygiene finding) immediately re-runs the gate without requiring an empty commit.

For all other platforms, see the dedicated platform guides:
- [GitLab CI/CD Component & Job Guide](docs/CONFIGURATION.md#gitlab-ci-cd)
- [Forgejo & Gitea Actions Guide](docs/CONFIGURATION.md#forgejo-actions)
- [Argo Workflows GitOps Template](docs/CONFIGURATION.md#argo-workflows)
- [pre-commit & Local Git Hooks](docs/CONFIGURATION.md#pre-commit-hook)
- [Docker Container Run](docs/CONFIGURATION.md#docker-container)
- [CLI Reference & Local Inner Loop](docs/CONFIGURATION.md#standalone-cli)

## Documentation

- [Interactive Documentation Site](https://orieg.github.io/discipline/)
- [Gate Specifications & Enforcement Rules](docs/GATES.md) — Normative rules, before/after diffs, and what gates do not catch
- [Configuration Reference](docs/CONFIGURATION.md) — 5-layer hierarchy, schema, action inputs/outputs, and platform guides
- [Engine Architecture & Sentinel Design](docs/ARCHITECTURE.md) — Fail-closed contracts (F1–F12), AST diff engine, and CI pipelines
- [Roadmap & Milestone Tracking](docs/ROADMAP.md) — Delivery phases, dependency graph, and planned gates

## Development

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

Every gate requires a unit test, an end-to-end test through the binary, a mutation test that kills the mutant, and a `self-test` case; see [`AGENTS.md`](AGENTS.md) §3.4.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
