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
| [`ci-skip-set`](#ci-skip-set) | integrity | **shipped** | any | rollup skip set matches each job's `if:` under the observed filter outputs |
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
| **Python** | pytest / unittest collection rules: `test*` functions, `test*` methods of `Test*` classes and `TestCase` subclasses (`self_test()` is not a test) | `assert` statements, `self.assert*`, `pytest.raises`, `pytest.approx`; strong: `==`, `assertEqual` family | `@pytest.mark.skip` / `skipif` / `xfail`, `@unittest.skip*` | `# type: ignore`, `# noqa`, `# pragma: no cover` | **shipped** |
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

Default enablement and default severity are part of the compatibility contract (`docs/ARCHITECTURE.md` §3.1): within a major version a default may only become stricter unless the change is recorded in the [Default Changes ledger](ROADMAP.md#default-changes-compatibility-ledger). `tests/test_config.rs::default_enablement_and_severity_match_snapshot` pins every row below, so a default cannot change without a deliberate edit that shows up in review.

A default is chosen from two inputs: **detection confidence** (how often a finding on an arbitrary repository is a real defect) and the **cost of being wrong** (a false block stops an unrelated merge; a missed finding lets erosion through). A gate whose finding population in an unknown repository is high because the flagged construct is also a routine, reviewed practice defaults to `warning`: it stays visible in every report and blocks under `--fail-on-warnings` or an explicit `severity = "error"`.

**Default-on gates:**

| Gate | Default | Detection confidence | Cost of being wrong | Rationale |
|---|---|---|---|---|
| `assertion-reduction` | on, `error` | High: AST count and strength comparison of the same test on base and head. | False block: a legitimate test refactor needs a scoped `allow-assertion-drop:` directive. Miss: silent loss of coverage. | The construct flagged (a test losing assertions) is rare in reviewed changes and is the core erosion this tool exists to stop. |
| `vacuous-tests` | on, `error` | High: AST detection of a new test with no non-tautological assertion. | False block: an assertion helper not yet listed in `assert_helper_fns`. Miss: a test that can never fail. | Only new tests are judged, so the population is bounded by the change itself. |
| `ignored-tests` | on, `error` (conditional skips: `note`) | High: AST skip markers on tests added or changed in the diff. | False block: an intentional skip needs `allow-ignore:`. Miss: a disabled test counted as passing. | Platform-predicated skips are already downgraded to `note`; unconditional skips are rare and deliberate. |
| `unsafe-safety-comment` | on, `error` | High: AST `unsafe` block or impl without a preceding `// SAFETY:` comment. | False block: a missing comment on a sound block, fixed by writing it. Miss: an undocumented soundness invariant. | Only diff-touched unsafe sites are checked; writing the comment is the fix. |
| `deletion-rationale` | on, `error` | High: git records the deletion exactly. | False block: a planned removal needs one `removes:` line. Miss: a stealth deletion of tests or benchmarks. | Deletions are infrequent and the fix is a single scoped line. |
| `pii` | on, `error` | High for home paths, LAN IPs and denylisted hosts; `diff_only = false`, so the whole tracked tree is swept. | False block: a pre-existing documentation path. Miss: a leaked workstation path or internal host in a public repository. | A leak is not reversible once published. Brownfield adopters use `diff_only = true`, `allowed_users`, or the grandfathering baseline. |
| `agent-scratch` | on, `error` | High: a tracked path matching agent state directories. | False block: a deliberately committed directory of the same name, exempted by path. Miss: private agent state in history. | The path set is narrow and the committed state is not reversible once pushed. |
| `shell-secrets` | on, `error` (token rules) / `warning` (heuristic rules) | High for structured tokens; heuristic for argv and pipe patterns. | False block: a token-shaped test fixture, exempted by path. Miss: a live credential in history. | The split already downgrades the heuristic rules at finding level. |
| `config-integrity` | on, `error` | High: base and head configuration are diffed structurally. | False block: an intended loosening needs `allow-gate-weakening:`. Miss: a change lowering its own bar (F9). | The gate protects every other gate; it cannot be advisory. |
| `ci-integrity` | on, `error` | High for `continue-on-error`, `\|\| true` and unpinned actions in modified workflows (`diff_only = true`). | False block: an intended pattern needs `allow-ci-weakening:`. Miss: a rollup that reports green while a job is skipped. | Only modified workflows are scanned by default, so pre-existing patterns do not block adoption. |
| `ci-skip-set` | on, `error` | High: each `needs` result is compared to its job's `if:` evaluated over the observed filter outputs; an unmodelled term is a finding, not a guess. Inert (a named "not evaluated" note) unless the rollup job supplies `DISCIPLINE_CI_CONTEXT`. | False block: a rollup whose workflow uses an `if:` form outside the modelled subset. Miss: a skip set the evaluator cannot distinguish from a legitimate one (all filters false on a change that touches no filtered path), reported as a note. | Supplying the context is the opt-in, so enabling it by default costs an ordinary diff check nothing. |
| `test-floor` | on, `error` | High: the base-ref test count is the floor unless one is configured. | False block: a test consolidation needs `allow-test-shrink:`. Miss: silent test-count erosion. | The ratchet is relative to the base ref, so it never fails a repository for its existing state. |
| `golden-output` | on, `error` | High: an edit to a committed golden or snapshot file. | False block: a legitimate snapshot update needs `allow-golden-update:`. Miss: an expectation rewritten to match a regression. | Snapshot edits are exactly the change a reviewer must see called out. |
| `dependency-delta` | on, `error` | High: manifest diffs are parsed; only changed dependencies are judged. | False block: a new dependency outside the allow-list needs `allow-dependency:` or an allow-list entry. Miss: a wildcard or unpinned git dependency. | The judged population is the dependencies the change adds. |
| `test-budget` | on, `error` | High: property-test and fuzz budgets compared base to head. | False block: an intended budget cut needs `allow-test-shrink:`. Miss: a fuzz or proptest budget quietly reduced. | Budgets are rarely edited and a cut is a deliberate decision. |
| `command` | on, `error` | Inert until a `command`, `preset`, or `DISCIPLINE_COMMAND` is configured; then the configured tool's own verdict. | False block: only a failure of the tool the repository chose to run. Miss: none while inert. | Enabling it by default costs nothing; configuring it is the opt-in. |
| `suppression-delta` | on, **`warning`** | Syntactically high, semantically low: `#[allow(...)]` is the reviewed escape hatch from `clippy -D warnings`, and `# noqa` / `// nolint` are routine. | False block at `error`: 78 findings (measured) across one consumer's last 100 merged pull requests, nearly all reviewed and intended. Miss at `warning`: none; the finding is still reported. | The flagged construct is a normal reviewed practice, so blocking by default fails ordinary changes. Repositories that treat every new suppression as a defect set `severity = "error"` (this repository does). |
| `time-estimates` | on, `warning` | Heuristic prose match over the whole tree (`diff_only = false`). | False block at `error`: at v0.4.2, 51 findings (measured) in one consumer repository and 26 (measured) in another, nearly all pre-existing documentation. | Prose context decides whether a duration is an estimate. |
| `bench-regression` | on, `warning` | Depends on the adapter: deterministic counts are high, wall-clock intervals are hardware-sensitive. | False block: runner jitter on wall-clock benchmarks. Miss: a real regression, still reported. | Projects with deterministic counters set `severity = "error"`. |
| `agents-md` | on, `warning` | High, but the finding is documentation hygiene, not a code defect. | False block: a repository without an agent guide fails every change. | Missing or forked guidance does not make a change unsafe. |

**Default-off gates** (`issue-link`, `provenance-tags`, `pr-checklist`, `scope-confinement`, `archive-contents`, `manifest-sync`, `version-lockstep`, `sanitizers`, `miri`, `unsafe-budget`, `msrv`) need repository-specific input (a tracker convention, archive path, manifest rules, version sources, toolchain) or encode a policy most repositories do not hold. They default to `error` so that enabling one is a single `enabled = true` line that blocks.

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
  - **Python**: Tests follow pytest and unittest collection with default settings: module-level functions named `test_*` (any `test*` in a test path), and `test*` methods of a class named `Test*` or deriving from a `TestCase` (followed through same-file bases, cycle-guarded). A same-file mixin a test class inherits keeps its `test*` methods; in a test path a mixin's methods are kept even when the subclass is in another file. No other name is special: `self_test()` is a script entry point, not a test. Direct same-file helper calls (`helper(...)`, and `self.helper(...)` within the class) are resolved 1 level deep; a helper's `assert`, `self.assert*` and `raise` statements count, the `raise` being the helper's failure path. Nested `def` / `lambda` bodies inside a helper are not counted. *Known limit*: A helper's own callees, and helpers imported from another file, are not followed; configure `assert_helper_fns` for those. `python_files` / `python_functions` overrides in pytest configuration are not read.
  - **C#**: Recognizes standard xUnit (`[Fact]`, `[Theory]`), NUnit (`[Test]`, `[TestCase]`, `[TestCaseSource]`), MSTest (`[TestMethod]`, `[DataTestMethod]`) attributes, qualified or with the `Attribute` suffix, and resolves 1-level same-file helpers. A method without one of these attributes is never a test, whatever its name. *Known limit*: Multi-targeting `#if` preprocessor branches inside expressions are gracefully downgraded to `Warning` severity with line numbers.
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
- **Default:** on, severity `warning` (see [Default Severity by Gate](#default-severity-by-gate)).
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `max_increase`, `allowed_suppressions`.

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
  - An estimate split across a line break (`ship in 3` / `weeks`): built-in and `extra_patterns` matches are found within a single line, then scoped to the clause that contains them. <!-- discipline:allow(time-estimates) -->
- **`allow_patterns` scope:** each pattern is matched against every line and against every paragraph with its soft-wrapped lines joined by one space (a blank line or a code fence ends the paragraph), so a phrase that wraps, such as `one-minute load average` split after `load`, is still matched. The exemption covers only the text the pattern matched: an estimate elsewhere on the same line or in the same paragraph still fires. `^` and `$` keep their per-line meaning. To exempt a whole line, write the pattern to match the whole line (`^Status:.*`).
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
- **False-Positive Rationale:** Field measurements on consumer repositories and public open-source projects demonstrate that `issue-link` produces disproportionate friction on routine maintenance PRs — documentation improvements, small chore PRs, dependency updates, and internal refactors — where formal tracking issues are neither required nor created. Repositories requiring tracking issues on all PRs can opt in via `[gates.issue-link] enabled = true`.
- **Languages:** Any.
- **What it catches:**
  - PRs with no referenced issue in the PR title or PR description.
  - Placeholder waiver values like `no-issue: <reason>` or empty waivers.
- **Lifting directive:** `no-issue: <reason>` on its own line in the PR description.
- **Config keys:** `enabled`, `severity`, `pattern`.

#### `provenance-tags`
- **Rule:** Published numeric claims, tables, mechanism assertions, wall-clock intervals, and paired comparisons in markdown files and PR bodies must carry truthful provenance tags, hardware counter evidence, confidence intervals, or explicit hypothesis/differentiation qualifiers.
- **Default:** `enabled = false` (opt-in).
- **False-Positive Rationale:** Field measurements on consumer repositories and public open-source projects indicate that `provenance-tags` produces excessive noise on tabular benchmark comparisons, descriptive configuration tables, and architectural diagrams that are illustrative or descriptive rather than novel-claim-bearing. Repositories publishing empirical research benchmarks and requiring strict provenance tagging can opt in via `[gates.provenance-tags] enabled = true`.
- **Languages:** Markdown (`*.md`) and PR description.
- **What it catches:**
  - Markdown tables containing unit-bearing numbers (`ns`, `µs`, `ms`, `ops/s`, `Mops/s`, `B/key`, etc.) without a provenance tag (`(measured: host, commit)`, `(target)`, or `(projected)`).
  - Unverified mechanism claims (`memory-latency-bound`, `branch-misprediction`, `TLB-bound`, etc.) without citing hardware counters (`perf stat`, `cycle_activity`, etc.) or marking as `hypothesis` / `unmeasured`.
  - Wall-clock ratios (`2.9x faster`, `3.1x speedup`) without confidence intervals (`[lo, hi]`, `BCa`, `CI`) or provisional markers.
  - Paired comparison figures (`11.9 ns vs 108.9 ns`, cross-metric comparisons) without shared workload tags (`(workload: id)`) or documented differentiation markers.
  - **Superseded figures** (with `superseded_registry`): a figure the repository has withdrawn, republished without a retraction marker (`retracted`, `superseded`, `corrected`, `previously`, ...) within three lines. The registry is a JSON file at `HEAD`: `{"figures": [{"id", "patterns", "context", "array_sequence", "replacement"}]}`; unknown fields are ignored. Patterns are case-insensitive and may use look-around. A pattern counts only when at least two of the figure's `context` words (one, if it lists one) appear in the same sentence or table cell or in the surrounding lines. Tracked JSON datasets matched by `superseded_json_paths` are swept value by value (string values by pattern, arrays by `array_sequence`; `provenance`, `retraction*`, `meta`, `description` and `_comment` keys are skipped). Changed files are swept; when the registry itself changes, every tracked markdown file and matching dataset is swept, so withdrawing a figure finds where it is already published.
  - **Pending measurements** (with `check_pending_citations`): a statement that a measurement is pending (`pending re-run`, `pending re-measurement`, `pending a quiet-host run`, ...) with no issue cited (`#123`, `issues/123`, or a GitHub issue URL) within the next 150 characters. With `require_open_pending_issues`, at least one cited issue must be open, checked with `gh api` (`DISCIPLINE_GH`, repository from `GITHUB_REPOSITORY` or a GitHub `origin`): text still saying "pending" after its issue closed is stale. Agent guides (`AGENTS.md`, `CLAUDE.md`, `GEMINI.md`) are not read for this check.
- **Could not check (exit 2):** a configured registry missing at `HEAD`, not JSON, or holding a pattern that does not compile; a swept dataset that is not JSON; with `require_open_pending_issues`, an issue whose state `gh` cannot report (not installed, unauthenticated, rate limited, no repository to resolve `#123` in). Each is named in the error.
- **Passing examples (accepted):**
  - Table caption carrying `*(measured: host, commit)*` or `*(target)*`.
  - Mechanism claim citing `perf stat` counters or labeled as `(hypothesis — unmeasured pending PMU counters)`.
  - Wall-clock speedup citing `[2.7x, 3.1x] BCa 95% CI` or `(provisional pending re-measurement)`.
  - Paired comparison citing `(workload: uniform-random)`.
- **Lifting directive:** `allow-provenance: <file-or-path> <reason>` in PR body or commit (aliases: `allow-unpaired-figures`, `discipline:allow(provenance-tags)`). Findings in the PR body itself are not liftable.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `check_tables`, `check_mechanisms`, `check_intervals`, `check_paired_figures`, `superseded_registry`, `superseded_json_paths`, `check_pending_citations`, `require_open_pending_issues`.

#### `pr-checklist`
- **Rule:** Reconciles ticked checklist items in PR descriptions (`- [x] Tests added/updated`, `- [x] Documentation updated`, `- [x] Benchmarks added`) against actual modified files in the pull request diff to prevent vacuous checkoffs. A test claim is backed by a changed test file, or by a test function the change adds or extends (more effective assertions) in any file a language pack analyses, so a `#[test]` added to a `mod tests` inside `src/` counts. A renamed test adds nothing.
- **Languages:** Any (test-function evidence: every shipped language pack).
- **What it catches:**
  - Ticked test checkboxes when no test file was modified and no test function was added or extended anywhere.
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
  - Deleted verification jobs and steps (`Deletion of Verification Step`). A base step is found in head by id, name, action, or first `run:` line; failing that, it is paired as a **rename** with an otherwise unmatched head step whose body (`run:` script without its full-line `#` comments, or action and `with:` inputs) has token Dice similarity of at least 0.60 (`STEP_RENAME_SIMILARITY`) and still carries every verification marker (`test`, `clippy`, `lint`, ...) the base body carried; ties go to the nearest position. A rename is reported in the gate notes, not as a violation, and the renamed step is still checked against its base form (dropped flags, `continue-on-error`). A step whose name and body both changed past the threshold, or whose body stopped verifying, is reported as deleted, with the closest candidate and its similarity in the message.
- **Passing commit / PR description (accepted):**
  ```text
  allow-ci-weakening: ci-gate temporary rollup relaxation during migration
  ```
- **What it does NOT catch:**
  - Local actions (`./...`) and docker actions (`docker://...`).
  - Workflows matching `excluded_jobs` (e.g. `detect-changes`).
- **Lifting directive:** `allow-ci-weakening: <subject> <reason>`.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `workflows`, `rollup_job`, `excluded_jobs`, `pin_actions`, `forbid_continue_on_error`, `forbid_or_true`, `diff_only`, `documented_job_count_path`, `documented_job_count_pattern`.

#### `ci-skip-set`
- **Rule:** A rollup job (`ci-gate`) has to count `skipped` as passing, because a conditional matrix skips the jobs a change does not touch. That rule alone cannot tell *skipped because irrelevant* from *skipped because the filter evaluation was wrong*: if change detection succeeds but emits all-false (a path-filter upgrade changing quantifier semantics, a renamed filter key resolving to empty), every conditional job skips, the rollup sees no failure, and a green required context sits over a run that verified nothing. This gate checks the skip set against the data the rollup actually observed. It asserts:
  1. The change-detection job (`change_job`, default `detect-changes`) is in the context and succeeded. A filter set nobody computed gates nothing.
  2. Every job in `unconditional_jobs` is in the context and was not `skipped`.
  3. For every job in the rollup's `needs`, `skipped` holds exactly when the job's `if:` is false under the observed outputs and dependency results. A job skipped under a true `if:` is the silent-narrowing case; a job that ran under a false `if:` means the observed outputs are not the ones the runner evaluated.
- **Runtime input, not a diff:** the data exists only inside the rollup job. The workflow passes `${{ toJson(needs) }}` through `DISCIPLINE_CI_CONTEXT` (inline JSON, or a path to a file holding it; action input `ci_context`); filter outputs are read from `needs.<change_job>.outputs` inside it. `github.*` values come from the runner's `GITHUB_*` variables. The binary makes no network call. The workflow file is read at `HEAD`. Without a context the gate reports `not evaluated: DISCIPLINE_CI_CONTEXT is not set` and examines nothing: it never passes or fails an ordinary diff check on this rule. Copy-paste rollup job: [CONFIGURATION.md § Rollup Skip-Set Check](CONFIGURATION.md#rollup-skip-set-check-ci-skip-set).
- **Evaluation model (GitHub Actions semantics):** `if:` is parsed, not pattern-matched. Modelled: `needs.<job>.outputs.<key>`, `needs.<job>.result`, `github.{event_name, ref, ref_name, ref_type, base_ref, head_ref, repository, repository_owner}`, string / number / boolean / `null` literals, `==`, `!=`, `!`, `&&`, `||`, parentheses, `contains`, `startsWith`, `endsWith`, and the status functions `always()`, `success()`, `failure()`, `cancelled()`. Comparison is GitHub's loose equality (strings case-insensitive, mixed types coerced to numbers). An `if:` with no status function carries GitHub's implicit `success()`, which requires every transitive dependency to have succeeded, so a job skipped because a dependency skipped is consistent. A `needs.<job>` reference outside the job's own `needs:` is not exposed by GitHub and is reported. `cancelled()` is true when any result in the context is `cancelled`. Anything else (`vars.*`, `inputs.*`, `github.event.*`, `fromJSON`, `<`, `>`, a `github.*` field whose variable is unset, a dependency missing from the context) is a `cannot be verified` finding.
- **What it catches:**
  - Change detection missing from the context, or not `success`.
  - An unconditional job missing or skipped.
  - A job skipped although its `if:` is true (for example, a push-event fallback `|| github.event_name != 'pull_request'` that did not fire).
  - A job that ran although its `if:` is false.
  - A context job the configured workflow does not define (the gate is pointed at the wrong file).
- **Named, not failed (notes):**
  - Every boolean output of the change-detection job is `false` on a pull request. Expected for a change touching no filtered path (a docs-only pull request); indistinguishable from an all-false evaluation by the data alone, so it is reported rather than blocked. The per-job check is the sound one: a filter that wrongly went false leaves some job's skip or run inconsistent with it.
  - An `if:` compares a change-detection output to `'true'`/`'false'`, but the output is absent from the context. GitHub omits empty outputs, so a renamed or removed filter reads as false and skips the job for the wrong reason.
- **What it does NOT catch:**
  - A filter that computes the wrong value from a correct change set (the golden change-set to job-set table belongs in the consumer's own tests).
  - Jobs that are not in the rollup's `needs` (`ci-integrity` owns the rollup allow-list).
  - Matrix legs individually: `toJson(needs)` carries one aggregated result per job.
- **Unreadable input fails closed:** a context that is not JSON, not an object, or has an entry without a string `result`, an unreadable context file, or a workflow missing at `HEAD`, exits 2.
- **Lifting directive:** none. A finding means the run's own evidence is inconsistent; the fix is the filter or the workflow, then a re-run.
- **Config keys:** `enabled`, `severity`, `exempt_paths` (matched against `workflow`), `workflow` (default `.github/workflows/ci.yml`), `change_job` (default `detect-changes`; `""` = the workflow has no change-detection job), `unconditional_jobs` (default `[]`).

#### `test-floor`
- **Rule:** Universal test count ratchet and floor sentinel. Operates in zero-config mode by default to prevent any drop in workspace AST test count across all supported languages relative to the base ref (with configurable `tolerance = 0`). When explicit floors are configured, reads test count floor constants and `min_tests` from the base ref (preventing PRs from silently lowering their own floor), enforces configured test count minimums, and ensures required test suite files exist. Complete test file deletions are detected and blocked.
- **Languages:** Any supported language pack (Rust, Python, JS/TS, PHPT, Java, Go, PHP, C/C++, C#, Ruby) or external test listing command (see [Counting basis](#counting-basis-static-or-runtime)).
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

##### Counting basis: static or runtime

`test-floor` can count tests two ways, and the two numbers for the same repository routinely differ. A floor is only meaningful against the basis it was measured on.

| | Static count (default) | Runtime count (`test_command`) |
|---|---|---|
| What is counted | Every test function the language packs find in tracked files (`#[test]`, `def test_*`, `@Test`, `it(...)` and so on), read from source without building anything. | The lines ending in `: test` that the command prints (the `cargo test -- --list` format), or a single integer if the command prints only that. |
| Sees | Every test function in the source, including tests behind a `#[cfg(...)]` or cargo feature that is off and tests in files no target compiles. | Only what compiled and was enumerated in that build: the enabled features, the current platform, the targets the command selects. It also sees what the source scan does not: doc-tests, and tests generated by macros or parametrisation. |
| Base ref | Counted the same way, so the zero-config ratchet (no floor configured) compares HEAD against the base ref on one basis. | Not available: the base ref would have to be checked out and built. A floor must therefore be configured (`min_tests`, or `constant_file` + `constant_name`); `test_command` without one exits 2 rather than compare a runtime count against a static one. |
| Cost | Parses tracked files; no toolchain needed. | Whatever the command costs: for `cargo test -- --list`, a full test build. |

**Why they differ.** Suppose the static count is 1046 and `cargo test -- --list` reports 300. Neither is wrong. The static scan counts every test function it can see, whether or not a build would include it; a workspace with large feature-gated or platform-gated suites, or test files outside any target, counts far higher statically. The runtime list counts one concrete build, so it drops everything that build excludes and adds doc-tests and generated cases the scan never sees. The gap is a property of the repository, not an error, and it is stable only as long as the build configuration is.

**Which to use.**
- **Static (the default)** when no floor exists yet or the goal is "no test function silently disappears": it needs no configuration and ratchets against the base ref by itself.
- **`test_command`** when porting a floor that was measured at runtime (a script that ran `cargo test -- --list` against a constant), when the number that matters is "tests that actually run in CI", or when a large share of the tests are generated. Keep the command's build configuration identical to the one the floor was measured with, or the count moves with the flags rather than the tests.

**Worked example: reproducing a `cargo test -- --list` floor.** A legacy script ran `cargo test --workspace --all-features -- --list`, counted the lines ending in `: test`, and failed below 300. The equivalent configuration:

```toml
[gates.test-floor]
enabled = true
# Same command and flags the floor was measured with. Runs from the
# repository root, without a shell: no pipes, no `| wc -l`.
test_command = "cargo test --workspace --all-features -- --list"
# The ported floor, on the same (runtime) basis.
min_tests = 300
```

The gate counts the lines ending in `: test` (`: benchmark` lines and the `N tests, M benchmarks` summaries are not counted), and that count is what `min_tests` is compared against: the gate reports it as the outcome's `examined` value and quotes it in a `Test Count Below Floor` finding. A failing command (non-zero exit, or a binary not on `PATH`) is a gate error with exit 2, never a count of zero. `tests/test_adoption.rs` pins this: a repository with two static test functions and a five-test listing passes a floor of 5 and fails a floor of 6 with the listing's count in the message.

To port the other way, adopting the static basis instead, run `discipline check` once with the gate enabled and no floor; the outcome's `examined` value is the static count to set as `min_tests`.

#### `archive-contents`
- **Rule:** Distribution archives (`.tar.gz`, `.tgz`, `.zip`, `.crate`, `.tar.bz2`, `.tar`) produced during packaging or release must contain all required files and zero forbidden developer artifacts, private files, or CI scripts.
- **Languages:** Any.
- **What it catches:**
  - Missing distribution archives when required (fails closed with exit 2).
  - Ambiguous archive glob patterns matching multiple candidate archives (fails closed with exit 2).
  - Missing `required_paths` in the archive (e.g. `config.m4`, `example_ext.h`, `LICENSE`, `README.md`).
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
  - Mismatched extracted versions across declared files in a group (e.g., `example_lib.h` has `"2.6.0"` while `package.xml` has `"2.6.1"`). The finding points at the file that drifted from the version most sources in the group agree on (a tie goes to the version declared first), and its message lists every source with the version it declares.
- **Failing diff example (rejected):**
  Bumping version in `package.xml` to `2.6.1` while `#define EXAMPLE_VERSION` in `example_lib.h` remains `2.6.0`.
- **Passing PR description (accepted):**
  ```text
  allow-version-mismatch: example-release staged release version bump across branches
  ```
- **What it does NOT catch:**
  - Unconfigured files or uncaptured version substrings.
  - Version increments in unversioned changelogs without regex capture groups.
- **Lifting directive:** `allow-version-mismatch: <group-name> <reason>` in PR description or commit message.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `groups` (`name`, `sources` (`path`, `regex`)).

##### Writing `groups`: a worked multi-ecosystem example

Each source is a file and a regex. The gate reads the **first match** of the regex in the file and takes capture group 1 as the version (the whole match when the regex has no group). So the regex must be anchored tightly enough that its first match is the project's own version and not a dependency's, a parent's or a toolchain floor. Every source in a group must declare the same version; a group needs at least two sources.

The example below keeps one library, `example-lib`, in lockstep across six ecosystems. It is not illustrative only: `tests/test_adoption.rs` reads this exact block from this file, builds a repository containing all six files (with decoy versions next to each real one), and checks that the gate passes when they agree and names the drifted file when any one of them is bumped alone.

<!-- version-lockstep-example:begin -->
```toml
[gates.version-lockstep]
enabled = true

[[gates.version-lockstep.groups]]
name = "example-release"

# Rust. `[package]` then the first line-leading `version =` after it, so a
# dependency's `version = "..."` inline table or `rust-version` never matches.
[[gates.version-lockstep.groups.sources]]
path = "Cargo.toml"
regex = '''(?ms)^\[package\].*?^version\s*=\s*"([^"]+)"'''

# npm. The first "version" key, which is the top-level one: dependency
# entries are keyed by package name, not by "version".
[[gates.version-lockstep.groups.sources]]
path = "package.json"
regex = '''"version"\s*:\s*"([^"]+)"'''

# NuGet. The <Version> element; a <PackageReference Version="..."> is an
# attribute, so it cannot match.
[[gates.version-lockstep.groups.sources]]
path = "dotnet/ExampleLib.csproj"
regex = '''<Version>([^<]+)</Version>'''

# Maven. The <version> that follows the project's own <artifactId>, not the
# <parent> block's version or a dependency's.
[[gates.version-lockstep.groups.sources]]
path = "java/pom.xml"
regex = '''<artifactId>example-lib</artifactId>\s*<version>([^<]+)</version>'''

# RubyGems. `spec.version = "..."`; `required_ruby_version` does not match
# because the regex needs `.version` directly.
[[gates.version-lockstep.groups.sources]]
path = "ruby/example-lib.gemspec"
regex = '''\.version\s*=\s*(?:"|')([^"']+)(?:"|')'''

# C header. The string macro, not EXAMPLE_VERSION_MAJOR and friends: the
# regex requires whitespace and a quote right after the macro name.
[[gates.version-lockstep.groups.sources]]
path = "include/example_lib.h"
regex = '''#define\s+EXAMPLE_VERSION\s+"([^"]+)"'''
```
<!-- version-lockstep-example:end -->

The files it reads look like this (the version is `1.4.2` everywhere):

| File | The line the regex captures from |
|---|---|
| `Cargo.toml` | `version = "1.4.2"` under `[package]` |
| `package.json` | `"version": "1.4.2",` |
| `dotnet/ExampleLib.csproj` | `<Version>1.4.2</Version>` |
| `java/pom.xml` | `<artifactId>example-lib</artifactId>` then `<version>1.4.2</version>` |
| `ruby/example-lib.gemspec` | `spec.version = "1.4.2"` |
| `include/example_lib.h` | `#define EXAMPLE_VERSION "1.4.2"` |

Notes for adapting it:
- Write regexes in TOML literal strings (`'...'` or `'''...'''`) so backslashes reach the regex engine unescaped. Use `'''...'''` when the regex itself contains a `'`, as the gemspec one does.
- `^` and `$` match only at the start and end of the file unless the regex starts with `(?m)`; `.` crosses newlines only with `(?s)`. The Cargo regex uses both.
- A crate that inherits its version (`version.workspace = true`) declares it in the workspace root's `[workspace.package]` table instead: point the source at that file with `(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"`.
- A gemspec that reads `ExampleLib::VERSION` holds no literal: point the source at `lib/example_lib/version.rb` with `VERSION\s*=\s*(?:"|')([^"']+)(?:"|')`.
- A group whose files are released separately belongs in its own group; lifting a mismatch names the group: `allow-version-mismatch: example-release <reason>`.

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
  - Memory growth in generic JSON rows with no usable timing signal (`{"median_ms": 0, "heap_bytes": 160, "rss_bytes": 20480}`): the row is a deterministic byte counter (`heap_bytes`, else `bytes`, else `rss_bytes`) gated like instruction counts.
  - Stale `exempt_arms` entries that match no benchmark arm in the run (error, whatever the gate severity). In git mode the arms are those of every tracked benchmark artifact at head.
- **Arm exemptions (`exempt_arms`):** an entry matches an arm by exact name, trailing-`*` literal prefix, `::` path suffix or its `/` parameter head, glob (`*.heap.*`, `*::random_*`), or the form the harness prints (`map_get random` for the reported arm `map_get/random`). A malformed glob is a configuration error (exit 2).
- **Degradation without failure:**
  When wall-clock benchmarks lack confidence intervals on either base or head, the engine degrades the verdict to **"not comparable (no CI available)"** in notes and does not fail the build on bare point estimates. A zero base timing estimate degrades to **"not comparable (zero base estimate)"** in the same way.
- **Passing override directive (accepted):**
  ```text
  allow-regression: search_bench intentional algorithmic trade-off for zero-allocation scan
  ```
- **What it does NOT catch:**
  - Uncommitted benchmark results (benchmark files must be committed or generated in CI workspace).
  - Wall-clock variance from co-resident CPU contention without sample distribution statistics.
- **Lifting directive:** `allow-regression: <benchmark-name-or-path> <reason>`.
- **Sourced overrides and citation freshness (`require_sourced_override = true`):** the reason must cite a CI run URL or a committed artifact path and name the arms it approves. Every citation in the reason is then checked, not only the first, because a reason with two citations rests on both:
  - a cited run whose conclusion is `cancelled`, `timed_out`, `action_required`, `startup_failure`, `stale` or `skipped` voids the override: it may have skipped the job whose numbers are quoted;
  - a cited run that concluded `failure` counts only if every job listed in `citation_measurement_jobs` that it started reached its guard step with every earlier step green. A regression trips the guard on numbers it measured; a crashed benchmark leaves none, and both conclude `failure`;
  - the cited run's head must be reachable from the head under review (`ahead` or `identical` in the compare API); a run at a head a force-push rewrote away measured other code;
  - a cited data artifact (`.json`, `.csv`, `.txt`, `.log`, `.out`) must be tracked and must not have been last committed before the branch's newest change under `citation_source_paths`. Prose and figures (`.md`, `.svg`) are cited as rules, not as the source of a number, and are not dated.

  Run, job and commit data come from `gh api`, run as an external tool with the same bounded runner the `command` gate uses (`DISCIPLINE_GH` names the executable; the binary itself has no network stack). A citation the job cannot decide (no `gh`, unauthenticated, rate limited, a non-GitHub run URL, no base ref, no `citation_measurement_jobs` for a `failure` run, no `citation_source_paths` for an artifact) is reported by name as **"citation not verified"** and the override is **not admitted**: the gate stays armed.
- **Paired-ratio mode (`mode = "paired-ratio"`):** gates a ratio of two arms measured in the same interleaved rounds (a subject and a fixed twin), compared against a committed ratio baseline: a ratio of ratios. A runner that is slower than yesterday's slows both arms and the ratio holds, which makes this the one sound way to gate wall-clock numbers on shared CI runners **when building the old version is impractical**. When the old version can be built and run in the same job, version-vs-version in the same run (the default mode with `--bench-base-file` and `--bench-head-file`) remains the preferred model: it needs no stored baseline and compares the change directly. The two are not interchangeable; a paired-ratio verdict is about the subject relative to its twin.
  - **Run file (`discipline-bench-ratio/v1`, passed with `--bench-head-file`):** `provenance` (`platform`, `runner_class`, optional `runner_id`, `commit`, `twin.identity`, `twin.version`) and named `axes` (`timing`, `memory`, ...), each with an `adverse` direction (`up` or `down`), gated `cells` carrying per-round data `rounds[] = {subject, twin, order}` with `order` `subject-first` or `twin-first`, and in-situ `controls` carrying `rounds[] = {a, b, order}` from two independently built arms of identical source interleaved into the same rounds.
  - discipline computes each cell's ratio (the median of the per-round `subject / twin`) and its 95% percentile-bootstrap interval. A supplied `ratio` is advisory; one that disagrees with its own rounds is an error. A cell with only means, fewer than 6 rounds (below that the bootstrap interval collapses onto the sample extremes), or an arm order that does not alternate is **"not comparable"**, never a pass.
  - **Controls are mandatory.** A run with an axis that carries no control cells is refused (exit 2). Every control cell must read null against its floor; one whose whole interval clears it makes the run **"not comparable"**, and any regression in that run is suppressed, not reported as a code regression.
  - **Thresholds are derived, not configured.** `discipline bench derive <run.json>...` computes, from repeated runs of the same commit, each axis's floor (p95 of every pairwise between-run drift, taken larger over smaller so the order the runs are listed in cannot move it, x 1.25, rounded up to 0.5pp, at least 1%) and each cell's floor (its own worst drift x 1.5, never below the axis floor until there are 8 runs across 4 distinct runners), and records them in the baseline beside the pooled ratios, with the twin's identity and its own historical band. A cell's threshold is the larger of its derived floor and this run's control scatter (p90 of |control ratio - 1|). `ratio_tolerance_pct` can only widen a derived floor; with no derived floor it is a configuration error.
  - A cell regresses only when the **whole** interval of its ratio of ratios clears `baseline x (1 +/- threshold)` in the adverse direction. A point estimate past the threshold with a straddling interval is reported as movement. Improvements are reported and never fail.
  - **The twin is part of the baseline.** A twin identity or version that differs from the baseline's reports **"baseline invalidated by twin change"** and nothing is compared across it. A twin whose own median leaves its historical band makes the cell "not comparable". There is deliberately no "every arm moved together, so it is runner noise" rule: a uniform move leaves the ratio unchanged and adds no signal, and treating it as noise would let a real uniform regression pass.
  - **The baseline is a threshold file.** `ratio_baseline` is read from the base ref, never from head. A change that loosens it (a floor, ceiling or twin band widened, a cell or axis removed, a cell made ungateable, a twin changed, or a stored ratio moved in the adverse direction) needs `allow-regression: <ratio_baseline path> <reason>`, and the directive appears in the overrides audit. So does a diff that touches the baseline together with non-benchmark source.
  - **Known limit.** A ratio of ratios is still a cross-run comparison, one level removed. The in-situ control validates this run's stability; it does not show that a baseline recorded on one runner generation stays valid after a runner fleet rotates to a different CPU generation. Re-derive the baseline when the fleet changes.
- **Config keys:** `enabled`, `severity`, `exempt_paths`, `tolerance_pct`, `paths`, `provenance`, `allow_cross_host`, `max_noise_cv`, `noise_margin_pct`, `exempt_arms`, `require_sourced_override`, `citation_source_paths`, `citation_measurement_jobs`, `mode`, `ratio_baseline`, `ratio_tolerance_pct`.

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

Discipline provides universal static binary drop-in replacements for the legacy verification scripts in high-assurance repositories:

| Gate | Replaced Legacy Script | Discipline Enhancements & Behavioral Differences |
|---|---|---|
| `assertion-reduction` | *(none — new capability)* | Multi-language AST extraction (10 language packs), callback-aware function tracking, compile-time assertions (`static_assert`, `const _: () = assert!`). |
| `vacuous-tests` | *(none — new capability)* | Language-specific AST helper detection (Python non-test methods, C/C++ non-zero return / throw helper recognition). |
| `ignored-tests` | *(none — new capability)* | Distinguishes newly arriving ignored tests from modified tests, configurable approved skip predicates (`cfg_attr(miri, ignore)`). |
| `deletion-rationale` | `scripts/check_deletion_rationale.py` | Line-anchored directive parsing, configurable `require_scope` and `allow_hidden` directive controls. |
| `time-estimates` | `scripts/check_docs_hygiene.py` | Clause-level (sentence-fragment) scoping of matches within a line, boundary lookarounds avoiding `\b` false positives on symbols (`×`, `~`), diff-scoped mode (`diff_only = true`), operational term-of-art and wrap window exemptions, `docs-lint: allow` alias. Paragraph scope applies to `allow_patterns` only (they match across soft-wrapped lines); built-in patterns do not detect an estimate split across a line break. |
| `pii` | `scripts/check_docs_hygiene.py` | Full test code inspection without blind spots, JSON string unescaping, cross-tree agent config directory/playbook detection, secret-backed hostname denylist. |
| `test-floor` | `scripts/check_test_floors.py` | Automatic base-ref constant extraction, direct `test_command` execution, fail-closed handling on unresolvable base floors, `allow-test-shrink:` override. |
| `ci-integrity` | `scripts/check_ci_gate.py` | Complete rollup job `needs:` closure validation, 40-character commit SHA pinning, masked failure detection (`continue-on-error`, `\|\| true`, `set +e`), `allow-ci-weakening:` override. |
| `ci-skip-set` | A rollup skip-set floor script | Parses each job's `if:` as an expression instead of splitting on `\|\|`, models GitHub's implicit `success()` over transitive dependencies, reads filter outputs from the same `toJson(needs)` as the results, reports an absent boolean filter output by name, fails closed on unmodelled terms. |
| `bench-regression` | `scripts/perf_report.py`, `scripts/wasm_fuel.py` | In-job dual-file mode (`--bench-base-file` and `--bench-head-file`), `iai-callgrind` console line and neutral JSON parsers, two-tier threshold (single-worst >5% or $\ge 2$ arms regressing >0.5% noise floor, advisory 0.1%), declared arm exemptions, sourced overrides verifying CI URL or committed artifact and named arms, missing-baseline fatal fail-closed. |
| `provenance-tags` | `scripts/check_docs_hygiene.py` | Table numeric provenance (`(measured: host, commit)`, `(target)`, `(projected)`), mechanism claim hardware counter citations, wall-clock intervals, paired comparison tags (`(workload: id)`), a superseded-figure registry over markdown and JSON datasets, and pending-measurement issue citations checked for an open issue. |
| `command` | Bespoke shell runner wrappers | Universal fail-closed timeout wrapper, zero-tests guards, turnkey presets (`cargo-public-api`, `miri`, `sanitizers`, `loom`, `cargo-deny`, `cargo-mutants`). |


**Not replaced:** a test that exercises a workflow's path filters against a golden change-set → job-set table. `ci-skip-set` checks that the rollup's skip set agrees with the filter outputs the run observed; it cannot tell whether a filter computed the right value from a correct change set. Keep that table in the repository's own tests.

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

All 31 gates across the six suites are implemented and shipped; `discipline gates` lists them with their effective state. Paired within-run ratio benchmarking shipped as `bench-regression` `mode = "paired-ratio"`. Known limitations and candidate work are tracked in the "Outstanding Checks & Known Limitations" section of [ROADMAP.md](ROADMAP.md).

