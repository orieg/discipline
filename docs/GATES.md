---
layout: default
title: Gate Specifications & Enforcement Rules
permalink: /gates/
---

# Gate Specifications & Enforcement Rules

This document establishes the normative enforcement rules, detection capabilities, limits, and configuration keys for all gates in `discipline`.

**Superseded when:** A gate rule is amended, a new gate ships, or language-pack detection boundaries expand. Update in place; do not fork.

---

## Gate Catalog

<!-- generated:gates -->
| Gate id | Suite | Status | Languages | Rule |
|---|---|---|---|---|
| `agents-md` | agent-guard | **shipped** | any | AGENTS.md exists; CLAUDE.md / GEMINI.md do not fork it |
| `assertion-reduction` | agent-guard | **shipped** | Rust, Python, JS/TS, PHPT | assertion count / strength must not drop in an existing test |
| `vacuous-tests` | agent-guard | **shipped** | Rust, Python, JS/TS, PHPT | new tests must carry a non-tautological assertion |
| `ignored-tests` | agent-guard | **shipped** | Rust, Python, JS/TS, PHPT | tests must not be newly #[ignore]d |
| `unsafe-safety-comment` | agent-guard | **shipped** | Rust | unsafe blocks / impls carry a // SAFETY: comment |
| `deletion-rationale` | agent-guard | **shipped** | any | deleted files and removed tests need a scoped removes: rationale |
| `time-estimates` | hygiene | **shipped** | any | no calendar / duration estimates in markdown or the PR body |
| `pii` | hygiene | **shipped** | any | no home paths, LAN IPs, or denylisted hostnames in tracked text |
| `agent-scratch` | hygiene | **shipped** | any | agent scratch state is never tracked |
| `config-integrity` | integrity | **shipped** | any | a change cannot weaken its own discipline.toml without a token |
| `scope-confinement` | agent-guard | planned | any | changes stay inside authorized paths |
| `suppression-delta` | agent-guard | planned | per pack | new #[allow], commented-out tests, cfg-gated tests |
| `provenance-tags` | hygiene | planned | any | published numerics carry (measured|target|projected) |
| `ci-integrity` | integrity | planned | any | workflow weakening: continue-on-error, || true, unpinned actions |
| `test-floor` | integrity | planned | any | test-count ratchet read from the base ref |
| `golden-output` | integrity | **shipped** | any | prevents stealth edits to committed golden/test output files without explicit override |
| `pr-checklist` | hygiene | planned | any | ticked PR checkboxes are reconciled against the diff |
| `command` | verification | planned | any | fail-closed wrapper for any tool: zero-tests guard, canary, count ratchet |
| `sanitizers` | verification | planned | Rust, C/C++ | ASan / TSan preset with audited suppressions and a race canary |
| `msrv` | quality | planned | Rust | cargo check under the pinned MSRV |
| `miri` | verification | planned | Rust | Miri tiers with zero-tests guard |
| `unsafe-budget` | verification | planned | Rust | unsafe count ratchet |
| `bench-regression` | bench | **shipped** | Rust, Go, Python, C/C++ | benchmark drift via harness adapters (deterministic counts or BCa intervals) |
<!-- /generated -->

---

## Language Scope & Detection Boundaries

Seven gates are language-independent and inspect text, git diffs, or repository metadata: `deletion-rationale` (file level), `agents-md`, `time-estimates`, `pii`, `agent-scratch`, `config-integrity`, and `golden-output`. The four AST gates (`assertion-reduction`, `vacuous-tests`, `ignored-tests`, `unsafe-safety-comment`) operate through tree-sitter AST extraction.

When a change touches source files in a language without an active pack, each AST gate **names the unanalysed files in its report notes** (F7) rather than rendering a silent zero.

### Language Packs

