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
| Gate id | Suite | Status | Languages | Rule Description |
|---|---|---|---|---|
| [`agents-md`](#agents-md) | agent-guard | **shipped** | any | AGENTS.md exists; CLAUDE.md / GEMINI.md do not fork it |
| [`assertion-reduction`](#assertion-reduction) | agent-guard | **shipped** | Rust, Python, JS/TS, PHPT, Java, Go, PHP, C/C++, C#, Ruby | assertion count / strength must not drop in an existing test |
| [`vacuous-tests`](#vacuous-tests) | agent-guard | **shipped** | Rust, Python, JS/TS, PHPT, Java, Go, PHP, C/C++, C#, Ruby | new tests must carry a non-tautological assertion |
| [`ignored-tests`](#ignored-tests) | agent-guard | **shipped** | Rust, Python, JS/TS, PHPT, Java, Go, PHP, C/C++, C#, Ruby | tests must not be newly #[ignore]d or skipped without directive |
| [`unsafe-safety-comment`](#unsafe-safety-comment) | agent-guard | **shipped** | Rust | unsafe blocks / impls carry a // SAFETY: comment |
| [`deletion-rationale`](#deletion-rationale) | agent-guard | **shipped** | any | deleted files and removed tests need a scoped removes: rationale |
| [`time-estimates`](#time-estimates) | hygiene | **shipped** | any | no calendar / duration estimates in markdown or the PR body |
| [`pii`](#pii) | hygiene | **shipped** | any | no home paths, LAN IPs, or denylisted hostnames in tracked text |
| [`agent-scratch`](#agent-scratch) | hygiene | **shipped** | any | agent scratch state is never tracked |
| [`shell-secrets`](#shell-secrets) | hygiene | **shipped** | shell, docker, workflows | no command-line secrets or unverified piped scripts in shell, docker, or CI |
| [`issue-link`](#issue-link) | hygiene | **shipped** | any | PR title or description links a tracking issue (#123, Fixes #123) |
| [`config-integrity`](#config-integrity) | integrity | **shipped** | any | a change cannot weaken its own discipline.toml without a token |
| [`scope-confinement`](#scope-confinement) | agent-guard | **shipped** | any | changes stay inside authorized paths |
| [`suppression-delta`](#suppression-delta) | agent-guard | **shipped** | per pack | new #[allow], commented-out tests, cfg-gated tests |
| [`provenance-tags`](#provenance-tags) | hygiene | **shipped** | any | published numerics carry (measured|target|projected) |
| [`ci-integrity`](#ci-integrity) | integrity | **shipped** | any | workflow weakening: continue-on-error, || true, unpinned actions |
| [`test-floor`](#test-floor) | integrity | **shipped** | any | test-count ratchet read from the base ref |
| [`golden-output`](#golden-output) | integrity | **shipped** | any | prevents stealth edits to committed golden/test output files without explicit override |
| [`dependency-delta`](#dependency-delta) | integrity | **shipped** | any | manifest diff inspection: zero wildcards, source/license allowlists, and deny.toml verification |
| [`test-budget`](#test-budget) | integrity | **shipped** | Rust, Python, JS/TS, Go, any | property-test and fuzz effort ratchet (cases, shrink iters, fuzztime, seed corpus) |
| [`pr-checklist`](#pr-checklist) | hygiene | **shipped** | any | ticked PR checkboxes are reconciled against the diff |
| [`command`](#command) | verification | **shipped** | any | fail-closed wrapper for any tool: zero-tests guard, canary, count ratchet |
| [`sanitizers`](#sanitizers) | verification | **shipped** | Rust, C/C++ | ASan / TSan preset with audited suppressions and a race canary |
| [`msrv`](#msrv) | quality | **shipped** | Rust | cargo check under the pinned MSRV |
| [`miri`](#miri) | verification | **shipped** | Rust | Miri tiers with zero-tests guard |
| [`unsafe-budget`](#unsafe-budget) | verification | **shipped** | Rust | unsafe count ratchet |
| [`bench-regression`](#bench-regression) | bench | **shipped** | Rust, Go, Python, C/C++ | benchmark drift via harness adapters (deterministic counts or BCa intervals) |
| [`archive-contents`](#archive-contents) | integrity | **shipped** | any | distribution archive must contain required paths and zero forbidden developer artifacts |
| [`manifest-sync`](#manifest-sync) | integrity | **shipped** | any | reconcile git-tracked files against packaging manifest declarations |
| [`version-lockstep`](#version-lockstep) | integrity | **shipped** | any | version declarations across headers, manifests, and files must remain in lockstep |
<!-- /generated -->

### Directive Policy

Each gate gets exactly one canonical directive (with at most one documented deprecated spelling). Directives must be scoped to their natural subject (file path, test name, action ref, workflow job, dependency name, or rule identifier). Blanket waivers without subjects are rejected.

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

## Confidence & Stakes Severity Hierarchy

Discipline organizes violation severity into a 3-tier hierarchy based on detection confidence and operational stakes:

1. **`error` (Blocking, Exit 1):** High-confidence violations representing clear security compromises, assertion drops, or intentional test erosions. Fails CI by default.
2. **`warning` (Non-blocking, Exit 0 unless `--fail-on-warnings`):** Heuristic or prose scans where context matters, or performance/benchmark gates that may fluctuate across hardware. Visible in all reports; promoted to blocking with `--fail-on-warnings`.
3. **`note` (Informational, Exit 0):** Advisory annotations (e.g. conditional skips on target platforms, stale baseline entries, unanalysed languages). Mapped to `note` in SARIF, `info` in GitLab Code Quality, and GitHub Actions `notice` annotations.

### Default Severity by Gate

While most integrity and correctness gates default to `error`, gates with higher false-positive susceptibility on brownfield repositories default to `warning`:

| Gate | Default Severity | Rationale |
|---|---|---|
| `time-estimates` | `warning` | Prose sweeps can catch calendar references or historical notes in documentation. Measured at v0.4.2: 51 errors across `orieg/expanse` and 26 in `orieg/php-judy`, nearly all pre-existing documentation. Defaults to warning so new adopters can triage without blocking PRs. |
| `bench-regression` | `warning` | Hardware jitter, noisy CI environments, and varying runner CPUs can trigger spurious regressions on wall-clock benchmarks. Projects requiring strict performance gates can configure `severity = "error"` or run with `--fail-on-warnings`. |
| `agents-md` | `warning` | Forked or missing `AGENTS.md` guidance is an engineering hygiene issue rather than a broken build or code safety defect. |
| *All other gates* | `error` | Syntactic regressions, assertion drops, vacuous tests, safety decay, and secret leaks default to blocking errors. |

### Finding-Level Severity Overrides

Certain gates distinguish high-confidence rules from heuristic indicators within the same gate:

- **`shell-secrets`:**
  - **High-confidence token rules (`error`):** Structured secrets matching canonical token entropy or formats (`TOKEN-GHP` `ghp_`, `TOKEN-AWS` `AKIA...`, `TOKEN-SLACK` `xox[bap]-`, `TOKEN-OPENAI` `sk-...`, `TOKEN-ANTHROPIC`, `TOKEN-PRIVATE-KEY` PEM headers).
  - **Heuristic rules (`warning`):** Command-line arguments and pipe constructs (`ARGV-ENV`, `FLAG-PASSWD`, `INJECT-PIPE`).
- **`ignored-tests`:**
  - **Unconditional skips (`error`):** Tests newly disabled via `#[ignore]`, `@pytest.mark.skip`, `xit`, or `@Disabled` without justification.
  - **Conditional target skips (`note`):** Platform-predicated skips (`#[cfg_attr(windows, ignore)]`, `skipif(sys.platform == 'win32')`).
- **`pii` / Workstation Hygiene:**
  - Private RFC 1918 LAN IPs (`192.168.x.x`, `10.x.x.x`, `172.16.x.x`) default to warning/redaction, but can be exempted for local triage via `gates.pii.lan_ips = false`.

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
  - Commented-out test functions (covered by `suppression-delta`).
- **Lifting directive:** `allow-ignore: <test-name> <reason>`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `approved_predicates`.

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
  - Unscoped deletions when `require_scope = false` is configured (waives all deletions in the PR).
- **Lifting directive:** `removes: <path-or-test> <reason>`, `deletes: <path-or-test> <reason>`, or namespaced `discipline: removes: <path-or-test> <reason>`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `paths`, `require_scope`, `allow_hidden`.

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

#### `scope-confinement`
- **Rule:** Agent modifications must remain strictly within configured authorized directory and file paths (`allowed_paths`) and never touch restricted paths (`forbidden_paths`).
- **Languages:** Any.
- **What it catches:**
  - Modifications touching paths outside `allowed_paths`.
  - Modifications touching paths matching `forbidden_paths` (e.g. security credentials, CI workflow definitions, release scripts).
- **Passing commit / PR body (accepted):**
  ```text
  allow-scope: authorized infra migration across deploy scripts
  ```
- **What it does NOT catch:**
  - Files exempted via `exempt_paths`.
  - Modifications when `allowed_paths` is empty and no `forbidden_paths` are matched.
- **Lifting directive:** `allow-scope: <reason>`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `allowed_paths`, `forbidden_paths`.

#### `suppression-delta`
- **Rule:** Rejects net increases in compiler, linter, or type checker suppression annotations (`#[allow]`, `#[expect]`, `@ts-ignore`, `@ts-expect-error`, `/* eslint-disable */`, `# noqa`, `// nolint`, `#pragma warning disable`) across tracked source files unless explicitly authorized.
- **Languages:** Rust, Python, TypeScript, JavaScript, Go, C/C++, C#.
- **What it catches:**
  - Newly added suppression annotations that silence linter or compiler warnings.
  - Commented-out test functions and unverified `#[cfg]` gates.
- **Failing diff (rejected):**
  ```rust
  + #[allow(dead_code, clippy::all)]
    fn internal_helper() { ... }
  ```
- **Passing commit / PR body (accepted):**
  ```text
  allow-suppression: unavoidable legacy FFI bindings in wrapper module
  ```
- **What it does NOT catch:**
  - Pre-existing suppression annotations present on the base ref.
  - Suppressions inside explicitly exempted file paths.
- **Lifting directive:** `allow-suppression: <reason>`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `patterns`.

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
  - Operational TTLs, cache expiration, retention, and timeouts (`timeout: 30s`, `retention: 7 days`). <!-- discipline:allow(time-estimates) -->
  - Terms of art: metric names ("one-minute load average", "1-min average", "`load1`"), derived operational wrap windows ("~6.06 days active window", wrap window, bitfield, epoch).
  - Benchmark measurements ("ran in 4.2 seconds").
  - Historical durations and narration ("was maintained for three years", "forty minutes later — a commit ordering", "shipped a day ago"). <!-- discipline:allow(time-estimates) -->
  - Code inside fenced blocks (` ``` `).
- **Lifting directive:** In markdown: `<!-- discipline:allow(time-estimates) -->` or inline marker `docs-lint: allow` on the matching line.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `include`, `extra_patterns`, `allow_patterns`, `scan_pr_body`, `diff_only`.

#### `pii`
- **Rule:** No leaked developer workstation home directories, private RFC 1918 LAN IPs, references to personal agent configurations, or denylisted hostnames in tracked text files or the PR body.
- **Languages:** Any.
- **What it catches:**
  - Local home directory paths: `/Users/<username>/...`, `/home/<username>/...`, `C:\Users\<username>\...`.
  - Private IPv4 LAN addresses: `10.x.x.x`, `172.16-31.x.x`, `192.168.x.x`.
  - Whole-token matches of denylisted internal hostnames.
  - References to personal maintainer agent configuration (`~/.claude`, `$HOME/.gemini`, `RESEARCH_DISCIPLINES.md`, `*_PLAYBOOK.md`) across tracked text files. <!-- discipline:allow(pii) -->
  - Leaks inside decoded JSON keys and string literals, including escaped slashes (`\/`).
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
  *(Note: Test code is explicitly scanned because test fixtures are where paths and IPs frequently leak. Self-referential fixtures must use runtime assembly, inline `discipline:allow(pii)`, or `exempt_paths`).*
- **Lifting directive:** `<!-- discipline:allow(pii) -->` or `docs-lint: allow` on the matching line.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `home_paths`, `lan_ips`, `secrets`, `agent_config_refs`, `allowed_users`, `hostname_denylist`, `extra_patterns`, `allow_patterns`, `scan_pr_body`, `diff_only`.

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
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `paths`.

#### `shell-secrets`
- **Rule:** No command-line argument secrets or unverified piped script execution in shell scripts, Dockerfiles, or CI workflow files.
- **Languages:** Shell (`*.sh`, `*.bash`, `*.zsh`), Dockerfiles, CI workflows (`.github/workflows/`, `.gitea/workflows/`, `.forgejo/workflows/`, `.gitlab-ci.yml`).
- **What it catches:**
  - `ARGV-ENV`: Passing secrets via command-line arguments to `env` (e.g. `env TOKEN=$MY_TOKEN ./script.sh`).
  - `ARGV-DOCKER`: Passing secret arguments via `docker run -e TOKEN=$SECRET` or `-e TOKEN="secret"`.
  - `ARGV-INLINE`: Expanding secret variables inside inline script strings `sh -c "... $SECRET ..."`.
  - `INJECT-XARGS`: Metacharacter command injection via `xargs -I {} sh -c '... {} ...'`.
  - `INJECT-PIPE`: Piping remote network downloads directly into shell interpreters `curl ... | bash`.
- **Installer & `INJECT-PIPE` Rationale:**
  - `INJECT-PIPE` flags piping remote network streams directly into shell interpreters (`curl ... | sh` / `curl ... | bash`) because uninspected piped execution is vulnerable to network truncation, connection drops leading to partial execution, and unverified execution.
  - Checksum-verifying installers that inspect payloads internally: when an installer script internally fetches release assets, verifies their cryptographic SHA-256 checksum against `SHA256SUMS`, and only unpacks or executes upon hash verification, the downloaded binary payload is verified.
  - However, download-verify-run (`curl -fsSL -o install.sh ... && bash install.sh`) remains the primary recommended pattern so operators can inspect the script before execution and avoid partial execution on interrupted connections.
- **Finding Severity Breakdown:**
  - `error`: High-entropy/structured secret tokens (`TOKEN-GHP` `ghp_`, `TOKEN-AWS` `AKIA...`, `TOKEN-SLACK` `xox[bap]-`, `TOKEN-OPENAI` `sk-...`, `TOKEN-ANTHROPIC`, `TOKEN-PRIVATE-KEY` PEM blocks).
  - `warning`: Heuristic argument and piping patterns (`ARGV-ENV`, `ARGV-DOCKER`, `ARGV-INLINE`, `FLAG-PASSWD`, `INJECT-PIPE`, `INJECT-XARGS`).
- **Lifting directive:** `secrets-argv-ok: <file-or-line> <reason>` in PR body or commit, or inline `discipline:allow(shell-secrets)`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `extra_secret_patterns`, `allow_patterns`, `diff_only`.

#### `issue-link`
- **Rule:** Every pull request title or description must reference a tracking issue (`#123`, `Fixes #123`, `Closes #123`), or carry an explicit `no-issue:` rationale.
- **Default:** `enabled = false` (opt-in).
- **False-Positive Rationale:** Field measurements on `orieg/expanse` and public open-source repositories demonstrate that `issue-link` produces disproportionate friction on routine maintenance PRs — documentation improvements, small chore PRs, dependency updates, and internal refactors — where formal tracking issues are neither required nor created. Repositories requiring tracking issues on all PRs can opt in via `[gates.issue-link] enabled = true`.
- **Languages:** Any.
- **What it catches:**
  - PRs with no referenced issue in the PR title or PR description.
  - Placeholder waiver values like `no-issue: <reason>` or empty waivers.
- **Lifting directive:** `no-issue: <reason>` on its own line in the PR description.
- **Config keys:** `enabled`, `severity`, `pattern`.

#### `provenance-tags`
- **Rule:** Published numeric claims, tables, mechanism assertions, wall-clock intervals, and paired comparisons in markdown files and PR bodies must carry truthful provenance tags, hardware counter evidence, confidence intervals, or explicit hypothesis/differentiation qualifiers.
- **Default:** `enabled = false` (opt-in).
- **False-Positive Rationale:** Field measurements on `orieg/expanse` and public open-source repositories indicate that `provenance-tags` produces excessive noise on tabular benchmark comparisons, descriptive configuration tables, and architectural diagrams that are illustrative or descriptive rather than novel-claim-bearing. Repositories publishing empirical research benchmarks and requiring strict provenance tagging can opt in via `[gates.provenance-tags] enabled = true`.
- **Languages:** Markdown (`*.md`) and PR description.
- **What it catches:**
  - Markdown tables containing unit-bearing numbers (`ns`, `µs`, `ms`, `ops/s`, `Mops/s`, `B/key`, etc.) without a provenance tag (`(measured: host, commit)`, `(target)`, or `(projected)`).
  - Unverified mechanism claims (`memory-latency-bound`, `branch-misprediction`, `TLB-bound`, etc.) without citing hardware counters (`perf stat`, `cycle_activity`, etc.) or marking as `hypothesis` / `unmeasured`.
  - Wall-clock ratios (`2.9x faster`, `3.1x speedup`) without confidence intervals (`[lo, hi]`, `BCa`, `CI`) or provisional markers.
  - Paired comparison figures (`11.9 ns vs 108.9 ns`, cross-metric comparisons) without shared workload tags (`(workload: id)`) or documented differentiation markers.
- **Passing examples (accepted):**
  - Table caption carrying `*(measured: host, commit)*` or `*(target)*`.
  - Mechanism claim citing `perf stat` counters or labeled as `(hypothesis — unmeasured pending PMU counters)`.
  - Wall-clock speedup citing `[2.7x, 3.1x] BCa 95% CI` or `(provisional pending re-measurement)`.
  - Paired comparison citing `(workload: uniform-random)`.
- **Lifting directive:** `allow-provenance: <file-or-path> <reason>` in PR body or commit, or inline `<!-- discipline:allow(provenance-tags) -->`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `include`, `check_tables`, `check_mechanisms`, `check_intervals`, `check_paired_figures`, `scan_pr_body`.

#### `pr-checklist`
- **Rule:** Reconciles ticked checklist items in PR descriptions (`- [x] Tests added/updated`, `- [x] Documentation updated`, `- [x] Benchmarks added`) against actual modified files in the pull request diff to prevent vacuous checkoffs.
- **Languages:** Any.
- **What it catches:**
  - Ticked test checkboxes when zero test files were modified.
  - Ticked documentation checkboxes when zero docs or markdown files were modified.
  - Ticked benchmark checkboxes when zero benchmark files were modified.
- **Passing PR body (accepted):**
  Checklists accurately reflect modified files, or unticked items remain `- [ ]`.
- **Lifting directive:** `allow-pr-checklist: <reason>`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `require_tests`, `require_docs`, `require_benches`.

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
  - Workflow-level switches (`disable:` in GitHub Actions steps) — protected by `ci-integrity`.
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
- **Lifting directive:** `allow-test-shrink: <target-or-metric> <reason>`.
#### `ci-integrity`
- **Rule:** CI/CD workflow integrity and rollup sentinel. Enforces complete rollup jobs (`ci-gate` must `needs:` all verification jobs), pins third-party actions by 40-character commit SHA, bans masked failures (`continue-on-error: true`), and bans exit-code suppression (`|| true`, `set +e`).
- **Languages:** CI workflow files (`.github/workflows/*.yml`, `.github/workflows/*.yaml`).
- **What it catches:**
  - Rollup job missing a dependency on verification jobs defined in the workflow (`Incomplete Rollup Job Needs`).
  - Third-party GitHub actions unpinned or pinned to mutable tags/branches (`@v4`, `@main`) instead of 40-character commit SHA.
  - Steps carrying `continue-on-error: true`.
  - Commands masking exit codes (`|| true`, `set +e`).
  - Documented job count mismatches when `documented_job_count_path` is configured.
- **Passing commit / PR description (accepted):**
  ```text
  allow-ci-weakening: ci-gate temporary rollup relaxation during migration
  ```
- **What it does NOT catch:**
  - Local actions (`./...`) and docker actions (`docker://...`).
  - Workflows matching `excluded_jobs` (e.g. `detect-changes`).
- **Lifting directive:** `allow-ci-weakening: <subject> <reason>`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `workflows`, `rollup_job`, `excluded_jobs`, `pin_actions`, `forbid_continue_on_error`, `forbid_or_true`, `diff_only`, `documented_job_count_path`, `documented_job_count_pattern`.

#### `test-floor`
- **Rule:** Universal test count ratchet and floor sentinel. Operates in zero-config mode by default to prevent any drop in workspace AST test count across all supported languages relative to the base ref (with configurable `tolerance = 0`). When explicit floors are configured, reads test count floor constants and `min_tests` from the base ref (preventing PRs from silently lowering their own floor), enforces configured test count minimums, and ensures required test suite files exist. Complete test file deletions are detected and blocked.
- **Languages:** Any supported language pack (Rust, Python, JS/TS, PHPT, Java, Go, PHP, C/C++, C#, Ruby) or external test listing command.
- **What it catches:**
  - Workspace test count dropping below merge base ref count in zero-config mode (with `tolerance = 0` default).
  - Workspace test count dropping below configured `min_tests` or base floor constant.
  - Complete deletion of test files causing total test count reduction.
  - Lowering of floor constant value in `constant_file` below merge base ref.
  - Lowering or removal of `min_tests` in `discipline.toml` below merge base ref.
  - Missing `required_suites` files.
  - Missing floor constant file on base ref (fails closed).
- **Passing commit / PR description (accepted):**
  ```text
  allow-test-shrink: TEST_FLOOR test suite pruned for modularization
  ```
  or
  ```text
  allow-gate-weakening: test-floor test suite restructured for modularization
  ```
- **What it does NOT catch:**
  - Test count increases (ratchet permits additions).
  - Reductions within configured `tolerance`.
  - Reductions excused with `allow-test-shrink: <subject> <reason>` or `allow-gate-weakening: test-floor <reason>`.
- **Lifting directive:** `allow-test-shrink: <subject> <reason>` or `allow-gate-weakening: test-floor <reason>`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `min_tests`, `tolerance`, `constant_file`, `constant_name`, `required_suites`, `test_command`.

#### `archive-contents`
- **Rule:** Distribution archives (`.tar.gz`, `.tgz`, `.zip`, `.crate`, `.tar.bz2`, `.tar`) produced during packaging or release must contain all required files and zero forbidden developer artifacts, private files, or CI scripts.
- **Languages:** Any.
- **What it catches:**
  - Missing distribution archives when required (fails closed with exit 2).
  - Ambiguous archive glob patterns matching multiple candidate archives (fails closed with exit 2).
  - Missing `required_paths` in the archive (e.g. `config.m4`, `php_judy.h`, `LICENSE`, `README.md`).
  - Forbidden entries matching `forbidden_patterns` regexes (e.g. `.git*`, `tools/**`, `tests/**`, private keys, local dev artifacts).
  - Supports `strip_components = 1` for archives rooted in a versioned directory (e.g. `Judy-2.6.0/config.m4`).
- **Failing archive example (rejected):**
  Archive containing `Judy-2.6.0/tools/check.sh` when `forbidden_patterns = ["^tools/"]`.
- **Passing PR description (accepted):**
  ```text
  allow-archive-leak: ^tools/ temporary packaging tool bundled for triage
  ```
- **What it does NOT catch:**
  - Files not packaged into the archive.
  - Dynamically generated files inside archives matching non-standard extensions outside supported formats.
- **Lifting directive:** `allow-archive-leak: <pattern> <reason>` in PR description or commit message.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `archive_path`, `required_paths`, `forbidden_patterns`, `strip_components`.

#### `manifest-sync`
- **Rule:** Reconciles git-tracked files in declared directories against file lists in packaging manifests (e.g., PECL `package.xml`, Ruby `gemspec`, Python `MANIFEST.in`, Debian `debian/install`, etc.). Bidirectional diffing detects both unmanifested git files (`+`) and ghost manifest entries (`-`).
- **Languages:** Any.
- **What it catches:**
  - Missing manifest files (fails closed with exit 2).
  - Unparseable manifest extraction regex (fails closed with exit 2).
  - Zero manifest entries found when watched paths contain tracked files (fail-closed integrity guard).
  - Unmanifested files (`+`): git-tracked files matching `watched_paths` (excluding `exclude_paths`) not declared in the manifest.
  - Ghost manifest entries (`-`): files declared in the manifest that do not exist in the working directory.
- **Failing diff example (rejected):**
  Adding a new source file to git repository without declaring it in `package.xml`.
- **Passing PR description (accepted):**
  ```text
  allow-manifest-drift: package.xml intentionally deferred manifest update during refactor
  ```
- **What it does NOT catch:**
  - Untracked or gitignored files in the workspace.
  - Files outside declared `watched_paths`.
- **Lifting directive:** `allow-manifest-drift: <manifest-path> <reason>` in PR description or commit message.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `rules` (`manifest`, `extract_regex`, `watched_paths`, `exclude_paths`).

#### `version-lockstep`
- **Rule:** Version declarations across multiple files (C header `#define`, packaging manifest `<release><version>`, `Cargo.toml`, `pyproject.toml`, documentation, etc.) must remain in lockstep.
- **Languages:** Any.
- **What it catches:**
  - Missing source files (fails closed with exit 2).
  - Unparseable source regexes or regexes failing to match the source file (fails closed with exit 2).
  - Mismatched extracted versions across declared files in a group (e.g., `php_judy.h` has `"2.6.0"` while `package.xml` has `"2.6.1"`).
- **Failing diff example (rejected):**
  Bumping version in `package.xml` to `2.6.1` while `#define PHP_JUDY_VERSION` in `php_judy.h` remains `2.6.0`.
- **Passing PR description (accepted):**
  ```text
  allow-version-mismatch: php-judy-release staged release version bump across branches
  ```
- **What it does NOT catch:**
  - Unconfigured files or uncaptured version substrings.
  - Version increments in unversioned changelogs without regex capture groups.
- **Lifting directive:** `allow-version-mismatch: <group-name> <reason>` in PR description or commit message.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `groups` (`name`, `sources` (`path`, `regex`)).

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

#### `sanitizers`
- **Rule:** Executes runtime sanitizers (AddressSanitizer `ASan` or ThreadSanitizer `TSan`) with negative-control race canaries and audited suppression list verification.
- **Languages:** Rust, C/C++.
- **What it catches:**
  - Memory errors (out-of-bounds access, use-after-free) or data races detected by LLVM sanitizers.
  - Failure of negative-control canaries to trigger expected sanitizer diagnostics.
  - Unaudited sanitizer suppression entries.
- **Lifting directive:** `allow-sanitizers: <reason>`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `sanitizer`, `timeout_seconds`, `canary`.

#### `miri`
- **Rule:** Executes Miri (`cargo miri test`) with a zero-tests guard to detect undefined behavior (UB), invalid memory operations, and memory leaks.
- **Languages:** Rust.
- **What it catches:**
  - Undefined behavior flagged during Miri execution.
  - Zero tests executing under Miri when test filters match zero cases (prevents vacuous passes).
- **Lifting directive:** `allow-miri: <reason>`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `args`, `timeout_seconds`, `allow_zero`.

#### `unsafe-budget`
- **Rule:** Enforces an `unsafe` block count ratchet: the total number of `unsafe` blocks and functions cannot increase without an explicit justification directive.
- **Languages:** Rust.
- **What it catches:**
  - Net additions of `unsafe` blocks or `unsafe fn` declarations across tracked source files.
- **Lifting directive:** `allow-unsafe: <reason>`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`.

---

### Pillar 5: Quality & Compiler Toolchain (`quality`)

#### `msrv`
- **Rule:** Validates that the repository declares a Minimum Supported Rust Version (`rust-version` in `Cargo.toml` or `pinned_version`) and compiles cleanly under that toolchain.
- **Languages:** Rust.
- **What it catches:**
  - Missing `rust-version` declaration in `Cargo.toml`.
  - Compilation or syntax errors when building under the declared or pinned MSRV toolchain.
- **Lifting directive:** `allow-msrv: <reason>`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `pinned_version`.

---

### Pillar 6: Benchmark Drift (`bench`)

#### `bench-regression`
- **Rule:** Benchmark output files are tracked across revisions. Comparisons against merge-base baselines enforce formal mathematical confidence intervals and exact deterministic instruction counts.
- **Languages:** Rust, C/C++, Go, Python, PHP.
- **What it catches:**
  - Regressions where conservative confidence intervals clear the tolerance threshold ($\Delta_{min} = \frac{L_{head} - U_{base}}{U_{base}} > \text{tolerance}$).
  - Exact instruction count regressions in Callgrind / IAI outputs (`events: Ir`).
  - Regressions in generic JSON sample arrays (`{"benchmarks": { "<name>": { "runs_ms": [...], "median_ms": ... } }}`) using deterministic bootstrap 95% confidence intervals ($B=2000$).
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

## Legacy Script Parity & Replacement Reference

Discipline provides universal static binary drop-in replacements for the legacy verification scripts in high-assurance repositories (such as `orieg/expanse`):

| Gate | Replaced Legacy Script | Discipline Enhancements & Behavioral Differences |
|---|---|---|
| `assertion-reduction` | `scripts/check_diff_guards.py` | Multi-language AST extraction (11 language packs), callback-aware function tracking, compile-time assertions (`static_assert`, `const _: () = assert!`). |
| `vacuous-tests` | `scripts/check_diff_guards.py` | Language-specific AST helper detection (Python non-test methods, C/C++ non-zero return / throw helper recognition). |
| `ignored-tests` | `scripts/check_diff_guards.py` | Distinguishes newly arriving ignored tests from modified tests, configurable approved skip predicates (`cfg_attr(miri, ignore)`). |
| `deletion-rationale` | `scripts/check_diff_guards.py` | Line-anchored directive parsing, configurable `require_scope` and `allow_hidden` directive controls. |
| `time-estimates` | `scripts/check_docs_hygiene.py` | Paragraph and sentence-level boundary lookarounds avoiding `\b` false positives on symbols (`×`, `~`), diff-scoped mode (`diff_only = true`), operational term-of-art and wrap window exemptions, `docs-lint: allow` alias. |
| `pii` | `scripts/check_docs_hygiene.py` | Full test code inspection without blind spots, JSON string unescaping, cross-tree agent config directory/playbook detection, secret-backed hostname denylist. |
| `test-floor` | `scripts/check_test_floors.py` | Automatic base-ref constant extraction, direct `test_command` execution, fail-closed handling on unresolvable base floors, `allow-test-shrink:` override. |
| `ci-integrity` | `scripts/check_ci_gate.py`, `scripts/check_gate_floor.py`, `scripts/check_ci_filters.py` | Complete rollup job `needs:` closure validation, 40-character commit SHA pinning, masked failure detection (`continue-on-error`, `\|\| true`, `set +e`), `allow-ci-weakening:` override. |
| `bench-regression` | `scripts/perf_report.py`, `scripts/wasm_fuel.py` | In-job dual-file mode (`--bench-base-file` and `--bench-head-file`), `iai-callgrind` console line and neutral JSON parsers, two-tier threshold (single-worst >5% or $\ge 2$ arms regressing >0.5% noise floor, advisory 0.1%), declared arm exemptions, sourced overrides verifying CI URL or committed artifact and named arms, missing-baseline fatal fail-closed. |
| `provenance-tags` | `scripts/check_docs_hygiene.py` | Table numeric provenance (`(measured: host, commit)`, `(target)`, `(projected)`), mechanism claim hardware counter citations, wall-clock intervals, paired comparison tags (`(workload: id)`). |
| `command` | Bespoke shell runner wrappers | Universal fail-closed timeout wrapper, zero-tests guards, turnkey presets (`cargo-public-api`, `miri`, `sanitizers`, `loom`, `cargo-deny`, `cargo-mutants`). |

---

## Repository Security Boundary & Workspace Ownership

Discipline inspects git history and diffs using `libgit2`. Access to the underlying git repository enforces strict security boundaries that differ between host workstations and containerized environments:

### Host Binary Enforcement (CVE-2022-24765 Protection)
On developer workstations and multi-tenant hosts, the native `discipline` binary enforces strict repository owner validation by default.
- **Threat Model (CVE-2022-24765):** In multi-user systems, an attacker can create a malicious `.git` directory in a shared location (such as `/tmp` or a shared parent directory) with poisoned configuration, hooks, or executable aliases. If a tool traverses into that directory without checking ownership, it could execute arbitrary code under the invoking user's credentials.
- **Fail-Closed Guard:** If the repository at the discovered path is not owned by the current user, `discipline` fails closed with code `code=Owner (-36)` and prints an actionable diagnostic naming the cause and resolution paths.
- **Opt-In Override:** In automated or specialized host environments where cross-user repository access is intentional and audited, owner validation can be disabled via the `--trust-workspace` CLI flag or by passing `DISCIPLINE_TRUST_WORKSPACE=1`.

### Container Image Relaxation
The official container image (`ghcr.io/orieg/discipline`) intentionally relaxes repository owner validation out-of-the-box (via system-wide `safe.directory '*'`, an entrypoint wrapper registering the active directory, and `ENV DISCIPLINE_TRUST_WORKSPACE=1`).
- **Operational Reality:** In containerized CI/CD runners (Docker volume mounts, Gitea Act Runner, Forgejo Runner, GitLab CI `/builds`, Kubernetes/Argo `/workspace`), checkout volumes are frequently owned by root (`0:0`) or the host runner UID, while the container executes as unprivileged `USER 10001:10001`. Requiring manual `--user` overrides or volume-mounted git configs adds significant friction and causes false-positive failures on normal setups.
- **Security Assessment:** Relaxing owner validation within the official container image is safe because:
  1. **Ephemeral Single-Purpose Sandbox:** The container executes inside an isolated container namespace with a dedicated filesystem and unprivileged user credentials (`USER 10001:10001`).
  2. **No Hook or Pager Execution:** `discipline` and `libgit2` do not invoke external git hooks, custom diff filters, or pager binaries, which eliminated the execution vector exploited in CVE-2022-24765.
  3. **No Root Escalation:** The container lacks `setuid` binaries or root escalation capabilities.

---

## Roadmap & Future Gates

All 30 foundational gates across the six suites are fully implemented and shipped in Discipline v0.6.0+. Future candidate research gates under evaluation (including paired within-run ratio benchmarking, mutation score floor, and fuzz corpus drift) are documented in [ROADMAP.md](ROADMAP.md).

