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
    P4 --> P6["Phase 6: Production dogfooding & legacy script retirement"]
    P5 --> P6
    P4 --> P8["Phase 8: Agent-evasion hardening"]
    P8 --> P9["Phase 9: Review follow-ups"]
    P9 --> P10["Phase 10: Remainder remediation"]
    P7 --> P10
    P7 --> P8
```

Phases 3, 4, and 5 depend upon Phase 2 and proceed in parallel. Phase 8 Tier 0 and Tier 1 depend only on Phase 4; Tier 2 adds facts to every language pack, so it follows Phase 7.

---

## Shipped Gates

<!-- generated:gates -->
| Gate | Suite | Languages | Rule Description |
|---|---|---|---|
| [`agents-md`](GATES.md#agents-md) | agent-guard | any | AGENTS.md exists; CLAUDE.md / GEMINI.md do not fork it |
| [`assertion-reduction`](GATES.md#assertion-reduction) | agent-guard | Rust, Python, JS/TS, PHPT, Java, Go, PHP, C/C++, C#, Ruby, Kotlin | assertion count / strength must not drop in an existing test |
| [`vacuous-tests`](GATES.md#vacuous-tests) | agent-guard | Rust, Python, JS/TS, PHPT, Java, Go, PHP, C/C++, C#, Ruby, Kotlin | new tests must carry a non-tautological assertion |
| [`ignored-tests`](GATES.md#ignored-tests) | agent-guard | Rust, Python, JS/TS, PHPT, Java, Go, PHP, C/C++, C#, Ruby, Kotlin | tests must not be newly #[ignore]d or skipped without directive |
| [`unsafe-safety-comment`](GATES.md#unsafe-safety-comment) | agent-guard | Rust | unsafe blocks / impls carry a // SAFETY: comment |
| [`deletion-rationale`](GATES.md#deletion-rationale) | agent-guard | any | deleted files and removed tests need a scoped removes: rationale |
| [`time-estimates`](GATES.md#time-estimates) | hygiene | any | no calendar / duration estimates in markdown or the PR body |
| [`pii`](GATES.md#pii) | hygiene | any | no home paths, LAN IPs, or denylisted hostnames in tracked text |
| [`agent-scratch`](GATES.md#agent-scratch) | hygiene | any | agent scratch state is never tracked |
| [`shell-secrets`](GATES.md#shell-secrets) | hygiene | shell, docker, workflows | no command-line secrets or unverified piped scripts in shell, docker, or CI |
| [`issue-link`](GATES.md#issue-link) | hygiene | any | PR title or description links a tracking issue (#123, Fixes #123) |
| [`commit-provenance`](GATES.md#commit-provenance) | hygiene | any | commits carry the required trailers; an agent-produced commit carries a review by someone else |
| [`config-integrity`](GATES.md#config-integrity) | integrity | any | a change cannot weaken its own discipline.toml without a token |
| [`stub-bodies`](GATES.md#stub-bodies) | agent-guard | Rust, Python, JS/TS, Go, Java, C#, PHP, Ruby, C/C++, Kotlin | added functions are not stubs; existing bodies are not replaced by todo!() / NotImplementedError / return null |
| [`error-swallowing`](GATES.md#error-swallowing) | agent-guard | Rust, Python, JS/TS, Go, Java, C#, PHP, Ruby, C/C++, Kotlin | no new empty error handler or discarded Result outside tests |
| [`instruction-smuggling`](GATES.md#instruction-smuggling) | agent-guard | any (invisible characters, instruction files); Rust, Python, JS/TS, Go, Java, C#, PHP, Ruby, C/C++, Kotlin and prose files (phrases) | no invisible Unicode, unreviewed agent-instruction edits, or instruction-like text in comments and prose |
| [`build-hooks`](GATES.md#build-hooks) | integrity | package.json, build.rs, setup.py, .npmrc, .pypirc, pip.conf, .cargo/config.toml, .env* | install and build hooks that gain network or shell access, and package-manager configuration edits, need a token |
| [`toolchain-config`](GATES.md#toolchain-config) | integrity | tsconfig, ruff, mypy, pytest, coverage, flake8, Cargo lints, rustflags, nextest, eslintrc, golangci, jest, codecov, phpstan, phpunit | compiler, linter, type-checker, test-runner and coverage configuration cannot be loosened without a token |
| [`scope-confinement`](GATES.md#scope-confinement) | agent-guard | any | changes stay inside authorized paths |
| [`suppression-delta`](GATES.md#suppression-delta) | agent-guard | per pack | newly added linter / compiler suppression annotations |
| [`provenance-tags`](GATES.md#provenance-tags) | hygiene | any | published numerics carry (measured|target|projected) |
| [`ci-integrity`](GATES.md#ci-integrity) | integrity | any | workflow weakening: continue-on-error, || true, unpinned actions |
| [`ci-skip-set`](GATES.md#ci-skip-set) | integrity | any | rollup skip set matches each job's `if:` under the observed filter outputs |
| [`test-floor`](GATES.md#test-floor) | integrity | any | test-count ratchet read from the base ref |
| [`golden-output`](GATES.md#golden-output) | integrity | any | prevents stealth edits to committed golden/test output files without explicit override |
| [`dependency-delta`](GATES.md#dependency-delta) | integrity | any | manifest diff inspection: zero wildcards, source/license allowlists, and deny.toml verification |
| [`test-budget`](GATES.md#test-budget) | integrity | Rust, Python, JS/TS, Go, any | property-test and fuzz effort ratchet (cases, shrink iters, fuzztime, seed corpus) |
| [`pr-checklist`](GATES.md#pr-checklist) | hygiene | any | ticked PR checkboxes are reconciled against the diff |
| [`command`](GATES.md#command) | verification | any | fail-closed wrapper for any tool: zero-tests guard, canary, count ratchet |
| [`sanitizers`](GATES.md#sanitizers) | verification | Rust, C/C++ | ASan / TSan preset with audited suppressions and a race canary |
| [`msrv`](GATES.md#msrv) | quality | Rust | cargo check under the pinned MSRV |
| [`miri`](GATES.md#miri) | verification | Rust | Miri tiers with zero-tests guard |
| [`unsafe-budget`](GATES.md#unsafe-budget) | verification | Rust | unsafe count ratchet |
| [`bench-regression`](GATES.md#bench-regression) | bench | Rust, Go, Python, C/C++ | benchmark drift via harness adapters (deterministic counts or BCa intervals) |
| [`archive-contents`](GATES.md#archive-contents) | integrity | any | distribution archive must contain required paths and zero forbidden developer artifacts |
| [`manifest-sync`](GATES.md#manifest-sync) | integrity | any | reconcile git-tracked files against packaging manifest declarations |
| [`version-lockstep`](GATES.md#version-lockstep) | integrity | any | version declarations across headers, manifests, and files must remain in lockstep |
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
- **Status:** **Completed & Verified** (`ci-integrity`, `test-floor`, `suppression-delta`, and `scope-confinement` shipped).

### Phase 5: Verification Presets & Benchmark Adapters
- **Deliverables:**
  - `command`: Fail-closed execution wrapper for arbitrary external tools with canary checks, zero-test guards, and timeout limits.
  - Verification presets: `sanitizers` (ASan/TSan), `miri`, `msrv`, `unsafe-budget`.
  - Benchmark regression sentinel (`bench-regression` shipped with dual-file mode, two-tier threshold, and iai/json adapters).
- **Go / no-go gate:** Zero-test runs, missing baseline artifacts, and unprovenanced runs fail closed; deterministic counts and conservative bootstrap intervals discriminate regressions without false positives.
- **Status:** **Completed & Verified** (`command`, presets, and `bench-regression` shipped).

### Phase 6: Production Dogfooding
- **Deliverables:** Production deployment of `discipline.toml` in high-assurance consumer repositories; phased retirement of bespoke shell verification scripts.
- **Go / no-go gate:** Incident replay fixtures reproducing historical bypasses are caught by Discipline without regressing verification fidelity.
- **Status:** **Parity Achieved** (all 12 parity gaps shipped with zero false positives across historical PR corpus).

### Phase 7: Language Packs II
- **Deliverables:** Java / Kotlin pack (JUnit 5, AssertJ, Hamcrest); C / C++ pack (GoogleTest, Catch2); Go pack (`testing.T`, `testify`); PHP AST pack (PHPUnit, Pest).
- **Go / no-go gate:** 100% discriminating test coverage per language pack.
- **Status:** Complete (Java, Kotlin, Go, PHP, and C / C++ packs shipped).

### Phase 8: Agent-Evasion Hardening
Candidate work from a review of discipline as a safety net against autonomous coding agents. The review's central finding: the detectors are strong, and the trust boundary around them is the cheaper target. A change that can switch its own run to advisory, or grant itself every override, does not need to beat an AST gate. Tiers are ordered; within a tier, items are independent.

**Tier 0: the gates' own trust boundary.** Blocks the value of every later tier.

| Item | Gap | Status |
|---|---|---|
| Advisory self-switch | `mode = "advisory"` in the change's own `discipline.toml` made the run exit 0 | **Shipped**: reported by `config-integrity` under `[meta]`, and not honoured until it is on the base side or `allow-gate-weakening: meta` lifts it |
| Guard self-disable | `[gates.config-integrity] enabled = false` (or `severity = "note"`) silenced the report of the change's own weakenings | **Shipped**: enablement is read from the base configuration; findings use the stricter of the base and head severity |
| Unclassified options | Most gate options had no loosening direction, among them `min_tests`, `max_increase`, `workflows`, `forbidden_paths`, `deny_dependencies`, `allowed_override_actors`, and every `command` | **Shipped**: one direction table covers every option; a unit test fails on any option the table does not name |
| Action `advisory` input | `advisory: true` on the action step, or `--advisory` on a `discipline check` run line, was invisible to `ci-integrity` | **Shipped** |
| Other forges' workflows | `ci-integrity` read `.github/workflows/` only | **Shipped**: `.gitea/` and `.forgejo/` workflows share the Actions rules; GitLab pipelines have their own base-versus-head diff (`src/guards/ci_gitlab.rs`). Open: what a pipeline pulls in through `include:`, and narrowed `rules:` |
| Self-granted overrides | The PR body and commit bodies are written by the change author, `fail_on_overrides` defaults to false, and nothing capped the count | **Shipped**, opt-in: `directives.max_overrides` is a per-change budget; `directives.require_approval` makes directive overrides fail until the forge shows an approving review of the head commit by an `allowed_override_actors` member who is not the author (read-only, through `src/forge.rs`; exit 2 when it cannot be read). GitLab approvals are open |
| Policy from the base ref | The change is judged by its own `discipline.toml` | **Shipped**, opt-in: `--policy-from base` / action input `policy_from: base`; `ci-integrity` reports the input being moved off `base`. Chosen over signing the file: a verifying key kept in the repository is writable by the same change, and verification needs a cryptography dependency |
| `doctor` enforcement | `discipline doctor` (required check, CODEOWNERS, non-blocking jobs) ran in no workflow here | **Shipped** for the local checks: the `dogfood` job runs `doctor --local-only --strict`, under which an uncovered CODEOWNERS target fails. `doctor` also reports the branch's review rules (`review`, `code-owner-review`, `last-push-approval`). Running the forge-side checks in CI needs a token that can read branch protection and is open |

**Tier 1: coverage from existing machinery** (base-versus-head structural diffs; no new AST facts).

- **`toolchain-config` gate.** **Shipped** (`src/guards/toolchain_config.rs`): tsconfig, ruff, mypy, pytest, coverage.py, flake8, Cargo `[lints]`, rustflags, nextest, eslintrc, golangci-lint, jest, codecov, phpstan and phpunit, diffed base against head under one rule table; coverage floors, retries and test-selection narrowing included; a configuration written as code is reported as changed, not analysed. Open: `clippy.toml` (its options are thresholds with no single direction), what a file pulls in through `extends` / presets, and a list appearing where none was.
- **`golden-output` coupling.** **Shipped:** default globs cover `__snapshots__/`, `*.ambr`, `*.approved.*` and `*.golden`; a change that rewrites expectations while nothing producing output changed has its own title; the message states the lines rewritten. Open: added snapshot files are still skipped, because telling a snapshot for a new test from one added to an existing test needs the test-to-snapshot mapping of each framework.
- **Lockfile integrity in `dependency-delta`.** **Shipped** for `Cargo.lock`, `package-lock.json` and `yarn.lock` v1 (`src/guards/lockfile.rs`): entry from a new source, dropped integrity hash, manifest changed without its lockfile, lockfile deleted. Open: the other lockfile formats (named as not analysed), and extending the `ci-integrity` `--locked` rule to `npm ci`, `--frozen-lockfile`, `--require-hashes` and `uv sync --locked`.
- **`suppression-delta` on AST facts.** **Shipped:** the gate reads `ParsedFileFacts.escape_hatches` from every pack and compares each changed file's head side with its base side as a multiset, so a moved site is not new and a marker inside a string is not a site. The Rust pack now extracts `#[allow]` / `#[expect]`, the Java pack reads `@SuppressWarnings` as an annotation node, JS gains `@ts-nocheck`, Go gains `//lint:ignore`.
- **Existing-gate corrections.** **Shipped:** `test-floor` counts tests that run (unconditionally ignored tests are left out on both sides) and names files it could not read or that parse with errors. Open: `test-budget` extracts with line patterns (contrary to the AST-aware rule); an assertion under a constant-false branch, or after an unconditional `return`, counts in full.

