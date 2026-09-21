# discipline

[![CI](https://github.com/orieg/discipline/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/orieg/discipline/actions/workflows/ci.yml)
[![Documentation](https://img.shields.io/badge/docs-orieg.github.io%2Fdiscipline-blue.svg)](https://orieg.github.io/discipline/)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-blue.svg)](#license)
[![Rust 1.90+](https://img.shields.io/badge/rustc-1.90%2B-orange.svg)](Cargo.toml)
[![Release](https://img.shields.io/github/v/release/orieg/discipline?logo=github)](https://github.com/orieg/discipline/releases)
[![Marketplace](https://img.shields.io/badge/Marketplace-Discipline%20CI%20Gate-blue?logo=github-actions)](https://github.com/marketplace/actions/discipline-ci-gate)
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

Discipline inspects the **diff** against the merge base using `tree-sitter` AST parsing and fail-closed verification rigors. It rejects erosion patterns before they reach review.

## Quickstart

### 1. Local Developer & Agent Inner Loop

Install the binary and inspect your working tree before pushing:

```bash
# Install static binary
curl -fsSL https://orieg.github.io/discipline/install.sh | bash

# Inspect uncommitted changes against HEAD across all gates
discipline diff

# Or inspect current branch diff against origin/main
discipline check
```

### 2. Adopting on an Existing Repository (Brownfield)

Adopting Discipline on an existing repository never requires resolving all historical debt upfront. Discipline provides cryptographic grandfathering baselines that pin existing findings so current code passes:

```bash
# Step 1: Baseline existing findings so current repository state passes
discipline baseline --write

# Step 2: Commit baseline file and enable Discipline
git add discipline-baseline.toml && git commit -m "chore: baseline existing discipline debt"

# All future diffs and pull requests are now strictly guarded against new regressions!
```

### 3. Pre-Merge CI Sentinel (GitHub Actions)

Add Discipline as a required check on pull requests:

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
      - uses: orieg/discipline@v0
        with:
          fail_on_warnings: true
```

The floating `@v0` ref automatically tracks the latest `v0.x.y` release while pinning against breaking changes. Use `@v0.5.1` if you require immutable release tag pinning.

> **Note on `edited`:** GitHub Actions does not trigger workflows on PR description edits by default. Specifying `types: [opened, synchronize, reopened, edited]` ensures that updating the PR body (such as adding an authorized override directive or resolving a PR-body hygiene finding) immediately re-runs the gate without requiring an empty commit.

For other CI platforms and orchestrators:
- [GitLab CI/CD Component & Job Guide](docs/CONFIGURATION.md#gitlab-ci-cd)
- [Forgejo & Gitea Actions Guide](docs/CONFIGURATION.md#forgejo-actions)
- [Argo Workflows GitOps Template](docs/CONFIGURATION.md#argo-workflows)
- [pre-commit & Local Git Hooks](docs/CONFIGURATION.md#pre-commit-hook)
- [Docker Container Run](docs/CONFIGURATION.md#docker-container)
- [CLI Reference & Local Inner Loop](docs/CONFIGURATION.md#standalone-cli)

## Gates

`discipline gates` prints this table with each gate's effective state. Detailed rules, detection boundaries, and what gates do not catch are documented in [`docs/GATES.md`](docs/GATES.md).

<!-- generated:gates -->
| Gate | Suite | Languages | Rule Description |
|---|---|---|---|
| [`agents-md`](docs/GATES.md#agents-md) | agent-guard | any | AGENTS.md exists; CLAUDE.md / GEMINI.md do not fork it |
| [`assertion-reduction`](docs/GATES.md#assertion-reduction) | agent-guard | Rust, Python, JS/TS, PHPT, Java, Go, PHP, C/C++, C#, Ruby | assertion count / strength must not drop in an existing test |
| [`vacuous-tests`](docs/GATES.md#vacuous-tests) | agent-guard | Rust, Python, JS/TS, PHPT, Java, Go, PHP, C/C++, C#, Ruby | new tests must carry a non-tautological assertion |
| [`ignored-tests`](docs/GATES.md#ignored-tests) | agent-guard | Rust, Python, JS/TS, PHPT, Java, Go, PHP, C/C++, C#, Ruby | tests must not be newly #[ignore]d or skipped without directive |
| [`unsafe-safety-comment`](docs/GATES.md#unsafe-safety-comment) | agent-guard | Rust | unsafe blocks / impls carry a // SAFETY: comment |
| [`deletion-rationale`](docs/GATES.md#deletion-rationale) | agent-guard | any | deleted files and removed tests need a scoped removes: rationale |
| [`time-estimates`](docs/GATES.md#time-estimates) | hygiene | any | no calendar / duration estimates in markdown or the PR body |
| [`pii`](docs/GATES.md#pii) | hygiene | any | no home paths, LAN IPs, or denylisted hostnames in tracked text |
| [`agent-scratch`](docs/GATES.md#agent-scratch) | hygiene | any | agent scratch state is never tracked |
| [`shell-secrets`](docs/GATES.md#shell-secrets) | hygiene | shell, docker, workflows | no command-line secrets or unverified piped scripts in shell, docker, or CI |
| [`issue-link`](docs/GATES.md#issue-link) | hygiene | any | PR title or description links a tracking issue (#123, Fixes #123) |
| [`config-integrity`](docs/GATES.md#config-integrity) | integrity | any | a change cannot weaken its own discipline.toml without a token |
| [`scope-confinement`](docs/GATES.md#scope-confinement) | agent-guard | any | changes stay inside authorized paths |
| [`suppression-delta`](docs/GATES.md#suppression-delta) | agent-guard | per pack | new #[allow], commented-out tests, cfg-gated tests |
| [`provenance-tags`](docs/GATES.md#provenance-tags) | hygiene | any | published numerics carry (measured|target|projected) |
| [`ci-integrity`](docs/GATES.md#ci-integrity) | integrity | any | workflow weakening: continue-on-error, || true, unpinned actions |
| [`test-floor`](docs/GATES.md#test-floor) | integrity | any | test-count ratchet read from the base ref |
| [`golden-output`](docs/GATES.md#golden-output) | integrity | any | prevents stealth edits to committed golden/test output files without explicit override |
| [`dependency-delta`](docs/GATES.md#dependency-delta) | integrity | any | manifest diff inspection: zero wildcards, source/license allowlists, and deny.toml verification |
| [`test-budget`](docs/GATES.md#test-budget) | integrity | Rust, Python, JS/TS, Go, any | property-test and fuzz effort ratchet (cases, shrink iters, fuzztime, seed corpus) |
| [`pr-checklist`](docs/GATES.md#pr-checklist) | hygiene | any | ticked PR checkboxes are reconciled against the diff |
| [`command`](docs/GATES.md#command) | verification | any | fail-closed wrapper for any tool: zero-tests guard, canary, count ratchet |
| [`sanitizers`](docs/GATES.md#sanitizers) | verification | Rust, C/C++ | ASan / TSan preset with audited suppressions and a race canary |
| [`msrv`](docs/GATES.md#msrv) | quality | Rust | cargo check under the pinned MSRV |
| [`miri`](docs/GATES.md#miri) | verification | Rust | Miri tiers with zero-tests guard |
| [`unsafe-budget`](docs/GATES.md#unsafe-budget) | verification | Rust | unsafe count ratchet |
| [`bench-regression`](docs/GATES.md#bench-regression) | bench | Rust, Go, Python, C/C++ | benchmark drift via harness adapters (deterministic counts or BCa intervals) |
| [`archive-contents`](docs/GATES.md#archive-contents) | integrity | any | distribution archive must contain required paths and zero forbidden developer artifacts |
| [`manifest-sync`](docs/GATES.md#manifest-sync) | integrity | any | reconcile git-tracked files against packaging manifest declarations |
| [`version-lockstep`](docs/GATES.md#version-lockstep) | integrity | any | version declarations across headers, manifests, and files must remain in lockstep |
<!-- /generated -->

## Installation

Discipline is distributed as a standalone static binary, native operating system packages (APT, RPM, Homebrew, MacPorts), cargo toolchain binary, or container image:

### Quick Install (Linux & macOS)

Install the latest pre-compiled static binary verified with cryptographic SHA-256 checksums:

```bash
# Recommended (download, inspect, and run):
curl -fsSL -o install.sh https://orieg.github.io/discipline/install.sh
bash install.sh
```

Alternatively, install via one-liner (the installer script internally enforces SHA-256 checksum verification):

```bash
curl -fsSL https://orieg.github.io/discipline/install.sh | bash
```

Custom destination directory or pinned release tag:

```bash
bash install.sh --to ~/.local/bin --version v0.5.1
```

### Debian / Ubuntu (APT)

Add the official APT repository and install via `apt`:

```bash
# 1. Add repository source
echo "deb [trusted=yes] https://orieg.github.io/discipline/apt/ stable main" | sudo tee /etc/apt/sources.list.d/discipline.list

# 2. Update package cache and install
sudo apt update
sudo apt install -y discipline
```

Direct `.deb` package downloads and repository metadata: [Discipline APT Repository](https://orieg.github.io/discipline/apt/).

### Enterprise Linux / Fedora (RPM)

Add the official RPM repository and install via `dnf` or `yum`:

```bash
# 1. Add repository configuration
sudo dnf config-manager --add-repo https://orieg.github.io/discipline/rpm/discipline.repo

# 2. Install discipline binary
sudo dnf install -y discipline
```

Direct `.rpm` package downloads and repodata manifests: [Discipline RPM Repository](https://orieg.github.io/discipline/rpm/).

### macOS (Homebrew & MacPorts)

- **Homebrew**:
  ```bash
  # Single-command install:
  brew install orieg/tap/discipline

  # Or tap first:
  brew tap orieg/tap
  brew install discipline
  ```
- **MacPorts**:
  ```bash
  sudo port install discipline
  ```

### Rust Toolchain

- **`cargo-binstall`** (pre-compiled binary fetch):
  ```bash
  cargo binstall discipline
  ```
- **`cargo install`** (from source via git, requires `rustc` 1.80+):
  ```bash
  cargo install --git https://github.com/orieg/discipline
  ```

### Binary Verification & Provenance

Each release publishes pre-compiled binaries with SHA-256 checksums and SLSA Build Level 2 attestations:

```bash
gh attestation verify discipline-x86_64-unknown-linux-musl.tar.gz --repo orieg/discipline
```

## Documentation

- [Interactive Documentation Site](https://orieg.github.io/discipline/)
- [Sandbox Quickstart Tutorial](docs/tutorials/getting-started.md) — Hands-on walkthrough triggering and resolving an AST violation
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