| Language | Test function patterns | Assertion vocabulary (strong = equality / pattern) | Skip markers (`ignored-tests`) | Escape hatches (`unsafe-safety-comment`) | Status |
|---|---|---|---|---|---|
| **Rust** | `#[test]`, `#[tokio::test]`, `#[async_std::test]`, `#[rstest]` | `assert*!`, `debug_assert*!`, `prop_assert*!`; strong: `_eq`, `_ne`, `matches` | `#[ignore]`, `#[cfg_attr(..., ignore)]` | `unsafe` block / impl + `// SAFETY:` | **shipped** |
| **Python** | `test_*` functions, `Test*` methods, `unittest.TestCase` | `assert` statements, `self.assert*`, `pytest.raises`, `pytest.approx`; strong: `==`, `assertEqual` family | `@pytest.mark.skip` / `skipif` / `xfail`, `@unittest.skip*` | `# type: ignore`, `# noqa`, `# pragma: no cover` | **shipped** |
| **JavaScript / TypeScript** | `test(` / `it(` callbacks (Jest, Vitest, Mocha, node:test) | `expect(...).matcher`, `assert.*`; strong: `toBe`, `toEqual`, `toStrictEqual`; weak: `toBeTruthy`, `toBeDefined` | `.skip`, `.todo`, `xit`, `xdescribe` | `@ts-ignore`, `@ts-expect-error`, `as any` | **shipped** |
| **Golden (PHPT)** | Standard PHPT sections (`--TEST--`, `--FILE--`, `--EXPECT--`) | Exact expectation sections (`--EXPECT--`, `--EXPECTF--`, `--EXPECTREGEX--`) | `--SKIPIF--`, `--XFAIL--` | — | **shipped** |
| **Java / Kotlin** | `@Test`, `@ParameterizedTest`, `@RepeatedTest` | JUnit `assert*`, AssertJ `assertThat(...)`, Hamcrest; strong: `assertEquals`, `isEqualTo` | `@Disabled`, `@Ignore`, `Assumptions.*` | `@SuppressWarnings`, `sun.misc.Unsafe` | *planned* |
| **C / C++** | GoogleTest `TEST*`, Catch2 `TEST_CASE`, doctest | `EXPECT_*` / `ASSERT_*`, `REQUIRE` / `CHECK`; strong: `_EQ`, `_STREQ` | `DISABLED_` prefix, `GTEST_SKIP()` | `reinterpret_cast`, `const_cast`, `// NOLINT` | *planned* |
| **Go** | `func Test*(t *testing.T)`, subtests | `t.Error*` / `t.Fatal*`, testify `assert.*` / `require.*`; strong: `Equal`, `DeepEqual` | `t.Skip*` | `unsafe` package, `//nolint` | *planned* |

---

## Shipped Gates

### Pillar 1: Agent Conformance and Diff Guard (`agent-guard`)

#### `assertion-reduction`
- **Rule:** For each test present on both sides (matched by module-qualified name within a file, or by name across files for moved tests), neither the count of effective assertions nor the count of strong assertions may drop.
- **Languages:** Rust, Python, JavaScript / TypeScript, PHPT.
- **What it catches:**
  - Deleting assertion statements or macros within existing tests.
  - Assertion weakening (e.g. `assert_eq!(a, b)` -> `assert!(a == b)` or `assert!(a.is_some())`).
  - Replacing strong matchers with truthiness checks (e.g. `expect(x).toEqual(y)` -> `expect(x).toBeTruthy()`).
  - Replacing assertions with tautologies (`assert!(true)`, `assert_eq!(x, x)`).
- **Failing diff example (rejected):**
  ```rust
  // BASE:
  #[test]
  fn test_lookup() {
      assert_eq!(cache.get("key"), Some(&100));
  }

  // HEAD (weakened — rejected by assertion-reduction):
  #[test]
  fn test_lookup() {
      assert!(cache.get("key").is_some());
  }
  ```
- **What it does NOT catch:**
  - Assertions inside dynamically evaluated strings or macro expansions (e.g. `proptest! { ... }`).
  - Assertions inside unconfigured helper functions (configure via `assert_helper_fns` or `extra_assert_macros`).
  - Dynamic loops in Python (`@pytest.mark.parametrize` counts definitions, not iterations) or JS (`test.each`).
