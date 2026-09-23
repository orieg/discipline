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
- Shipping the signature and leaving `todo!()`, `raise NotImplementedError` or `return null` as the body.
- Wrapping a failure in an empty `catch {}` / `except: pass`, logging it and moving on, `@`-silencing a PHP call, `rescue nil`, `runCatching { }.getOrNull()`, or discarding a `Result`.
- Keeping the assertion but moving it under `if false` or after a `return`, or padding a `todo!()` with a log line so the body no longer looks empty.
- Swapping the real dependency for a mock and asserting only that the mock was called.
- Regenerating snapshots, adding a snapshot for a test that already existed, adding retries, or loosening `tsconfig.json`, `ruff.toml`, `clippy.toml`, `[lints]` and coverage floors — or swapping what they `extends` — instead of fixing the cause.
- Repointing a lockfile entry at another host or dropping its integrity hash, or trading `npm ci` / `--frozen-lockfile` for an install that rewrites the lock.
- Editing the gate configuration (`discipline.toml`), the CI workflow, or the agent's own instruction files (`AGENTS.md`, `.cursorrules`) to disable failing checks.
- Carrying text aimed at the next agent: an injection in a comment, a PR description or a commit message, or a bidirectional override that hides what a parser reads.
- Running a command on every install from where CI checks do not look: a `package.json` `postinstall`, a `build.rs`, or a repointed registry in `.npmrc` / `pip.conf`.
- Introducing unverified calendar estimates or leaking developer workstation paths and LAN IPs.

Discipline inspects the **diff** against the merge base using `tree-sitter` AST parsing and fail-closed verification rigors. It rejects erosion patterns before they reach review, and it guards its own trust boundary: a change cannot switch its run to advisory, disable the gate that judges its configuration, narrow the CI step that runs the gate without a recorded reason, or (with `directives.require_approval`) excuse itself without a review by someone else. On a push to the default branch, the waivers reviewed on the merged pull request still count: `merged-pr-body` reads that pull request's body through the forge, because a squash or rebase merge drops it from the commit message.

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

permissions:
  contents: read

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