**Tier 2: new AST facts.** Packs are hand-written visitors, so each fact is written once per language. Prerequisite, **shipped**: `LanguagePack::supplies(Fact)`; a gate that needs a fact a pack does not supply names the file as not analysed.

- **`stub-bodies`.** **Shipped** for Rust, Python, JS/TS, Go, Java and C# (`src/ast/functions.rs`, one walker and classifier with per-language tables; `src/guards/stub_bodies.rs`): an added function whose whole body is a stub marker, and an existing substantive body replaced by a stub, an empty body or a bare constant return. Abstract, overload, Protocol, interface and test members are excluded. Open: PHP, Ruby and C/C++ function facts (their files are named as not analysed); a stub padded with a second statement.
- **Mock infiltration.** **Shipped** (`src/ast/mocks.rs`): each test carries `mock_setups` and `mock_asserts`, counted from the call nodes inside its body against a shared vocabulary that `mock_setup_fns` / `mock_assert_fns` extend. `vacuous-tests` reports a new test whose every assertion is on a double's interactions; `assertion-reduction` reports an existing test whose doubles rose while its assertions on real output did not. Both at warning. Rust, Python, JS/TS, Go, Java, C#.
- **Error swallowing.** **Shipped** as the `error-swallowing` gate (`src/ast/handlers.rs`): a new empty handler (`except: pass`, `catch (e) {}`, bare return) or a discarded result (`let _ = f()`, `f().ok()`, Go `_ = err`, `x, _ := f()`) outside tests, base against head per file. Rust, Python, JS/TS, Go, Java, C#. Open: a handler that logs and swallows.
- **Retry annotations.** **Shipped** through `ignored-tests` (`src/ast/retries.rs`): a test that gains a retry / flaky marker, or arrives with one, is `Test Retries On Failure`; a file-level `jest.retryTimes` marks every test in the file. CI-level retry wrappers stay with `ci-integrity` / `toolchain-config` (nextest `retries`, pytest `--reruns`).

