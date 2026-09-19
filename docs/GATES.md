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
| Gate id | Suite | Status | Languages | Rule | Example / Enforces |
|---|---|---|---|---|---|
| `agents-md` | agent-guard | **shipped** | any | AGENTS.md exists; CLAUDE.md / GEMINI.md do not fork it | Catches missing AGENTS.md or divergent non-symlinked CLAUDE.md / GEMINI.md |
| `assertion-reduction` | agent-guard | **shipped** | Rust, Python, JS/TS, PHPT, Java, Go, PHP, C/C++, C#, Ruby | assertion count / strength must not drop in an existing test | Rejects assert_eq!(a, b) replaced by assert!(true) or removed assertions |
| `vacuous-tests` | agent-guard | **shipped** | Rust, Python, JS/TS, PHPT, Java, Go, PHP, C/C++, C#, Ruby | new tests must carry a non-tautological assertion | Rejects empty test functions or tests without any executable assertions |
| `ignored-tests` | agent-guard | **shipped** | Rust, Python, JS/TS, PHPT, Java, Go, PHP, C/C++, C#, Ruby | tests must not be newly #[ignore]d | Rejects newly added #[ignore] or @pytest.mark.skip without directive |
| `unsafe-safety-comment` | agent-guard | **shipped** | Rust | unsafe blocks / impls carry a // SAFETY: comment | Rejects unsafe blocks or impls lacking a preceding // SAFETY: comment |
| `deletion-rationale` | agent-guard | **shipped** | any | deleted files and removed tests need a scoped removes: rationale | Rejects deleted files or removed tests lacking removes: <path> <reason> |
| `time-estimates` | hygiene | **shipped** | any | no calendar / duration estimates in markdown or the PR body | Rejects calendar deadlines, sprint projections, and duration estimates |
| `pii` | hygiene | **shipped** | any | no home paths, LAN IPs, or denylisted hostnames in tracked text | Rejects committed workstation home directories, private LAN IPs, and hostnames |
| `agent-scratch` | hygiene | **shipped** | any | agent scratch state is never tracked | Rejects committed agent scratch state like .claude/plans/ or scratch/*.tmp |
| `config-integrity` | integrity | **shipped** | any | a change cannot weaken its own discipline.toml without a token | Rejects changes disabling gates in discipline.toml without explicit token |
| `scope-confinement` | agent-guard | planned | any | changes stay inside authorized paths | Rejects diffs modifying files outside authorized path boundaries |
| `suppression-delta` | agent-guard | planned | per pack | new #[allow], commented-out tests, cfg-gated tests | Rejects newly added linter suppresses or commented-out test functions |
| `provenance-tags` | hygiene | planned | any | published numerics carry (measured|target|projected) | Rejects published benchmark numerics lacking (measured|target|projected) |
| `ci-integrity` | integrity | planned | any | workflow weakening: continue-on-error, || true, unpinned actions | Rejects workflow weakening such as continue-on-error: true or unpinned actions |
| `test-floor` | integrity | planned | any | test-count ratchet read from the base ref | Rejects net test count dropping below the merge-base baseline |
| `golden-output` | integrity | **shipped** | any | prevents stealth edits to committed golden/test output files without explicit override | Rejects edits to committed golden/snapshot fixture files without override |
| `dependency-delta` | integrity | **shipped** | any | manifest diff inspection: zero wildcards, source/license allowlists, and deny.toml verification | Rejects unapproved new dependencies, wildcard versions, or unpinned git sources |
| `test-budget` | integrity | **shipped** | Rust, Python, JS/TS, Go, any | property-test and fuzz effort ratchet (cases, shrink iters, fuzztime, seed corpus) | Rejects reductions in proptest cases, shrink iterations, or fuzz durations |
| `pr-checklist` | hygiene | planned | any | ticked PR checkboxes are reconciled against the diff | Rejects ticked PR checklist items that have no matching diff changes |
| `command` | verification | **shipped** | any | fail-closed wrapper for any tool: zero-tests guard, canary, count ratchet | Wraps cargo test or custom tools to enforce non-zero exit and test execution |
| `sanitizers` | verification | planned | Rust, C/C++ | ASan / TSan preset with audited suppressions and a race canary | Enforces clean memory/thread sanitizer runs without unreviewed suppressions |
| `msrv` | quality | planned | Rust | cargo check under the pinned MSRV | Validates compilation under the minimum supported toolchain version |
| `miri` | verification | planned | Rust | Miri tiers with zero-tests guard | Runs undefined behavior checks ensuring test suites are not silently bypassed |
| `unsafe-budget` | verification | planned | Rust | unsafe count ratchet | Rejects any net increase in total unsafe block count across the project |
| `bench-regression` | bench | **shipped** | Rust, Go, Python, C/C++ | benchmark drift via harness adapters (deterministic counts or BCa intervals) | Detects CPU instruction regressions and non-overlapping BCa confidence intervals |
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
| **Java** | `@Test`, `@ParameterizedTest`, `@RepeatedTest` (JUnit 4/5, TestNG) | JUnit `assert*`, AssertJ `assertThat(...)`; strong: `assertEquals`, `assertThrows`, `isEqualTo` | `@Disabled`, `@Ignore`, `@Test(enabled = false)` | `@SuppressWarnings` | **shipped** |
| **Kotlin** | `@Test`, `@ParameterizedTest` | JUnit `assert*`, AssertJ, Kotest | `@Disabled`, `@Ignore` | `@Suppress` | *planned* |
| **C / C++** | GoogleTest `TEST*`, Catch2 `TEST_CASE`, doctest, C ABI smoke (`main`, `test_*`) | `EXPECT_*` / `ASSERT_*`, `REQUIRE` / `CHECK`, `assert(...)`; strong: `_EQ`, `_STREQ`, comparison operators | `DISABLED_` prefix, `GTEST_SKIP()`, Catch2 `SKIP()` / `[.]` | `// NOLINT` | **shipped** |
| **Go** | `func Test*(t *testing.T)`, subtests | `t.Error*` / `t.Fatal*`, testify `assert.*` / `require.*`; strong: `Equal`, `DeepEqual` | `t.Skip*` | `unsafe` package, `//nolint` | **shipped** |
| **PHP** | PHPUnit `test*` methods, `@test` docblock / attribute, Pest `test(` / `it(` | PHPUnit `assert*`, Pest matchers (`->toBe`, `->toEqual`); strong: `assertEquals`, `assertSame`, `assertCount` | `$this->markTestSkipped()`, `$this->markTestIncomplete()`, `->skip()`, `#[Requires*]` | `// @psalm-suppress`, `// @phpstan-ignore`, `// phpcs:ignore` | **shipped** |
| **C#** | `[Fact]`, `[Theory]` (xUnit), `[Test]` (NUnit), `[TestMethod]` (MSTest) | `Assert.*`, `StringAssert.*`, `CollectionAssert.*`; strong: `Equal`, `True`, `Throws` | `[Ignore]`, `[Fact(Skip = "...")]` | `#pragma warning disable`, `[SuppressMessage]` | **shipped** |
| **Ruby** | `def test_*` (Minitest, Test::Unit), `it` / `specify` (RSpec) | `assert_*`, `refute_*`, RSpec `expect(...).to eq(...)`; strong: `assert_equal`, `eq` | `xit`, `xdescribe`, `:skip`, `skip` | `# rubocop:disable` | **shipped** |

---

## Shipped Gates

### Pillar 1: Agent Conformance and Diff Guard (`agent-guard`)

#### `assertion-reduction`
- **Rule:** For each test present on both sides (matched by module-qualified name within a file, or by name across files for moved tests), neither the count of effective assertions nor the count of strong assertions may drop.
- **Languages:** Rust, Python, JavaScript / TypeScript, PHPT, Java, Go, PHP, C/C++, C#, Ruby.
- **What it catches:**
  - Deleting assertion statements or macros within existing tests.
  - Assertion weakening (e.g. `assert_eq!(a, b)` -> `assert!(a == b)` or `assert!(a.is_some())`).
  - Replacing strong matchers with truthiness checks (e.g. `expect(x).toEqual(y)` -> `expect(x).toBeTruthy()`).
  - Replacing assertions with tautologies (`assert!(true)`, `assert_eq!(x, x)`).
  - Deleting compile-time invariant assertions outside tests (e.g. `const _: () = assert!(...);`, `static_assertions::*`, `const_assert!`, C/C++ `static_assert`).
- **Compile-Time Invariant Protection:**
  In addition to test functions, `assertion-reduction` tracks compile-time assertions outside test functions (struct sizes, field alignments, type layout invariants, and C/C++ `static_assert`). Deleting or removing compile-time guards triggers an assertion reduction violation on `Test compile-time-assertions`:
  ```rust
  // BASE (protected layout guard):
  const _: () = assert!(std::mem::size_of::<PacketHeader>() == 64);
  const _: () = assert!(std::mem::align_of::<PacketHeader>() == 8);

  // HEAD (align_of guard stealthily deleted — rejected):
  const _: () = assert!(std::mem::size_of::<PacketHeader>() == 64);
  ```
  Can be lifted when intentionally modifying type layouts using `allow-assertion-drop: compile-time-assertions <reason>` or `allow-assertion-drop: <file> <reason>`.
- **Zero-Allocation Test Pattern:**
  High-assurance repositories enforce zero-allocation invariants in critical hot paths by asserting allocator counters (e.g., comparing before-and-after allocated byte counts) or calling zero-allocation harnesses (`assert_no_alloc(|| { ... })`). Existing gates protect this pattern end-to-end without requiring an intrusive runtime allocator sentinel inside Discipline:
  - If an agent removes or weakens the allocation assertion (`assert_eq!(before, after)`), `assertion-reduction` catches the reduction.
  - If an agent empties the allocation test body or uses a constant check, `vacuous-tests` rejects it.
  - If an agent stealth-deletes the zero-allocation test function, `deletion-rationale` blocks the deletion unless justified with `removes:`.
  - If an agent marks the test `#[ignore]` or skips it, `ignored-tests` flags the change.
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
  - Assertions inside unconfigured helper functions (configure via `assert_helper_fns` or `extra_assert_macros`, though same-file helper functions are resolved automatically in supported packs).
  - Dynamic loops in Python (`@pytest.mark.parametrize` counts definitions, not iterations) or JS (`test.each`).
  - Dynamic branch reachability: assertions inside unreachable branches (e.g. `if False:`, `if (0) { ... }`, or dead closures) are counted by the AST parser because runtime execution reachability is out of scope for static analysis.
- **Lifting directive:** `allow-assertion-drop: <test-name> <reason>` in PR description or commit message.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `extra_assert_macros`, `assert_helper_fns`.

#### `vacuous-tests`
- **Rule:** A newly added test function must carry at least one non-tautological assertion, a configured assertion helper call, `.unwrap()` / `.expect()`, `?` in a fallible test returning `Result` or `Option`, or an expected panic attribute.
- **Languages:** Rust, Python, JavaScript / TypeScript, PHPT, Java, Go, PHP, C/C++, C#, Ruby.
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
  - Dynamic branch reachability: assertions inside unreachable branches (e.g. `if False:`, `if (0) { ... }`, or dead closures) are counted as syntactically present by AST static analysis because dynamic runtime reachability is out of scope.
- **Per-pack resolution contracts & known limits:**
  - **Go**: Functions must match `TestXxx` or `FuzzXxx` with `*testing.T` or `*testing.F` parameters. Lowercase helpers (e.g. `testNewRouter`) and `testing.TB` interfaces are treated as helper functions. Direct same-file helper calls are resolved 1 level deep. *Known limit*: Indirect closure assertions (e.g. assertions inside HTTP handler or router callbacks executed indirectly via `router.ServeHTTP(rw, req)`) appear vacuous without `assert_helper_fns = ["ServeHTTP"]` or direct assertions in the test body.
  - **Java**: 1-level same-file helper resolution covers direct helper methods within the test class. Custom domain assertion methods named `assert*` with an uppercase following character (e.g. `assertMetaDataIsEqualTo`, `assertPreconditionViolationFor`) are recognized automatically. *Known limit*: Assertions dispatched through external test fixture classes or mock framework verifiers outside the file require `assert_helper_fns`.
  - **C / C++**: Catches GoogleTest, Catch2, doctest assertions. *Known limit*: Heavy preprocessor macro constructs (`#ifdef`, complex template metaprogramming) are gracefully downgraded to `Warning` severity with line numbers; surrounding well-formed AST regions continue to be inspected.
  - **C#**: Recognizes standard xUnit (`[Fact]`, `[Theory]`), NUnit (`[Test]`), MSTest (`[TestMethod]`) attributes and resolves 1-level same-file helpers. *Known limit*: Multi-targeting `#if` preprocessor branches inside expressions are gracefully downgraded to `Warning` severity with line numbers.
  - **Ruby**: Only methods prefixed with `test_` (or named `test`) in Minitest/Test::Unit and RSpec `it`/`specify` blocks are extracted as test cases. Lifecycle hooks (`setup`, `teardown`) are excluded from vacuous checks. Same-file helper method assertions are resolved 1 level deep.
  - **PHP / PHPT**: PHPT sections require explicit `--EXPECT--`, `--EXPECTF--`, or `--EXPECTREGEX--`. PHPUnit methods require `$this->assert*` or configured helpers. Newly added NUL bytes in source fixtures are flagged as potential corruption and require `allow-nul:` or `discipline:allow(assertion-reduction)`.
- **Lifting directive:** `allow-assertion-drop: <test-name> <reason>` or configuring `assert_helper_fns`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `extra_assert_macros`, `assert_helper_fns`, `min_assertions_per_test`.

#### `ignored-tests`
- **Rule:** An existing test may not become ignored or skipped, and a new test may not arrive skipped.
- **Languages:** Rust, Python, JavaScript / TypeScript, PHPT, Java, Go, PHP, C/C++, C#, Ruby.
- **What it catches:**
  - Rust: `#[ignore]`, `#[cfg_attr(all(), ignore)]` (conditional skips like `#[cfg_attr(miri, ignore)]` emit a warning and do not count as unconditional ignores).
  - Python: `@pytest.mark.skip`, `@pytest.mark.skipif`, `@pytest.mark.xfail`, `@unittest.skip`, `@unittest.skipIf`.
  - JavaScript / TypeScript: `it.skip`, `test.skip`, `xit`, `xtest`, `describe.skip`, `xdescribe`, `it.todo`.
  - PHPT: newly added `--SKIPIF--` or `--XFAIL--` sections.
  - Java: `@Disabled`, `@Ignore`, `@Test(enabled = false)` (including class-level annotations propagating to all methods).
  - Go: `t.Skip`, `t.Skipf`, `t.SkipNow`.
  - PHP: `$this->markTestSkipped()`, `$this->markTestIncomplete()`, `->skip()`, `#[Requires*]`, `@group skip`, `@skip` (including class docblock propagation).
  - Ruby: `xit`, `xdescribe`, `xcontext`, `:skip`, `skip: true` (including hierarchical propagation from outer blocks).
  - C / C++: `DISABLED_` test or suite prefix, `GTEST_SKIP()`, Catch2 `SKIP()` or `[.]`/`[!hide]` hidden tags.
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

#### `dependency-delta`
- **Rule:** Universal manifest diff inspection and dependency sentinel across languages. Only newly added or modified dependencies in the diff against the merge-base ref are evaluated. Enforces zero wildcards, immutable commit or release tag pins on git sources, repo-level `deny.toml` verification (banned crates, sources, wildcards), and configured allow/deny dependency lists.
- **Languages:** any (Cargo.toml, package.json, pyproject.toml, requirements*.txt, go.mod, composer.json, Gemfile, *.csproj, Directory.Packages.props).
- **What it catches:**
  - Wildcard or unconstrained dependency version specifications (`*`, `latest`, empty version string).
  - Unpinned git dependencies (floating branches like `branch = "main"` without explicit commit SHA or tag).
  - Newly introduced dependencies that violate repository `deny.toml` `[bans]` or `[sources]`.
  - Dependencies listed in configured `deny_dependencies`.
  - Newly added dependencies not present in configured `allow_dependencies` (when configured).
- **Failing diff example (rejected):**
  ```diff
  // Cargo.toml
  + serde = "*"
  + unsafe-unpinned-lib = { git = "https://github.com/org/repo.git", branch = "main" }
  ```
- **Passing commit / PR description (accepted):**
  ```text
  allow-dependency: serde temporary unpinned version for testing
  allow-dependency: unsafe-unpinned-lib tracking upstream experimental branch
  ```
- **What it does NOT catch:**
  - Unmodified pre-existing dependencies already present in the merge base ref.
  - Dependencies explicitly excused via scoped `allow-dependency: <name> <reason>`.
- **Lifting directive:** `allow-dependency: <dependency-name> <reason>`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `manifests`, `allow_wildcards`, `require_git_pins`, `deny_file`, `allow_dependencies`, `deny_dependencies`.

#### `test-budget`
- **Rule:** Universal property-test and fuzz effort ratchet across languages and CI workflows. Property-testing iterations, shrink limits, fuzzing durations, fuzz targets, and seed corpus directories cannot be lowered without an explicit scoped directive.
- **Languages:** Rust, Python, JS/TS, Go, any workflow/script.
- **What it catches:**
  - Reductions in Rust `proptest` (`cases`, `max_shrink_iters`) and `quickcheck` (`tests`, `gen_size`).
  - Reductions in Python `hypothesis` (`max_examples`, `deadline`).
  - Reductions in JS/TS `fast-check` (`numRuns`).
  - Lowered fuzzing or test effort in workflows and shell scripts (`PROPTEST_CASES`, `-max_total_time`, `-runs`, Go fuzz `-fuzztime`).
  - Removal of fuzz targets from `fuzz/Cargo.toml` (`[[bin]] name = "..."`) or deletion of `fuzz/fuzz_targets/*.rs`.
  - Shrunken seed corpus directories or deleted seed files (`fuzz/corpus/**`, `corpus/**`).
- **Failing diff example (rejected):**
  ```diff
  // tests/prop.rs
  - cases: 10000
  + cases: 1000
  ```
- **Passing commit / PR description (accepted):**
  ```text
  allow-test-shrink: proptest cases trimmed for faster local iteration in dev branch
  ```
- **What it does NOT catch:**
  - Increases or additions of property-testing iterations or new fuzz targets (ratchet permits tightening).
  - Reductions explicitly excused by scoped directive `allow-test-shrink: <target/metric> <reason>`.
- **Lifting directive:** `allow-test-shrink: <target-or-metric> <reason>` or `allow-test-budget: <target-or-metric> <reason>`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `corpus_dirs`, `fuzz_targets`, `scan_workflows`, `scan_scripts`.

---

### Pillar 4: Verification Suite (`verification`)

#### `command`
- **Rule:** Universal, language-neutral fail-closed wrapper for external verification commands. Discipline executes the command directly without shell pipes, captures stdout/stderr concurrently, bounds runtime (timeout = exit 2), detects missing binaries in PATH (exit 2), enforces a test count ratchet against the merge base ref, forbids declared output patterns, fails when zero items/tests are executed, verifies negative-control canaries produce stated diagnostics, and protects preset policy files against stealth deletion.
- **Untrusted PR Text Guard:** PR diffs cannot alter or introduce commands or preset selections in `discipline.toml` without runner environment authorization (`DISCIPLINE_COMMAND` or `DISCIPLINE_ALLOW_COMMAND_CHANGE`).
- **Turnkey Presets:** Turnkey data-driven configurations providing pre-calibrated defaults for common high-assurance tools:
  - **Diff-Scoped Mutation Testing:** `cargo-mutants` (`cargo mutants --in-diff`, zero-mutants guard `0 mutants tested`, forbids `survived`, `MISSED`), `mutmut` (`mutmut run`), `stryker` (`npx stryker run`), `pit` (`mvn org.pitest:pitest-maven:mutationCoverage`).
  - **Diff Coverage:** `lcov` (`lcov --summary lcov.info`), `cobertura` (`coverage.xml`).
  - **Semver & API Compatibility:** `cargo-semver-checks` (`cargo semver-checks check-release`), `api-snapshot` (`git diff --exit-code api.snapshot`).
  - **Supply Chain & Advisory Wrappers:** `cargo-deny` (`cargo deny check`, guarded policy file `deny.toml`), `pip-audit` (`pip-audit`), `npm-audit` (`npm audit --audit-level=high`), `govulncheck` (`govulncheck ./...`).
  - **Deterministic Concurrency Testing:** `loom` (`cargo test --test loom -- --nocapture`, zero-tests guard `running 0 tests`).
- **Languages:** any.
- **What it catches:**
  - Non-zero command exit codes (exit 1).
  - Missing binaries in `PATH` (fails closed with exit 2).
  - Command timeouts exceeding `timeout_seconds` (fails closed with exit 2).
  - Forbidden strings or regexes detected in stdout or stderr.
  - Zero tests or items executed when `allow_zero = false`.
  - Stealth deletion of preset policy files (e.g. `deny.toml`, `api.snapshot`).
  - Extracted count dropping below the `min_count` ratchet floor established on the merge base ref.
  - Negative-control canaries failing to produce their declared diagnostic message or unexpectedly succeeding.
- **Passing override directive (accepted):**
  ```text
  allow-command: cargo-mutants no mutants generated on documentation diff
  ```
- **Lifting directive:** `allow-command: <command-or-preset-name> <reason>`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `preset`, `command`, `timeout_seconds`, `count_pattern`, `min_count`, `forbid_output`, `zero_items_pattern`, `allow_zero`, `canary_command`, `canary_expected_diagnostic`, `commands`.

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
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `tolerance_pct`, `paths`, `provenance`, `allow_cross_host`, `max_noise_cv`, `noise_margin_pct`.

---

## Preset Profile Configurations

The following curated configuration profiles provide turn-key setups tailored for specific engineering environments. Copy the desired profile directly into `discipline.toml` at the repository root.

### Profile 1: Research & High-Assurance Algorithm Labs

Tailored for scientific computing, cryptographic libraries, and high-assurance algorithmic cores. Enforces zero-tolerance benchmark drift with statistical variance guards, strict hygiene (zero time estimates, PII redaction), property-test ratchets, and immutability of golden outputs.

```toml
schema_version = 1
fail_on_warnings = true
fail_on_overrides = false

[directives]
sources = ["pr-body", "commits"]
allow_hidden = false

[gates.assertion-reduction]
enabled = true
severity = "error"

[gates.vacuous-tests]
enabled = true
severity = "error"
min_assertions_per_test = 1

[gates.ignored-tests]
enabled = true
severity = "error"

[gates.unsafe-safety-comment]
enabled = true
severity = "error"

[gates.deletion-rationale]
enabled = true
severity = "error"

[gates.time-estimates]
enabled = true
severity = "error"

[gates.pii]
enabled = true
severity = "error"
allow_lan_ips = false

[gates.agent-scratch]
enabled = true
severity = "error"

[gates.config-integrity]
enabled = true
severity = "error"

[gates.golden-output]
enabled = true
severity = "error"

[gates.dependency-delta]
enabled = true
severity = "error"
deny_wildcards = true
enforce_deny_toml = true
enforce_git_pins = true

[gates.test-budget]
enabled = true
severity = "error"
ratchet = true

[gates.bench-regression]
enabled = true
severity = "error"
tolerance_pct = 3.0
max_noise_cv = 0.15
noise_margin_pct = 2.0
provenance = true
allow_cross_host = false
```

### Profile 2: Enterprise Backend & Systems Services

Tailored for production web services, distributed systems, and enterprise microservices. Focuses on multi-language test coverage preservation, supply chain audit verification, PII redaction, and preventing silent test suppression.

```toml
schema_version = 1
fail_on_warnings = false
fail_on_overrides = false

[directives]
sources = ["pr-body"]
allow_hidden = false

[gates.assertion-reduction]
enabled = true
severity = "error"
assert_helper_fns = ["check_response_ok", "assert_valid_record"]

[gates.vacuous-tests]
enabled = true
severity = "error"

[gates.ignored-tests]
enabled = true
severity = "error"

[gates.deletion-rationale]
enabled = true
severity = "error"
paths = ["tests/**", "src/**", "migrations/**"]

[gates.time-estimates]
enabled = true
severity = "warning"

[gates.pii]
enabled = true
severity = "error"

[gates.agent-scratch]
enabled = true
severity = "error"

[gates.config-integrity]
enabled = true
severity = "error"

[gates.dependency-delta]
enabled = true
severity = "error"
deny_wildcards = true

[gates.command]
enabled = true
severity = "error"

[[gates.command.commands]]
name = "cargo-deny"
preset = "cargo-deny"
```

### Profile 3: AI Coding Agent Diff Sentinel

Designed specifically for automated agent workflows (Claude Code, Antigravity, Copilot, Cursor). Restricts agent drift, prevents deletion or weakening of test suites, blocks ghost/vacuous tests with assertion density requirements, rejects placeholder justifications (e.g. `todo`, `fix later`), and bans ephemeral agent scratch directories from entering git history.

```toml
schema_version = 1
fail_on_warnings = true
fail_on_overrides = false

[directives]
sources = ["pr-body"]
allow_hidden = false

[gates.agents-md]
enabled = true
severity = "error"

[gates.assertion-reduction]
enabled = true
severity = "error"

[gates.vacuous-tests]
enabled = true
severity = "error"
min_assertions_per_test = 1

[gates.ignored-tests]
enabled = true
severity = "error"

[gates.unsafe-safety-comment]
enabled = true
severity = "error"

[gates.deletion-rationale]
enabled = true
severity = "error"

[gates.time-estimates]
enabled = true
severity = "error"

[gates.pii]
enabled = true
severity = "error"

[gates.agent-scratch]
enabled = true
severity = "error"

[gates.config-integrity]
enabled = true
severity = "error"

[gates.golden-output]
enabled = true
severity = "error"

[gates.test-budget]
enabled = true
severity = "error"
ratchet = true
```

---

## Planned Gates

The following gates are registered with `available: false` in the gate registry. Attempting to enable or configure them exits non-zero (F5). Full roadmaps, dependencies, and go/no-go gates are documented in [ROADMAP.md](ROADMAP.md).

- `scope-confinement` (Suite: Agent Guard) — Changes stay inside authorized directory paths.
- `suppression-delta` (Suite: Agent Guard) — Tracks net increases in compiler/linter suppression attributes (`#[allow]`, `@ts-ignore`, `# noqa`).
- `provenance-tags` (Suite: Hygiene) — Published numeric claims must carry `(measured)`, `(target)`, or `(projected)`.
- `ci-integrity` (Suite: Integrity) — Detects workflow weakening (`continue-on-error`, dropped `needs`, unpinned actions).
- `test-floor` (Suite: Integrity) — Test count ratchet read directly from the base ref.
- `pr-checklist` (Suite: Hygiene) — Reconciles ticked PR checkboxes against actual diffs.
- `sanitizers` (Suite: Verification) — Memory and thread sanitizer presets with race canaries.
- `msrv` (Suite: Quality) — Verifies build against minimum supported Rust version.
- `miri` (Suite: Verification) — Undefined behavior verification under Miri with zero-test guards.
- `unsafe-budget` (Suite: Verification) — Ratchet limiting the total number of `unsafe` blocks.