- **Lifting directive:** `allow-assertion-drop: <test-name> <reason>` in PR description or commit message.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `extra_assert_macros`, `assert_helper_fns`.

#### `vacuous-tests`
- **Rule:** A newly added test function must carry at least one non-tautological assertion, a configured assertion helper call, `.unwrap()` / `.expect()`, `?` in a fallible test returning `Result` or `Option`, or an expected panic attribute.
- **Languages:** Rust, Python, JavaScript / TypeScript, PHPT.
- **What it catches:**
  - Ghost tests containing only setup logic, variable bindings, or logging with zero assertions.
  - Verbatim tautologies: `assert_eq!(x, x)`, `assert_eq!(1, 1)`, `assert!(true)`.
  - Constant expression tautologies: `assert!(1 == 1)`, `assert!(1 + 1 > 0)`, `assert_ne!(1, 2)`.
  - Empty PHPT expectation sections.
- **Failing diff example (rejected):**
  ```python
  # Newly added test without non-tautological assertion — rejected by vacuous-tests:
  def test_compute():
      result = compute_values()
      assert 1 == 1
  ```
- **Passing diff example (accepted):**
  ```python
  def test_compute():
      result = compute_values()
      assert result == [10, 20, 30]
  ```
- **What it does NOT catch:**
  - Semantic non-assertions that involve external function calls (e.g. `assert!(check_validity())` where `check_validity()` returns `true` unconditionally).
  - Tests whose assertions occur in deeply nested helper callbacks not tracked by static analysis.
- **Lifting directive:** `allow-assertion-drop: <test-name> <reason>` or configuring `assert_helper_fns`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `extra_assert_macros`, `assert_helper_fns`.

#### `ignored-tests`
- **Rule:** An existing test may not become ignored or skipped, and a new test may not arrive skipped.
- **Languages:** Rust, Python, JavaScript / TypeScript, PHPT.
- **What it catches:**
  - Rust: `#[ignore]`, `#[cfg_attr(..., ignore)]`.
  - Python: `@pytest.mark.skip`, `@pytest.mark.skipif`, `@pytest.mark.xfail`, `@unittest.skip`, `@unittest.skipIf`.
  - JavaScript / TypeScript: `it.skip`, `test.skip`, `xit`, `xtest`, `describe.skip`, `xdescribe`, `it.todo`.
  - PHPT: newly added `--SKIPIF--` or `--XFAIL--` sections.
- **Failing diff example (rejected):**
  ```typescript
  // Skipping failing test instead of fixing — rejected by ignored-tests:
  test.skip('parses unicode payload', () => {
    expect(parse('payload')).toBeDefined();
  });
  ```
- **What it does NOT catch:**
  - Conditional runtime early-returns (`if condition { return; }`).
  - Dynamic test framework skips invoked within function bodies (`pytest.skip(...)`).
  - Commented-out test functions (covered by planned `suppression-delta`).
- **Lifting directive:** `allow-ignore: <test-name> <reason>`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`.

#### `unsafe-safety-comment`
- **Rule:** Every `unsafe` block, `unsafe fn`, or `unsafe impl` on an added line must be preceded by a load-bearing `// SAFETY:` comment. Deleting a `// SAFETY:` comment above an existing block is also blocked.
- **Languages:** Rust.
- **What it catches:**
  - Unsafe blocks or impls without preceding `// SAFETY:` comments.
  - Deletion of an existing `// SAFETY:` comment above an untouched `unsafe` block.
  - Vacuous placeholder comments: `// SAFETY: todo`, `// SAFETY: tbd`, `// SAFETY: safe`, `// SAFETY: trust me`, `// SAFETY: noop`, `// SAFETY: fine`.
  - Misplaced comments (comments trailing after the block or lowercase `safety:`).