The floating `@v0` ref tracks the latest `v0.x.y` release, and moves only after that release has passed its smoke tests. Before 1.0, a minor release can change gate behaviour; each one lists those changes under "Upgrading" in its release notes and in the [compatibility ledger](docs/ROADMAP.md#default-changes-compatibility-ledger). Pin `@v0.10.2` when a verdict must be reproducible from the workflow file alone.

> **Note on `edited`:** GitHub Actions does not trigger workflows on PR description edits by default. Specifying `types: [opened, synchronize, reopened, edited]` ensures that updating the PR body (such as adding an authorized override directive or resolving a PR-body hygiene finding) immediately re-runs the gate without requiring an empty commit.
>
> **Note on `push`:** the action also runs on `push` events (base `github.event.before`), but a push run reads directives from the pushed commits' messages only: a squash or rebase merge drops the PR body, so a waiver that passed on the pull request fails the push run on `main` that follows. Gate on `pull_request` as above; if you also run on `push`, give the run a token that can read pull requests (the `merged-pr-body` source then reads the merged pull request's body and names it in the notes) or put directives in commit messages too. Details: [Override Directives](docs/CONFIGURATION.md#override-directives).

A check only blocks a merge when the branch requires it. See [Repository Protection](docs/guides/ci-platforms.md#8-repository-protection) for the settings (required check, up-to-date branch, no bypass, CODEOWNERS on gate configuration) and a ruleset example.

### 4. Pre-Merge CI Sentinel (Gitea & Forgejo Actions)

The same composite action runs unchanged under Gitea's `act_runner` and under `forgejo-runner`. Reference it by its full URL, since the runner resolves a bare `owner/repo` against your own instance. Create `.gitea/workflows/discipline.yml` (or `.forgejo/workflows/discipline.yml`):

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
      - uses: https://github.com/orieg/discipline@v0
        with:
          fail_on_warnings: true
```

The action fetches the matching release archive and its `SHA256SUMS` from GitHub and verifies the checksum before running, so the runner needs outbound HTTPS to `github.com`. On an air-gapped runner, point `binary_path` at a discipline binary already on the runner image (see [Installation](#installation)), or set `download_url` to an internal mirror of the release store:

```yaml
      - uses: https://github.com/orieg/discipline@v0
        with:
          binary_path: /opt/discipline/discipline
```

The `runs-on` label must map to a runner image with `git`, `curl` (or `wget`) and Node.js (`actions/checkout` is a JavaScript action; the default `act_runner` images qualify). The action itself is shell-only. Gitea and Forgejo name the resulting status check `CI Sentinel / discipline (pull_request)`; use that name when making it a required check on the default branch. `discipline doctor` reads a `GITEA_TOKEN` or `FORGEJO_TOKEN` to confirm the protection is in place. See the [Forgejo & Gitea Actions guide](docs/guides/ci-platforms.md#3-forgejo--gitea-actions) for the full walkthrough.

For other CI platforms and orchestrators (copy-paste pipelines for GitLab, Argo, Azure Pipelines, Bitbucket, CircleCI and Jenkins are in [`templates/`](templates/), indexed in the [CI guide](docs/guides/ci-platforms.md#7-other-ci-platforms-templates)):
- [GitLab CI/CD Component & Job Guide](docs/CONFIGURATION.md#gitlab-ci-cd)
- [Forgejo & Gitea Actions Guide](docs/guides/ci-platforms.md#3-forgejo--gitea-actions)
- [Argo Workflows GitOps Template](docs/CONFIGURATION.md#argo-workflows)
- [pre-commit & Local Git Hooks](docs/CONFIGURATION.md#pre-commit-hook)
- [Agent Hooks (Claude Code, Codex, Cursor, Aider)](docs/CONFIGURATION.md#agent-hooks)
- [MCP Server (`discipline mcp`)](docs/CONFIGURATION.md#mcp-server)
- [Docker Container Run](docs/CONFIGURATION.md#docker-container)
- [CLI Reference & Local Inner Loop](docs/CONFIGURATION.md#standalone-cli)

## Gates

Eleven tree-sitter language packs (Rust, Python, JavaScript / TypeScript, PHPT, Java, Go, PHP, C / C++, C#, Ruby, Kotlin) supply the AST facts; the language table in [`docs/GATES.md`](docs/GATES.md) lists what each pack reads. `discipline gates` prints this table with each gate's effective state. Detailed rules, detection boundaries, and what gates do not catch are documented in [`docs/GATES.md`](docs/GATES.md). Gates ship on unless their rule is a repository policy (`issue-link`, `commit-provenance`, `provenance-tags`, `scope-confinement`, `pr-checklist`, the verification presets); `docs/ROADMAP.md` records every default change per release.

<!-- generated:gates -->
| Gate | Suite | Languages | Rule Description |
|---|---|---|---|
| [`agents-md`](docs/GATES.md#agents-md) | agent-guard | any | AGENTS.md exists; CLAUDE.md / GEMINI.md do not fork it |
| [`assertion-reduction`](docs/GATES.md#assertion-reduction) | agent-guard | Rust, Python, JS/TS, PHPT, Java, Go, PHP, C/C++, C#, Ruby, Kotlin | assertion count / strength must not drop in an existing test |
| [`vacuous-tests`](docs/GATES.md#vacuous-tests) | agent-guard | Rust, Python, JS/TS, PHPT, Java, Go, PHP, C/C++, C#, Ruby, Kotlin | new tests must carry a non-tautological assertion |
| [`ignored-tests`](docs/GATES.md#ignored-tests) | agent-guard | Rust, Python, JS/TS, PHPT, Java, Go, PHP, C/C++, C#, Ruby, Kotlin | tests must not be newly #[ignore]d or skipped without directive |
| [`unsafe-safety-comment`](docs/GATES.md#unsafe-safety-comment) | agent-guard | Rust | unsafe blocks / impls carry a // SAFETY: comment |
| [`deletion-rationale`](docs/GATES.md#deletion-rationale) | agent-guard | any | deleted files and removed tests need a scoped removes: rationale |
| [`time-estimates`](docs/GATES.md#time-estimates) | hygiene | any | no calendar / duration estimates in markdown or the PR body |
| [`pii`](docs/GATES.md#pii) | hygiene | any | no home paths, LAN IPs, or denylisted hostnames in tracked text |
| [`agent-scratch`](docs/GATES.md#agent-scratch) | hygiene | any | agent scratch state is never tracked |
| [`shell-secrets`](docs/GATES.md#shell-secrets) | hygiene | shell, docker, workflows | no command-line secrets or unverified piped scripts in shell, docker, or CI |
| [`issue-link`](docs/GATES.md#issue-link) | hygiene | any | PR title or description links a tracking issue (#123, Fixes #123) |
| [`commit-provenance`](docs/GATES.md#commit-provenance) | hygiene | any | commits carry the required trailers; an agent-produced commit carries a review by someone else |
| [`config-integrity`](docs/GATES.md#config-integrity) | integrity | any | a change cannot weaken its own discipline.toml without a token |
| [`stub-bodies`](docs/GATES.md#stub-bodies) | agent-guard | Rust, Python, JS/TS, Go, Java, C#, PHP, Ruby, C/C++, Kotlin | added functions are not stubs; existing bodies are not replaced by todo!() / NotImplementedError / return null |
| [`error-swallowing`](docs/GATES.md#error-swallowing) | agent-guard | Rust, Python, JS/TS, Go, Java, C#, PHP, Ruby, C/C++, Kotlin | no new empty error handler or discarded Result outside tests |
| [`instruction-smuggling`](docs/GATES.md#instruction-smuggling) | agent-guard | any (invisible characters, instruction files); Rust, Python, JS/TS, Go, Java, C#, PHP, Ruby, C/C++, Kotlin and prose files (phrases) | no invisible Unicode, unreviewed agent-instruction edits, or instruction-like text in comments and prose |
| [`build-hooks`](docs/GATES.md#build-hooks) | integrity | package.json, build.rs, setup.py, .npmrc, .pypirc, pip.conf, .cargo/config.toml, .env* | install and build hooks that gain network or shell access, and package-manager configuration edits, need a token |
| [`toolchain-config`](docs/GATES.md#toolchain-config) | integrity | tsconfig, ruff, mypy, pytest, coverage, flake8, Cargo lints, rustflags, nextest, eslintrc, golangci, jest, codecov, phpstan, phpunit | compiler, linter, type-checker, test-runner and coverage configuration cannot be loosened without a token |
| [`scope-confinement`](docs/GATES.md#scope-confinement) | agent-guard | any | changes stay inside authorized paths |
| [`suppression-delta`](docs/GATES.md#suppression-delta) | agent-guard | per pack | newly added linter / compiler suppression annotations |
| [`provenance-tags`](docs/GATES.md#provenance-tags) | hygiene | any | published numerics carry (measured|target|projected) |
| [`ci-integrity`](docs/GATES.md#ci-integrity) | integrity | any | workflow weakening: continue-on-error, || true, unpinned actions |
| [`ci-skip-set`](docs/GATES.md#ci-skip-set) | integrity | any | rollup skip set matches each job's `if:` under the observed filter outputs |
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
bash install.sh --to ~/.local/bin --version v0.10.2
```

### Debian / Ubuntu (APT)

Add the official APT repository and install via `apt`:

```bash
# 1. Install the repository signing key and add the source
curl -fsSL https://orieg.github.io/discipline/apt/discipline-archive-keyring.gpg | sudo tee /usr/share/keyrings/discipline-archive-keyring.gpg >/dev/null
echo "deb [signed-by=/usr/share/keyrings/discipline-archive-keyring.gpg] https://orieg.github.io/discipline/apt/ stable main" | sudo tee /etc/apt/sources.list.d/discipline.list

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
  brew install orieg/tap/discipline
  ```
- **MacPorts**: each release attaches a generated `Portfile`. Until the port is in the MacPorts tree, install it from a local ports tree:
  ```bash
  mkdir -p ~/ports/devel/discipline
  curl -fsSL -o ~/ports/devel/discipline/Portfile https://github.com/orieg/discipline/releases/latest/download/Portfile
  # Once: add `file:///Users/<you>/ports` above the rsync line in /opt/local/etc/macports/sources.conf
  (cd ~/ports && portindex) && sudo port install discipline
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

### Shell Completions

The APT, RPM, Homebrew and MacPorts packages install completions for bash, zsh and fish; nothing to do. A release tarball carries the same scripts under `completions/`, and any binary can regenerate them with `discipline completions <shell>`. For a tarball, `install.sh` or `cargo install` setup, put the script where the shell loads it:

```bash
# bash (bash-completion loads this directory)
discipline completions bash > ~/.local/share/bash-completion/completions/discipline
```

```bash
# zsh: a directory on $fpath, before `compinit` runs (add `fpath+=~/.zfunc` to ~/.zshrc if it is new)
mkdir -p ~/.zfunc && discipline completions zsh > ~/.zfunc/_discipline
```

```bash
# fish
discipline completions fish > ~/.config/fish/completions/discipline.fish
```

PowerShell and Elvish are generated the same way (`discipline completions powershell`, `discipline completions elvish`). The script describes the subcommands and flags of the binary that produced it: regenerate it after every upgrade, and clear zsh's cache (`rm -f ~/.zcompdump*`) when a new subcommand does not complete.

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