**Tier 3: governance.**

- **`instruction-smuggling` gate.** **Shipped** (`src/guards/instruction_smuggling.rs`; `Fact::Prose` in `src/ast/prose.rs`): (1) zero-width, bidirectional and tag characters in added lines, blocking; (2) any edit to an agent-instruction file needs `allow-agent-instructions:`, blocking; (3) instruction phrases, role markers, concealment, exfiltration, reviewer steering and long base64 runs in comments, strings and prose files, warning. Findings name the location and a class, never the text. Audited: `--format agent-prompt` prints titles, messages and locations only, never override reasons; `suppression-delta` still quotes the annotation line, recorded in ARCHITECTURE §10. Open: prose spans for PHP, Ruby and C/C++ packs.
- **`commit-provenance` gate.** **Shipped**, default off (`src/guards/commit_provenance.rs`): required trailers; an agent-identified commit (trailer, author name or email against `agent_markers`) needs a `Reviewed-by:` naming someone other than its author. `doctor` now reports `review`, `code-owner-review` and `last-push-approval` from rulesets and classic protection. Verifying signatures in the binary stays declined (cryptography dependency and a trusted keyring); git notes are not fetched by CI checkouts.
- **Registry verification of new dependencies: declined.** A lookup of package existence and first-publish date needs a network path beyond the forge API, which `AGENTS.md` §3.3 forbids, and it would send internal package names to public registries. Lockfile integrity (Tier 1) is the offline control; existence and advisory checks stay with the `command` presets. Reopen only with a design that keeps private names on the runner.