- **Failing diff example (rejected):**
  ```rust
  // Missing justification — rejected by unsafe-safety-comment:
  let val = unsafe { *ptr };

  // Placeholder comment — rejected by unsafe-safety-comment:
  // SAFETY: safe
  let val = unsafe { *ptr };
  ```
- **Passing diff example (accepted):**
  ```rust
  // SAFETY: ptr is guaranteed non-null, 8-byte aligned, and points to
  // an initialized u64 allocated in buffer_init().
  let val = unsafe { *ptr };
  ```
- **What it does NOT catch:**
  - Flawed or mathematically invalid justifications (static AST cannot verify human semantic correctness beyond placeholder rejection).
  - `unsafe` hidden inside macro invocations outside tree-sitter Rust AST parsing.
- **Lifting directive:** Requires providing a substantive `// SAFETY:` comment naming invariants, or `exempt_paths`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `placeholders`.

#### `deletion-rationale`
- **Rule:** Deleted files and removed tests require an explicit, scoped `removes:` or `deletes:` rationale in the PR description or commit message.
- **Languages:** Any (files); Rust, Python, JS/TS, PHPT (removed test functions).
- **What it catches:**
  - Silent file deletions across all tracked paths.
  - Silent test removals from surviving test files.
- **Failing diff example (rejected):**
  ```diff
  - deleted file: tests/test_concurrency.rs
  ```
  *(Without `removes:` directive in commit message or PR body — rejected by deletion-rationale)*
- **Passing commit / PR body (accepted):**
  ```text
  removes: tests/test_concurrency.rs replaced by proptest model in tests/test_model.rs
  ```
- **What it does NOT catch:**
  - File renames where `git` detects similarity above rename thresholds (properly treated as modifications).
- **Lifting directive:** `removes: <path-or-test> <reason>` or `deletes: <path-or-test> <reason>`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `paths`.

#### `agents-md`
- **Rule:** `AGENTS.md` must exist at the repository root. Tracked agent guide files (`CLAUDE.md`, `GEMINI.md`) must be symbolic links to `AGENTS.md` or textually identical to prevent split-brain instructions.
- **Languages:** Any.
- **What it catches:**
  - Missing `AGENTS.md`.
  - Independent or divergent edits made directly to `CLAUDE.md` or `GEMINI.md`.
- **Passing setup (accepted):**
  ```bash
  ln -sf AGENTS.md CLAUDE.md
  ln -sf AGENTS.md GEMINI.md
  ```
- **What it does NOT catch:**
  - Non-standard guide names outside `CLAUDE.md`, `GEMINI.md`, and `AGENTS.md`.
- **Lifting directive:** Ensure `CLAUDE.md` and `GEMINI.md` are symlinks: `ln -sf AGENTS.md CLAUDE.md`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`.

---

### Pillar 2: Hygiene (`hygiene`)

#### `time-estimates`
- **Rule:** No calendar or duration estimates in tracked markdown documentation or the PR body.
- **Languages:** Any.
- **What it catches:**
  - Calendar intervals: "1-2 days", "3 weeks", "next sprint", "Q2", "Phase 2 (1 week)". <!-- discipline:allow(time-estimates) -->
  - Aggregate durations: "~10 engineer-days", "three deliverables in 2 weeks". <!-- discipline:allow(time-estimates) -->
- **Failing diff example (rejected):**
  ```markdown
  ### Phase 2: Complete AST Parser (estimated: 2 weeks)
  ```
- **Passing diff example (accepted):**
  ```markdown
  ### Phase 2: Complete AST Parser (blocked on grammar stabilization)
  ```
- **What it does NOT catch:**
  - Operational TTLs, cache expiration, and timeouts (`timeout: 30s`, `retention: 7 days`). <!-- discipline:allow(time-estimates) -->
  - Benchmark measurements ("ran in 4.2 seconds").
  - Historical durations ("was maintained for three years"). <!-- discipline:allow(time-estimates) -->
  - Code inside fenced blocks (` ``` `).
- **Lifting directive:** In markdown: `<!-- discipline:allow(time-estimates) -->` on the matching line.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `include`, `extra_patterns`, `allow_patterns`, `scan_pr_body`.

#### `pii`
- **Rule:** No leaked developer workstation home directories, private RFC 1918 LAN IPs, or denylisted hostnames in tracked text files or the PR body.
- **Languages:** Any.
- **What it catches:**
  - Local home directory paths: `/Users/<username>/...`, `/home/<username>/...`, `C:\Users\<username>\...`.
  - Private IPv4 LAN addresses: `10.x.x.x`, `172.16-31.x.x`, `192.168.x.x`.
  - Whole-token matches of denylisted internal hostnames.
  - Leaks inside decoded JSON string literals.
- **Failing diff example (rejected):**
  ```rust
  // Workstation path leak — rejected by pii:
  let default_path = "/Users/dev-user/project/data.bin"; // discipline:allow(pii)
  let target_node = "192.168.1.42"; // discipline:allow(pii)
  ```
- **Passing diff example (accepted):**
  ```rust
  let default_path = "/var/data/project/data.bin";
  let target_node = "127.0.0.1";
  ```
- **What it does NOT catch:**
  - Standard documentation placeholders: `runner`, `user`, `username`, `example`, `shared`.
  - RFC 1918 CIDR network notations in routing documentation (`10.0.0.0/8`, `192.168.0.0/16`).
  - Binary files (non-text).
- **Lifting directive:** `<!-- discipline:allow(pii) -->` on the matching line.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `home_paths`, `lan_ips`, `allowed_users`, `hostname_denylist`, `extra_patterns`, `allow_patterns`, `scan_pr_body`.

#### `agent-scratch`
- **Rule:** Agent transcripts, session files, and scratch artifacts must never be tracked in git.
- **Languages:** Any.
- **What it catches:**
  - Committing directories: `.claude/`, `.gemini/`, `.antigravity/`, `.cursor/`, `scratch/`.
  - Session files: `*.session.*`, `.aider*`.
- **Failing commit (rejected):**
  ```bash
  git add .gemini/scratch/notes.md && git commit -m "add scratch notes"
  ```
- **Passing setup (accepted):**
  Agent artifacts kept untracked or in `.gitignore`:
  ```text
  .claude/
  .gemini/
  .antigravity/
  *.session.*
  scratch/
  ```
- **What it does NOT catch:**
  - Files untracked in `.gitignore` (safely ignored).
- **Lifting directive:** Remove tracked scratch files from git (`git rm --cached`).
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `paths`.

---

### Pillar 3: Integrity (`integrity`)

#### `config-integrity`
- **Rule:** A change cannot loosen its own `discipline.toml` configuration without an explicit override directive.
- **Languages:** Any.
- **What it catches:**
  - Disabling a gate (`enabled = false`).
  - Lowering severity (`severity = "error"` -> `severity = "warning"`).
  - Growing loosening lists (`exempt_paths`, `allowed_users`, `allow_patterns`, `assert_helper_fns`).
  - Shrinking tightening lists (`paths`, `include`, `hostname_denylist`).
  - Deleting `discipline.toml`.
- **Failing diff example (rejected):**
  ```diff
  [gates.vacuous-tests]
  -enabled = true
  +enabled = false
  ```
  *(Without `allow-gate-weakening: vacuous-tests <reason>` — rejected by config-integrity)*
- **Passing PR description (accepted):**
  ```text
  allow-gate-weakening: vacuous-tests test suite refactor in progress
  ```
- **What it does NOT catch:**
  - Tightening edits (enabling gates, adding denylists, raising severity) — tightening is permitted freely.
  - Workflow-level switches (`disable:` in GitHub Actions steps) — protected by planned `ci-integrity`.
- **Lifting directive:** `allow-gate-weakening: <gate-id> <reason>` in PR description or commit message.
- **Config keys:** `enabled`, `severity`, `exempt_paths`.