- **Go / no-go gate:** each item meets the gate contract in `AGENTS.md` §3.4: positive and negative unit controls, an end-to-end case through the binary, a `self-test` case, and a named test that kills a mutated detector. A Tier 2 item additionally names every pack that does not supply its fact.
- **Status:** Tier 0 shipped, with three named remainders (GitLab `include:` / `rules:` in `ci-integrity`, GitLab approvals in `require_approval`, forge-side `doctor` checks in CI); Tier 1 shipped (`toolchain-config`, lockfile integrity, `golden-output`, `test-floor`, `suppression-delta` on AST facts; the named remainders are `clippy.toml`, added snapshot files, the other lockfile formats, and `test-budget`'s line patterns); Tier 2 shipped (`stub-bodies`, mock infiltration, `error-swallowing`, retry annotations; PHP, Ruby and C/C++ function and handler facts are the named remainder); Tier 3 shipped (`instruction-smuggling`, `commit-provenance`; registry verification declined).

### Phase 9: Review Follow-Ups
Items from an external written review of Phase 8 (2026-09-21), each checked against the code before acting. The review's "strengths" section was right on the gates and wrong on their reach (six languages for the new facts, not "9+"; `agent_diff` is a module, not a gate).

| Review point | Verdict | Status |
|---|---|---|
| Assertion-free tests, tautologies, mocking the system under test | Already covered by `vacuous-tests` (`Vacuous Test Added`, constant-expression tautologies) and the Phase 8 mock rules. The example `assert val is not None` was a real miss: a near-tautology that counts as an assertion. | **Shipped**: `Test Asserts Only Trivial Properties` (`src/ast/calls.rs`, warning) |
| Assertion density, mutation analysis | `min_assertions_per_test` exists. In-binary mutation stays declined: the `command` gate's mutation presets are the control. | Declined |
| Package hallucination / slopsquatting | Correct description. Decided offline-only in Phase 8 (§3.3; a lookup discloses internal names). | Declined; see the note below |
| Build hooks and dotfiles | Correct and the largest gap. | **Shipped**: `build-hooks` gate |
| Delay-based concurrency patching | Correct. | **Shipped**: `Test Sleeps` through `ignored-tests` (warning) |
| Reviewer-bot injection in PR descriptions | `instruction-smuggling` already carried `reviewer-steering` for code and prose files; it did not read the PR body or commit messages. | **Shipped**: change-description scan, directive lines excluded |

- **Registry lookup, revisited.** An opt-in `dependency-delta.verify_registry` (default off) that asks crates.io, npm and PyPI whether a newly added direct dependency exists and when it was first published is implementable on the forge client (`src/forge.rs`: TLS, redirect pinning, body cap, `DISCIPLINE_NO_NETWORK`, exit 2 when unreachable). It stays declined until three things are settled: `AGENTS.md` §3.3 must be amended in the same PR; the names it sends are the change's new dependencies, which for a private monorepo means internal package names reaching a public registry, so a `private_prefixes` skip list is a precondition, not an option; and the lookup runs on every check of every PR that adds a dependency, so rate limits and outages become CI failures (exit 2), which the fail-closed contract requires. Lockfile integrity remains the offline control. Reopen when a consumer asks for it with those three answered.
- **Go / no-go gate:** as Phase 8.
- **Status:** shipped (`build-hooks`; sleeps, trivial assertions and change-description scanning in existing gates).

### Phase 10: Remainder Remediation
Every item Phases 7 to 9 left named as open, in one ordered plan. Each row is the whole remaining work for that item; a row ships when its gate criterion holds and the AGENTS.md §3.4 contract is met (unit controls, e2e through the binary, self-test case, a named test that kills a mutated detector). Rows in one step are independent of each other; a step depends on the step before it only where stated.

**Step 0: false positives from the first consumer replay** (before anything else: each is a v0.8.0 finding a real repository's CI had already judged correct, checked against the code on 2026-09-21; a gate that blocks correct changes is held at `warning` by that consumer until these ship)

| Item | Verified against the code | Work | Ships when |
|---|---|---|---|
| A consumer cannot declare its own test entry points | **Shipped** (`[tests] functions` / `paths`). Yes: the pytest-collection rule makes a script's `self_test()` production code, and `error-swallowing`, `pii` and the assertion gates all read that boundary; today's workaround is `assert_helper_fns` | One declaration, `[languages.<lang>] test_functions = [...]` and a gate-independent `test_paths`, honoured by every gate that separates test code from production code (`TestFn` collection, `handlers::extract`'s `is_test_line`, `pii`'s scope) | One setting removes the three families of findings from the consumer's replay; three gate-local options are not added |
| `error-swallowing`: expect-this-to-raise | **Shipped.** Yes: `try` / `except: pass` / `else: raise` and `except: continue` followed by a recorded failure are empty handlers to `handlers.rs` | A `try` whose `else:` raises, or whose fall-through records a failure, is an assertion, not a swallow: read the sibling `else` clause and the statement after the `try` | The quoted snippets are silent; a bare `except: pass` in a library module still fails |
| `error-swallowing`: a tuple binding is not a call | **Shipped.** Yes: `rust_discards` accepts any `let _ =` whose right side contains `(`, so `let _ = (word, alloc);` is reported as a discarded result | Report `let _ =` only when the right side is a call expression (AST node, not text) | The tuple is silent; `let _ = file.sync_all();` still fails |
| `error-swallowing`: Cargo `tests/`, `benches/`, `examples/` are test scope | **Shipped.** Yes: test scope is the `TestFn` spans only, so a helper in `crates/x/tests/*.rs` is production code | `is_test_line` also true for every line of a file under those directories (and the other packs' equivalents via `functions::test_path`) | `let _ = catch_unwind(...)` in an integration test is silent |
| `unsafe-safety-comment`: `unsafe trait` documented by rustdoc | **Shipped** (heading required; a run-walk pairing bug in the comment collector, exposed by the fix, is fixed with it). Yes: an `unsafe trait` is an unsafe site requiring `// SAFETY:`; a doc comment with a `# Safety` section (the convention clippy's `missing_safety_doc` checks) is not accepted | Accept a preceding doc comment containing a `# Safety` heading for `unsafe trait` and `unsafe fn`; decide and document whether a contract stated without the heading counts (recommendation: no) | The quoted `OlcEngine` trait is silent |
| `stub-bodies`: abstract base-class methods | **Shipped.** Yes: the skip covers `@abstractmethod` and `Protocol` / `TypedDict` / `NamedTuple` bases, not `abc.ABC` nor a method overridden by a subclass in the same file | Skip a stub in a class deriving from `ABC` / `ABCMeta`, and one whose name a same-file subclass overrides | `bump_version.py`'s base-class pair is silent |
| `time-estimates`: past-interval phrasing | **Shipped.** Yes: "N units later" is exempt, "the N-unit gap between" two past events is not | Extend the terms-of-art list with the past-interval forms (`N-unit gap between`, `N units between`, past-tense equivalents) | The quoted README line is silent; a forward-looking "ships in N units" still fails |
| `--whole-tree` records delta-only rules | **Shipped** (`DELTA_ONLY_GATES` in `src/guards/mod.rs`; symlinks skipped in `changed_files`). Yes: a whole-tree baseline diffs against an empty tree, so `New Direct Dependency Added`, `Agent Instructions Changed` and `Test Arrives Ignored` fire for every file, and symlinked `CLAUDE.md` / `GEMINI.md` are recorded separately | In whole-tree mode, gates whose rule describes a delta report nothing (each `GateOutcome` names itself delta-only); a symlink resolves to its target once | The consumer's baseline records no delta-only rule |
| `provenance-tags`: what satisfies a ratio, and `diff_only` | **Shipped** (`ratio_satisfied_by`, `deterministic_units`, `diff_only`). Policy disagreement, not a defect: the consumer's paragraph-scoped rule accepts an interval, a results artifact reference, or a `superseded` / `unsourced` / `provisional` marker, and exempts deterministic units | `ratio_satisfied_by = ["interval", "artifact:<glob>", "marker:<word>"]` and `deterministic_units`, default unchanged; `diff_only` as on the other hygiene gates | With a config expressing the consumer's rule, the gate agrees with its script on every tracked Markdown line |
| Wiring recipes | **Shipped** (`docs/guides/ci-platforms.md`, two sections: rollup skip-set wiring, benchmark job wiring). Documentation | A complete `ci-skip-set` rollup job (`DISCIPLINE_CI_CONTEXT` from `toJson(needs)` plus path-filter outputs, and the `if:` forms modelled) and a `bench-regression` job (merge base and head measured with iai-callgrind, `require_sourced_override`, which citation-freshness rules are enforced) in `docs/guides/ci-platforms.md` | The consumer retires its two remaining scripts against the recipes |

Acceptance for Step 0 as a whole: the consumer's 100-PR replay with its config, the test-entry declaration and `error-swallowing` at `error` blocks only on `instruction-smuggling` `AGENTS.md` edits, its three true positives and its two deliberate `|| true` lines.

**Step 1: parity of the new facts across packs** (the Tier 2 remainder; every later step benefits)

| Item | From | Work | Ships when |
|---|---|---|---|
| Function, handler and prose facts for PHP, Ruby and C/C++ | **Shipped** (`PHP_FUNCTIONS` / `RUBY_FUNCTIONS` / `C_FUNCTIONS` and the matching handler, mock, retry and prose specs; PHP `@` and Ruby `rescue nil` are `Error Silenced`, C/C++ `(void)call()` is `Result Discarded`). Tier 2, Tier 3 | Fill `FunctionSpec`, `HandlerSpec`, mock/call kinds and prose kinds in `src/ast/php.rs`, `ruby.rs`, `c_cpp.rs`; classifiers exist in `functions.rs` (`classify_php`, `classify_ruby`, `classify_c`); set `supplies()` for `Functions`, `Handlers`, `Prose` | `stub-bodies`, `error-swallowing` and `instruction-smuggling` report no "supplies no facts" note on a `.php`, `.rb`, `.c` / `.cpp` change, and the pack tests mirror the six existing ones |
| Kotlin pack | **Shipped** (`src/ast/kotlin.rs` behind the default `lang-kotlin` feature, grammar `tree-sitter-kotlin-ng`). Phase 7 | New `src/ast/kotlin.rs` behind `lang-kotlin`: `@Test` / `@ParameterizedTest`, JUnit / AssertJ / Kotest matchers, `@Disabled` / `@Ignore`, `@Suppress`, plus the four new facts | The four-point contract per language holds, `docs/GATES.md` language table no longer lists Kotlin as planned, `UNSUPPORTED_SOURCE_EXTS` drops `kt` / `kts` |

**Step 2: precision of existing detectors** (Tier 1 and Tier 2 corrections; independent of Step 1)

| Item | From | Work | Ships when |
|---|---|---|---|
| `test-budget` on AST facts | **Shipped** (`src/ast/budgets.rs`, `Fact::Budgets`; Rust, Python, JS/TS packs; scripts and workflows stay line-based). Tier 1 | Replace the line patterns in `src/guards/test_budget.rs` with pack facts: a `Fact::Budgets` (proptest `cases`, quickcheck `tests`, hypothesis `max_examples`, fast-check `numRuns`) read from call and attribute nodes | The `min_tests = 40` fixture class cannot recur (a budget inside a string or comment is not a budget); existing e2e cases pass unchanged |
| Unreachable assertions | **Shipped** (`src/ast/reach.rs`, every pack). Tier 1 | In each pack's assertion walk, do not count an assertion under a constant-false condition or after an unconditional `return` / `panic!` / `pytest.skip()` at the same block level; reuse the Rust constant-expression check | `assert_eq!` inside `if false {}` and after `return` counts 0; a self-test case pins each |
| Padded stubs | **Shipped** (`functions::classify_body`, shared `inert_statement`). Tier 2 | `BodyShape::Stub` also when a body is a stub marker plus statements that cannot affect the result (logging, a bare assignment, `println!`), judged per language | `fn f() { log::warn!("todo"); todo!() }` is a stub; `fn f() { init(); todo!() }` is not |
| Logging swallow | **Shipped** (`handlers::body_swallows` returns the kind; `LOGGING_VOCAB` shared with the stub classifier). Tier 2 | In `handlers.rs`, a handler whose statements are all logging calls (`log.`, `logger.`, `console.error`, `eprintln!`, `warn!`) and no re-raise, return of the error, or state change is `Empty Error Handler Added` with a "logs and swallows" message | The e2e negative control keeps a handler that logs **and** re-raises silent |

**Step 3: coverage of files and forges** (Tier 0 and Tier 1 remainders)

| Item | From | Work | Ships when |
|---|---|---|---|
| Other lockfile formats | Tier 1 | `parse_lock` for `pnpm-lock.yaml`, `poetry.lock`, `uv.lock`, `composer.lock`, `Gemfile.lock`, Yarn 2+ (`__metadata:`); `go.sum` stays exempt | Each format has a source-swap and a dropped-hash test; the "not analysed" note is gone for them |
| Frozen-install flags in CI | Tier 1 | Extend the `ci-integrity` `--locked` rule to `npm ci` → `npm install`, `--frozen-lockfile`, `--immutable`, `--require-hashes`, `uv sync --locked` / `--frozen`, `poetry install --no-update` | A dropped flag or a softened install command is `Cargo Flag Dropped`-class finding under its own title |
| Added snapshot files | Tier 1 | In `golden-output`, an added snapshot whose name maps to a test that already existed on the base side (Jest `__snapshots__/<file>.snap` keyed by test title, insta `<module>__<test>.snap`, pytest-snapshot `<test>.ambr`) is reported; a snapshot for a new test is not | Two e2e cases per framework: new test + new snapshot silent, existing test + new snapshot reported |
| GitLab `include:` and `rules:` | Tier 0 | `ci_gitlab.rs` follows `include: local:` files in the same tree (never `project:` / `remote:`, which are named as not read) and reports a `rules:` / `only:` / `except:` change on a verification job as `Verification Job Narrowed` | A local include that adds `allow_failure` is reported; a remote include is a note |
| `clippy.toml` | Tier 1 | Per-key direction table for the threshold keys (`too-many-arguments-threshold`, `cognitive-complexity-threshold`, `type-complexity-threshold`, ... all `Cap`); `allowed-*` lists `Grown`; `disallowed-*` lists `Shrunk` | Raising a threshold or growing an allow list is `Toolchain Configuration Weakened` |
| `toolchain-config` inheritance | Tier 1 | Report a changed `extends` / `plugins` / `presets` entry as `Toolchain Configuration Changed (not analysed)` at warning, the same way a configuration written as code is | An `extends` swap is never silent |

**Step 4: forge-side enforcement** (Tier 0 remainders; needs a token that can read branch protection and pull-request reviews)

| Item | From | Work | Ships when |
|---|---|---|---|
| Forge-side `doctor` in CI | Tier 0 | A `doctor` job in `ci.yml` with a fine-grained token (read: administration, pull requests) stored as a repository secret, `--strict`, on `main` pushes only | The job is in `ci-gate`'s `needs` and the asserted job count; `required-check`, `review`, `code-owner-review`, `last-push-approval` are Pass on this repository |
| GitLab approvals in `require_approval` | Tier 0 | `pull_approvers` for `ForgeKind::GitLab`: `projects/{id}/merge_requests/{iid}/approval_state`, matched on the head SHA of the approval rule | The `FakeForge` e2e has a GitLab variant; an approval of an earlier SHA is refused |

**Step 5: PR-body waivers on a push to the default branch** (from a consumer's replay at v0.9.0: a pull request whose body carried `allow-agent-instructions: AGENTS.md <reason>` passed on `pull_request`, was squash-merged, and the `push` run on `main` — base from `github.event.before` — failed on the same edit because a squash or rebase merge builds the commit message from the branch's commits, not from the body, and a merge commit's default message does not carry it either; every PR-body directive reproduces this)

| Item | From | Work | Ships when |
|---|---|---|---|
| Say why a waiver is missing on a push | **Shipped** (`check` appends the push note to every directive-liftable finding and the gate notes). Consumer replay | On a push (or `--commit` / `--commit-range`) run, a violation the matching directive would have lifted names the event and says PR-body directives are not in scope; the remediation names commit-message directives and the `merged-pr-body` source instead of a PR body the run never reads | The replay of the consumer's merge as a push says why it failed |
| Event / source matrix in the docs | **Shipped** (CONFIGURATION.md Override Directives and Action Reference, ci-platforms.md §8, README quickstart note). Consumer replay | `docs/CONFIGURATION.md` (action reference and directives) and `docs/guides/ci-platforms.md` state which sources each event sees (`pull_request`: body and branch commits; `push`: pushed commits, plus `merged-pr-body` once shipped), that squash and rebase merges drop the body, and the recommended trigger set; the README quickstart comment covers the push case next to `edited` | The matrix is in both documents |
| `ci-integrity`: a narrowed verification step | **Shipped** (`Verification Step Narrowed`, warning). Consumer workaround | A newly added or narrowed step-level `if:` on a verification step (including the discipline step) is `Verification Step Narrowed` at `warning`, liftable with `allow-gate-weakening: ci-integrity <reason>` | A diff adding `if: github.event_name == 'pull_request'` to a verification step is reported |
| `merged-pr-body` directive source | Consumer replay | On a push event, each commit in the range resolves its merged pull request through the forge client (GitHub `commits/{sha}/pulls`, Gitea / Forgejo `commits/{sha}/pull`, GitLab `repository/commits/:sha/merge_requests`) and its body is read under the same trust rules as on the pull request (line-anchored, `allow_hidden`, scoped subjects, `allowed_override_actors` against the PR author, `max_overrides`, `require_approval`); on by default for push events, reported in the notes with the PR number; a direct push keeps commits only; an unreachable forge or a token without permission is `could not check` (exit 2) unless `directives.degrade_offline = true` turns it into a named notice | A squash-merge commit whose PR body carries the waiver passes on push; the same commit with a body lacking it fails; a direct push fails; the GitLab / Gitea / Forgejo lookups have unit coverage |
| `doctor` flags the configuration | Consumer replay | When a workflow runs the discipline action (or `discipline check`) on `push` to the default branch and the branch's merge method allows squash or rebase, a `Warn` finding names the fix: enable `merged-pr-body`, restrict the step to `pull_request`, or put directives in commit messages | The consumer's `ci.yml` as it stood is reported |

- **Go / no-go gate:** as Phase 8; the `merged-pr-body` source ships with the offline behaviour decided and documented, never as a silent pass.
- **Status:** planned; ordered as listed (the message and docs first, the source second, `doctor` last).

**Declined, and stays declined unless the stated condition changes:** registry verification of new dependencies (Phase 9 note: needs a `private_prefixes` contract, accepts registry outages as CI failures, and an amendment to `AGENTS.md` §3.3); in-binary mutation analysis (the `command` gate's presets are the control); in-binary commit-signature verification (cryptography dependency and a trusted keyring; `doctor` reads the branch rule).

- **Go / no-go gate:** as Phase 8; additionally, Step 1 ships all three packs together or names in `docs/GATES.md` which fact each pack still lacks.
- **Status:** in progress. Order: Step 0 first (correctness for a live consumer), then Step 1 (it widens what every later step covers), Step 2, then Step 5 (a second consumer-replay correctness item, ahead of coverage work), Step 3, Step 4 when the token exists.

---

## Default Changes (Compatibility Ledger)

Default enablement and severity are part of the compatibility contract (`docs/ARCHITECTURE.md` §3.1). Every change to a built-in default is recorded here, newest first; a **loosening** within a major version is not allowed without an entry. Each entry names the one-line configuration that restores the previous behaviour.

| Release | Gate | Old default | New default | Direction | Reason | Restore previous behaviour |
|---|---|---|---|---|---|---|
| v0.9.0 | `build-hooks` | (new gate) | on, `error` | stricter | A new or changed `package.json` lifecycle script, a build script gaining process / network / shell access, or any edit to package-manager configuration (`.npmrc`, `.pypirc`, `pip.conf`, `.cargo/config.toml`, `.env*`) is reported. | `[gates.build-hooks]` `enabled = false` |
| v0.8.0 | `commit-provenance` | (new gate) | off, `error` | none (off) | Required commit trailers and a review by someone else on agent-produced commits. Off because which trailers a repository requires is its own policy. | n/a |
| v0.8.0 | `instruction-smuggling` | (new gate) | on, `error` | stricter | Zero-width, bidirectional and tag characters in added lines and any edit to an agent-instruction file (`AGENTS.md`, `.cursorrules`, `copilot-instructions.md`, skill files, ...) are reported; instruction-like phrases and encoded blobs in comments, strings and prose at `warning`. Findings carry location and class only. | `[gates.instruction-smuggling]` `enabled = false` |
| v0.8.0 | `error-swallowing` | (new gate) | on, `error` | stricter | A new empty error handler or discarded fallible result outside tests is reported, base against head per file. Rust, Python, JS/TS, Go, Java, C#; other packs name their files as not analysed. | `[gates.error-swallowing]` `enabled = false` |
| v0.8.0 | `stub-bodies` | (new gate) | on, `error` | stricter | An added function whose whole body is `todo!()` / `raise NotImplementedError` / a not-implemented `throw`, or an existing body replaced by a stub, an empty body or a bare constant return, is reported. Rust, Python, JS/TS, Go, Java, C#; other packs name their files as not analysed. | `[gates.stub-bodies]` `enabled = false` |
| v0.8.0 | `toolchain-config` | (new gate) | on, `error` | stricter | A change cannot loosen the toolchain configuration it is judged by: same design as `config-integrity`, one rule table over tsconfig, ruff, mypy, pytest, coverage, flake8, Cargo lints, rustflags, nextest, eslintrc, golangci, jest, codecov, phpstan, phpunit. A configuration written as code is reported at `warning` as not analysed. | `[gates.toolchain-config]` `enabled = false` |
| v0.7.0 | `ci-skip-set` | (new gate) | on, `error` | stricter | Checks a rollup job's skip set against the filter outputs it observed. Inert (reported as not evaluated) until a workflow passes `DISCIPLINE_CI_CONTEXT`. | `[gates.ci-skip-set]` `enabled = false` |
| v0.7.0 | `suppression-delta` | on, `error` | on, `warning` | looser | `#[allow(...)]` is the reviewed escape hatch from `clippy -D warnings` and `# noqa` is routine; 78 findings (measured) across one consumer's last 100 merged pull requests. First entry recorded under this contract. | `[gates.suppression-delta]` `severity = "error"` |
| v0.5.0 (retrospective) | `time-estimates` | on, `error` | on, `warning` | looser | Brownfield documentation produced mostly pre-existing findings. Shipped without a migration note; a consumer relying on the default stopped blocking silently. | `[gates.time-estimates]` `severity = "error"` |
| v0.5.0 (retrospective) | `agents-md` | on, `error` | on, `warning` | looser | Missing or forked agent guidance is hygiene, not a code defect. Shipped without a migration note. | `[gates.agents-md]` `severity = "error"` |
| v0.5.0 (retrospective) | `bench-regression` | on, `error` | on, `warning` | looser | Wall-clock benchmarks are sensitive to runner jitter. Shipped without a migration note. | `[gates.bench-regression]` `severity = "error"` |
| v0.2.1 (retrospective) | `issue-link` | on, `error` | off | looser | Needs a repository-specific tracker convention. Shipped without a migration note. | `[gates.issue-link]` `enabled = true` |
| v0.2.1 (retrospective) | `provenance-tags` | on, `error` | off | looser | Encodes a research-publication policy most repositories do not hold. Shipped without a migration note. | `[gates.provenance-tags]` `enabled = true` |

From v0.7.0 a new gate that ships enabled is listed too, since for a consumer it changes what blocks. Gates introduced between v0.2.0 and v0.6.0 are not listed.

### Behaviour Changes

A change to what a gate reports, an exit code, or an output, with an unchanged default. Newest first; each release's rows are copied into its release notes under "Upgrading" (`scripts/release_notes_upgrade.py`).

| Release | Area | Change | Direction | Migration |
|---|---|---|---|---|
| unreleased | `ci-integrity` | A verification step that gains a step-level `if:`, or whose `if:` changes, is `Verification Step Narrowed` at `warning` (the `always()` / `failure()` forms stay `Conditional Masking on Verification Step`). | stricter (warning) | `allow-gate-weakening: ci-integrity <reason>`. |
| unreleased | `check` on a push event | A finding whose remediation names a directive says that the run is a push, that PR-body directives are not in scope, and which sources a push reads; the gate's notes carry the same note. Output only. | none | None. |
| unreleased | `assertion-reduction`, `vacuous-tests` | An assertion inside a constant-false `if` or after an unconditional `return` / `panic!` / `pytest.fail()` / `throw` at the same block level counts 0 (a skip call is not a terminator: `ignored-tests` owns it) in every language pack. A test that only asserts there is vacuous; moving an assertion there is a reduction. | stricter | `allow-assertion-drop:` / `allow-vacuous:` as for any finding, or delete the dead assertion. |
| unreleased | `test-budget` | Rust, Python and JS/TS budgets are read from the syntax tree (`Fact::Budgets`): a named integer in a configuration position, including inside a `proptest!` token tree. A budget-like word in a string, a comment or an unrelated assignment (`min_tests = 40`, a bare `tests = 300`) no longer counts, so a fixture quoting one is not a reduction and cannot be one. | narrower | None; workflow and script budgets are unchanged. |
| unreleased | packaging | Shell completions for bash, zsh and fish are generated by `discipline docs --write` into `completions/` (committed, checked by `docs --check`), shipped in the release tarballs, and installed by the APT and RPM packages, the Homebrew formula (`generate_completions_from_executable`) and the Portfile. The README's Installation section documents the tarball / `install.sh` / `cargo install` case. | additive | None; a hand-installed script is superseded by the packaged one on the next upgrade. |
| unreleased | `stub-bodies` | A stub marker preceded only by logging lines or assignments that call nothing is a stub (`fn f() { log::warn!("todo"); todo!() }`); a marker preceded by a call is still substantive. | stricter | `allow-stub: <name> <reason>`, or `[gates.stub-bodies]` `enabled = false`. |
| unreleased | `error-swallowing` | A new handler whose every statement only logs the error is `Empty Error Handler Added` ("logs it, and does nothing else"); a handler that logs and re-raises, returns or records the failure is not. | stricter | `allow-swallow: <path> <reason>`, or `[gates.error-swallowing]` `enabled = false`. |
| unreleased | language packs | New Kotlin pack (`.kt`, `.kts`) behind the default `lang-kotlin` feature: JUnit / TestNG / Kotest tests, assertions, skips, `@Suppress`, and function, handler and prose facts. Previously those files were named in a note as not analysed. | stricter | Build without `lang-kotlin`, or exempt the paths under the gate's `exempt_paths`. |
| unreleased | `stub-bodies`, `error-swallowing`, `instruction-smuggling` | The PHP, Ruby, C and C++ packs supply function, handler and prose facts: a stub body, an empty `catch` / `rescue`, PHP `@call()` and Ruby `call rescue nil` (new title `Error Silenced`), C/C++ `(void)call()`, and instruction-like text in comments and strings of those languages are reported. Previously those files were named in a note as not analysed. A top-level PHP `function` is now read as a test when its name starts with `test` (the pack matched an old grammar kind and missed them). | stricter | Exempt a path under the gate's `exempt_paths`, or record the site with the gate's directive. |
| unreleased | action | The `actor` input defaults to the pull request author (`github.event.pull_request.user.login`), falling back to `github.actor` off a pull request. Previously it was the triggering login, so on an `edited` event an allow-listed editor of someone else's pull request waived `fail_on_overrides` for an override they could have written themselves. | reclassified | Pass `actor:` explicitly to keep judging the triggering login. |
| unreleased | documentation | The job-container recipe in `docs/CONFIGURATION.md` (Gitea / Forgejo, no `uses:`) was wrong in two ways: it named a `docker://` image under `runs-on`, which matches no runner label, so the job never ran; `git clone` landed on the default branch, so a job that did run compared the base with itself and passed every pull request. The recipe now runs on a registered label with `container: image:` pinned by tag and digest, fetches `refs/pull/<n>/head`, verifies the checkout against the event's head commit, and passes the base, title, body and the pull request author as the actor. It is executed in CI. | stricter | A repository that copied the old recipe has a gate that never ran or never failed: replace the job with the current recipe and re-run it on an open pull request. |
| unreleased | `baseline --whole-tree` | Gates whose rule describes a change (dependency-delta, ignored-tests, config-integrity, ci-integrity, build-hooks, error-swallowing, stub-bodies, suppression-delta, test-floor, test-budget, golden-output, deletion-rationale, assertion-reduction, scope-confinement, commit-provenance, bench-regression, ci-skip-set, toolchain-config; instruction-smuggling's instruction-file rule) are not evaluated in a whole-tree run and record nothing. Symlinks are no longer enumerated as changed files. | narrower | Re-write an existing whole-tree baseline; stale entries for those gates are reported as stale. |
| unreleased | `provenance-tags` | New options `ratio_satisfied_by`, `deterministic_units`, `diff_only`; defaults keep the built-in rule. | looser when set | None. |
| unreleased | configuration | New `[tests]` table (`functions`, `paths`) declares test scope for every gate that separates test code from production code. Older binaries reject the table. | looser when set | Upgrade every binary that reads the file together. |
| unreleased | `error-swallowing` | The expect-this-to-raise idiom, a `let _ =` binding of a non-call, and handlers in Cargo `tests/` / `benches/` / `examples/` are no longer reported. | looser | None; these were false positives. |
| unreleased | `unsafe-safety-comment` | A `# Safety` rustdoc section documents an `unsafe trait` / `unsafe fn`. A comment run above a site is now walked correctly when it contains bare `///` lines (a line comment's end position is at column 0 of the next row and was read as spanning it). | looser | None; documented sites were false positives. |
| unreleased | `stub-bodies` | Methods of `abc.ABC` classes and base-class methods overridden by a same-file subclass are not stubs. | looser | None. |
| unreleased | `time-estimates` | Past-interval phrasing (`N-hour gap between`, `gap of N days`, `N minutes between each`) is a term of art. | looser | None. |
| v0.9.0 | `ignored-tests` | A test that gains a hard-coded delay (`thread::sleep`, `time.sleep`, `setTimeout`, ...) is `Test Sleeps` (warning). | stricter | `allow-ignore: <test> <reason>`. |
| v0.9.0 | `vacuous-tests` | A new test whose every assertion holds for nearly any value (`is not None`, `toBeDefined`, `.is_ok()`, `assertNotNull`, ...) is `Test Asserts Only Trivial Properties` (warning). | stricter | Assert on the value. |
| v0.9.0 | `instruction-smuggling` | The PR title and body and every commit message in the range are scanned for instruction phrases (warning) and invisible characters (blocking); directive lines are skipped. `examined` counts them. | stricter | `allow-agent-instructions: pr-body <reason>`. |
| v0.8.0 | `doctor` | Three review findings from branch rules: `review` (approving-review count), `code-owner-review`, `last-push-approval` (or stale-review dismissal). Each is a warning where the rule is absent, so `--strict` runs on a branch without review rules now fail. | stricter | Add the review rule, or read the finding as advice without `--strict`. |
| v0.8.0 | `ignored-tests` | A test that gains a retry / flaky marker (`@pytest.mark.flaky`, `jest.retryTimes`, `@RetryingTest`, ...) is reported as `Test Retries On Failure`. | stricter | `allow-ignore: <test> <reason>`. |
| v0.8.0 | `vacuous-tests`, `assertion-reduction` | Mock usage is read from test bodies. A new test asserting only on a double's interactions is reported (`Test Asserts Only On Mocks`, warning); an existing test whose doubles rose without a stronger assertion on real output is reported (`Mocking Grew Without Stronger Assertions`, warning). New options `mock_setup_fns` / `mock_assert_fns` on both gates. | stricter | `allow-assertion-drop: <test> <reason>` for the delta; assert on the result for the vacuity class. |
| v0.8.0 | `suppression-delta` | Sites come from the language packs and are a base-versus-head delta per file: a moved suppression, or one inside a string, is no longer reported; Java `@SuppressWarnings`, `@ts-nocheck` and `//lint:ignore` now are; Ruby and PHP suppressions are read for the first time. Files whose head side does not parse are named as not analysed. | reclassified | None; findings that were false positives disappear, and a few new ones appear in Java, PHP and Ruby. |
| v0.8.0 | `test-floor` | The static count is of tests that run: an unconditionally ignored / skipped test is no longer counted, on the base or the head side. Counts can drop. | stricter | Re-baseline `min_tests` / constant floors; the gate notes state how many tests were left out. |
| v0.8.0 | `dependency-delta` | Lockfiles are read, not only sized: an entry from a new source, a dropped integrity hash, a manifest changed without its tracked lockfile, and a deleted lockfile are violations. | stricter | `allow-dependency: <package-or-lockfile> <reason>`. |
| v0.8.0 | `golden-output` | Default `paths` add `**/__snapshots__/**`, `**/*.ambr`, `**/*.golden`, `**/*.approved.*`; a rewrite with no output-producing change is titled "Golden Output Regenerated Without Source Change". | stricter | Set `paths` to the previous four globs. |
| v0.8.0 | `ci-integrity` | GitLab pipelines (`.gitlab-ci.yml`, `.gitlab/ci/*.yml`) are in the default `workflows` and are diffed for `allow_failure`, `when: manual`, masked script lines, `--advisory`, deleted verification jobs. | stricter | Remove the GitLab globs from `workflows`. |
| v0.8.0 | directives | New options `directives.max_overrides` and `directives.require_approval`, and the `--policy-from` flag / `policy_from` action input. All default to the previous behaviour. Older binaries reject the two new keys. | stricter when set | Upgrade every binary that reads the file together. |
| v0.8.0 | report | The JSON report gains `policy_failures` when an override budget or approval refuses a run; a run can now exit 1 with `errors: 0`. | reclassified | Read `status` / the exit code, not `errors`. |
| v0.8.0 | `config-integrity` | `mode = "advisory"` introduced by the change under review is reported under `[meta]` and is not honoured: the run keeps its enforcing exit code until the setting is on the base side. | stricter | `allow-gate-weakening: meta <reason>`, or merge the mode change on its own. |
| v0.8.0 | `config-integrity` | The gate runs whenever the base configuration enables it, even if the head configuration or `--disable` turns it off, and reports at the stricter of the base and head severity. | stricter | `allow-gate-weakening: config-integrity <reason>`; the new setting applies from the next change. |
| v0.8.0 | `config-integrity` | Every gate option has a loosening direction. Newly reported: a lowered `min_tests` / `min_assertions_per_test`, a raised `max_increase` / `tolerance`, a removed floor or cap, shrunk `workflows`, `forbidden_paths`, `deny_dependencies`, `manifests`, `required_paths`, `forbidden_patterns`, `forbid_output`, `corpus_dirs`, `fuzz_targets`, `extra_secret_patterns`, `required_suites`, `placeholders`, `commands`, `rules`, `groups`; grown `allowed_suppressions`, `excluded_jobs`, `first_party_action_prefixes`, `allowed_override_actors`; an emptied `allow_dependencies` / `allowed_paths`; `allow_cross_host`, `allow_wildcards`, `allow_increase` switched on; a changed or removed `command`, `test_command`, `canary_command`, `preset`, `count_pattern`, `zero_items_pattern`, `pattern`, `deny_file`, `rollup_job`, `sanitizer`, `archive_path`, `constant_*`, `documented_job_count_*`. | stricter | `allow-gate-weakening: <gate> <reason>`. |
| v0.8.0 | `ci-integrity` | `advisory: true` added to the discipline action step, or `--advisory` added to a `discipline check` / `discipline diff` run line, is a violation. | stricter | `allow-gate-weakening: ci-integrity <reason>`. |
| v0.8.0 | `ci-integrity` | The default `workflows` globs also cover `.gitea/workflows/` and `.forgejo/workflows/`. | stricter | Set `workflows` to the previous two `.github/workflows/` globs. |
| v0.7.2 | `doctor` | On Gitea, doctor reads the instance version; below 1.26 (which ignores a workflow's `permissions:`) the token finding is information, not a warning, and names the upgrade. | narrower | None. |
| v0.7.1 | forge access | An SSH remote's host is resolved through `~/.ssh/config` (`Host` alias to `HostName`, with `Include`) before the forge's API address is built; `doctor` prints that address (`forge_url` in JSON) and points an unreachable host at `DISCIPLINE_FORGE_URL`. | reclassified | None; set `DISCIPLINE_FORGE_URL` if the API is served elsewhere than the SSH host. |
| v0.7.0 | configuration | New keys (`superseded_registry`, `pending_issue_repos`, `mode`, `citation_*`, `[gates.ci-skip-set]`, ...) are rejected by older binaries, which refuse unknown keys. | stricter | Upgrade every binary that reads the file (pre-commit `rev:`, pinned images) together. |
| v0.7.0 | `test-floor` | `test_command` without `min_tests` or a `constant_*` floor exits 2 instead of passing. | stricter | Set `min_tests`, or remove `test_command`. |
| v0.7.0 | `time-estimates` | `allow_patterns` exempt only the text they match, not the whole line. | stricter | Widen the pattern to cover the text to exempt. |
| v0.7.0 | test detection | Python tests follow pytest/unittest collection (`self_test` is not a test; methods count only in `Test*` / `TestCase` classes); the C# name-only fallback is removed. Test counts can drop. | stricter | Re-baseline `min_tests` / constant floors after upgrading. |
| v0.7.0 | `bench-regression` | A malformed `exempt_arms` glob exits 2; an entry that matches no arm in the run is an error at any gate severity. | stricter | Fix or remove the entry. |
| v0.7.0 | `miri`, `sanitizers` | A missing or unsupported toolchain exits 2 (action `status: error`) instead of reporting undefined behaviour or a race (exit 1). | reclassified | Install the toolchain component, or disable the gate. |
| v0.7.0 | `baseline` | `baseline --write` records blocking findings only; `--all-severities` restores the previous output. `--write` refuses to replace an unreadable baseline (exit 2). | narrower | Pass `--all-severities` to keep warnings and notes. |
| v0.7.0 | submodules | A submodule pointer change is skipped instead of exiting 2. | looser | None needed. |
| v0.7.0 | `ci-integrity` | A renamed CI step is paired with its original by body similarity (at least 0.60, same verification markers) instead of being reported as deleted. | looser | None; the rename is listed in the gate notes. |
| v0.7.0 | `pr-checklist` | A ticked test box is backed by a test function added or extended in any analysed file, not only by a changed test file. | looser | None. |
| v0.7.0 | `provenance-tags` | The superseded registry is the base registry plus the head registry; a pending statement citing only another repository's issue is a violation unless listed in `pending_issue_repos`. | stricter | List the tracking repository in `pending_issue_repos`. |
| v0.7.0 | report | A gate that examined nothing because its input is absent is "not evaluated", not passed: the summary adds "N not evaluated" and the `passed_gates` output drops by one. | reclassified | Read `status` / exit code, not `passed_gates`. |
| v0.7.0 | forge access | Forge features call the API over HTTPS in-process: `gh` and `curl` are no longer used and `DISCIPLINE_GH` is gone. GitHub tokens come from `GH_TOKEN` or `GITHUB_TOKEN`. | reclassified | Pass `GH_TOKEN` / `GITHUB_TOKEN` to the job. |
| v0.7.0 | action | `uses: orieg/discipline@v0` downloads the binary of the release the tag points at, not the newest release. | reclassified | None. |
| v0.7.0 | `ci-skip-set` | With `workflow` left at its default, the gate reads the running workflow (`GITHUB_WORKFLOW_REF`) or the first `ci.yml` under `.github/`, `.gitea/` or `.forgejo/workflows/`, instead of always `.github/workflows/ci.yml`. | reclassified | Set `workflow` to keep a fixed path. |
| v0.7.0 | submodules | A skipped submodule pointer change is named in the `deletion-rationale` (and `scope-confinement`) notes. | narrower | None. |
| v0.7.0 | packaging | MacPorts: the `Portfile` is a release asset (the in-tree copy is gone); the in-tree Homebrew formula is removed (the release generates it and updates the tap). | reclassified | Fetch `Portfile` from the release. |

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
- **Self-granted overrides:** A directive in the PR body or a commit body is written by the author of the change it excuses. `max_overrides`, `require_approval` and `--policy-from base` bound that, and all three are opt-in: a repository that sets none of them accepts every well-formed directive. Policy refusals appear in the terminal, step-summary and JSON reports; the JUnit, SARIF and GitLab reports carry gate findings only, so read the exit code.
- **What no static gate closes:** An implementation that special-cases the inputs its tests use, or a wrong change accompanied by plausible tests, passes every diff-based gate. The mutation presets of the `command` gate are the control for that class.
- **Option enumeration in `config-integrity`:** The test that requires every gate option to have a loosening direction enumerates options from the published schema and the serialized defaults. An `Option` field missing from the schema is the one shape it cannot see.