#### `golden-output`
- **Rule:** Committed test snapshots, golden outputs, and recorded fixtures cannot be modified or deleted without an explicit scoped rationale.
- **Languages:** Any.
- **What it catches:**
  - Edits or deletions of files matching `paths` (`**/golden/**`, `**/snapshots/**`, `**/*.snap`, `tests/fixtures/**/output*`).
  - Stealth snapshot re-blessing to mask test regressions.
- **Failing diff example (rejected):**
  ```diff
  // Modified golden output file: tests/golden/api_response.json
  - "status": "active", "count": 42
  + "status": "unknown", "count": 0
  ```
- **Passing commit / PR description (accepted):**
  ```text
  allow-golden-update: tests/golden/api_response.json schema upgrade for version 2 endpoint
  ```
- **What it does NOT catch:**
  - Newly added snapshot files for newly created tests.
- **Lifting directive:** `allow-golden-update: <path> <reason>`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `paths`.

---

### Pillar 5: Benchmark Drift (`bench`)

#### `bench-regression`
- **Rule:** Benchmark output files are tracked across revisions. Comparisons against merge-base baselines enforce formal mathematical confidence intervals and exact deterministic instruction counts.
- **Languages:** Rust, C/C++, Go, Python.
- **What it catches:**
  - Regressions where conservative confidence intervals clear the tolerance threshold ($\Delta_{min} = \frac{L_{head} - U_{base}}{U_{base}} > \text{tolerance}$).
  - Exact instruction count regressions in Callgrind / IAI outputs (`events: Ir`).
  - Missing merge-base benchmark artifacts (fails closed with exit 2).
  - Garbage or corrupted benchmark output files (fails closed with exit 2).
  - Deleted benchmark files without authorization (exit 1).
  - Benchmarks renamed away without baseline (exit 1).
  - Unmatched host/runner provenance tags between base and head.
- **Degradation without failure:**
  When wall-clock benchmarks lack confidence intervals on either base or head, the engine degrades the verdict to **"not comparable (no CI available)"** in notes and does not fail the build on bare point estimates.
- **Passing override directive (accepted):**
  ```text
  allow-regression: search_bench intentional algorithmic trade-off for zero-allocation scan
  ```
- **What it does NOT catch:**
  - Uncommitted benchmark results (benchmark files must be committed or generated in CI workspace).
  - Wall-clock variance from co-resident CPU contention without sample distribution statistics.
- **Lifting directive:** `allow-regression: <benchmark-name-or-path> <reason>`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `tolerance_pct`, `paths`, `provenance`, `allow_cross_host`.

---

## Planned Gates

The following gates are registered with `available: false` in the gate registry. Attempting to enable or configure them exits non-zero (F5). Full roadmaps, dependencies, and go/no-go gates are documented in [ROADMAP.md](ROADMAP.md).

- `scope-confinement` (Suite: Agent Guard) — Changes stay inside authorized directory paths.
- `suppression-delta` (Suite: Agent Guard) — Tracks net increases in compiler/linter suppression attributes (`#[allow]`, `@ts-ignore`, `# noqa`).
- `provenance-tags` (Suite: Hygiene) — Published numeric claims must carry `(measured)`, `(target)`, or `(projected)`.
- `ci-integrity` (Suite: Integrity) — Detects workflow weakening (`continue-on-error`, dropped `needs`, unpinned actions).
- `test-floor` (Suite: Integrity) — Test count ratchet read directly from the base ref.
- `pr-checklist` (Suite: Hygiene) — Reconciles ticked PR checkboxes against actual diffs.
- `command` (Suite: Verification) — Fail-closed wrapper for external tools with zero-test guards and canary checks.
- `sanitizers` (Suite: Verification) — Memory and thread sanitizer presets with race canaries.
- `msrv` (Suite: Quality) — Verifies build against minimum supported Rust version.
- `miri` (Suite: Verification) — Undefined behavior verification under Miri with zero-test guards.
- `unsafe-budget` (Suite: Verification) — Ratchet limiting the total number of `unsafe` blocks.
