# Architecture

Engine design for `discipline`. Requirements and roadmap live in `docs/PRD.md`; this document explains how the code meets them and where its edges are.

**Superseded when:** the engine is restructured (for example the Phase 3 extractor-trait split). Update in place; do not fork.

## Module map

| Module | Responsibility |
|---|---|
| `src/config.rs` | Gate registry (`GATES`), schema, layered resolution |
| `src/gitctx.rs` | All git access: base resolution, merge-base diff, blobs, index |
| `src/tokens.rs` | Override-directive parser shared by every gate |
| `src/ast.rs` | Language dispatch and the Rust fact extractor (tree-sitter) |
| `src/guards/agent_diff.rs` | Diff gates over base-vs-head facts |
| `src/guards/hygiene.rs` | Whole-tree sweeps: `time-estimates`, `pii`, `agent-scratch`, `agents-md` |
| `src/guards/integrity.rs` | `config-integrity` (shipped), `golden-output` (shipped) |
| `src/guards/perf/` | `bench-regression` (shipped): mathematical bounds engine (`bounds.rs`) and fail-closed gate (`mod.rs`) |
| `src/guards/mod.rs` | Gate scheduling, `GateOutcome`, path filters, inline markers |
| `src/report/` | Terminal, annotations, job summary, step outputs, JSON |
| `src/selftest.rs` | Controls compiled into the binary |
| `src/style.rs` | ANSI styling (in-tree: the usual crate is MPL-2.0, outside `deny.toml`'s allow-list) |

## Control flow of `check`

1. **Resolve configuration.** Defaults, file, inline override, gate switches, and the secret denylist are merged as `toml::Value` trees and deserialized once under `deny_unknown_fields`. Gate ids are checked against the registry first, so a planned gate yields "planned, not available" rather than a generic unknown-key error.
2. **Open the git context.** `--staged` measures the index against `HEAD` (or the empty tree before the first commit). Otherwise `base` and `origin/<base>` are tried, then `merge-base(base, HEAD)`. Any failure is an `Err`, which `main` turns into exit `2`.
3. **Collect directive text:** the PR body plus every commit message from the merge base to `HEAD`.
4. **Run gates** in registry order. The five diff gates share one computation (`agent_diff::run`), performed once.
5. **Render**, then exit `0` or `1`.

Every gate returns a `GateOutcome`: violations, an `examined` count, the number of lines exempted by inline markers, and `notes` for anything it could not verify. The report prints all four for every gate on every run.

## The fact model and language packs

Gates never look at syntax. `ast::analyze` turns source into:

- `TestFn` — qualified name, line, total / strong / tautological assertion counts, `ignored`, `should_panic`
- `UnsafeSite` — line, kind, `documented`
- `has_parse_errors`

`language_for(path)` selects an extractor by extension; today only Rust has one. Extensions of other common source languages are listed in `UNSUPPORTED_SOURCE_EXTS` so that a change touching them is named in the AST gates' notes instead of reading as a clean zero.

A language pack is a grammar plus an extractor that fills the same facts from its ecosystem's conventions (PRD §6.1). Two parts of the current code are Rust-shaped and move behind the pack boundary in Phase 3:

- **Test identity.** `match_tests` pairs tests by module-qualified function name within a file, then by leaf name across files (a moved test). Callback-style frameworks identify a test by its description string and nesting, so identity must become a per-pack function.
- **Escape hatches.** `UnsafeSite` generalizes to "a site the language lets you opt out of a check, and whether it is justified".

### `SAFETY:` documentation rule

A site is documented when a comment containing `SAFETY:` is found:

1. in the contiguous run of comment and attribute lines directly above the `unsafe` node, or above any ancestor up to its enclosing statement (so a comment above `let v = unsafe { … };` or above a match arm counts); or
2. inline between the start of that statement and the `unsafe` keyword.

Only real comment nodes that begin their line qualify. A string literal containing the text, a trailing comment after the block, and a lower-case `Safety:` do not.

Scope: a site is reported when its line was added, or when the file's count of undocumented sites rose relative to the base. The second clause catches a `SAFETY:` comment deleted from above a block whose own line did not change.

#### Invariant Quality and Placeholder Rejection
Rather than relying on naive word-count thresholds, Discipline enforces substantive safety invariants:
- **Placeholder rejection:** Comments consisting solely of vacuous placeholders (`todo`, `tbd`, `n/a`, `safe`, `ok`, `fine`, `valid`, `trust me`, `temporary`, `placeholder`, `fixme`, `wip`, `noop`) or keyword restatements (`unsafe`, `safety`) are rejected.
- **Substantive threshold:** A comment is accepted if and only if it contains at least one substantive non-placeholder word, regardless of total length (e.g. `// SAFETY: caller-checked non-null.` passes; `// SAFETY: this is totally fine ok` fails).
- **Configurable placeholders:** Additional placeholder terms can be declared per-repository under `[gates.unsafe-safety-comment]` via `placeholders = [...]` in `discipline.toml`.

### Assertion strength

Two levels: *strong* (names containing `_eq`, `_ne`, `matches`) and everything else. A drop in either the effective total or the strong count is a reduction.

#### Tautologies
Tautologies are subtracted from the effective total:
1. **Verbatim equality:** `assert_eq!(x, x)` where arguments are textually identical non-empty tokens.
2. **Constant-expression tautologies:** For all assert macros, if tested operands consist exclusively of literals (integer, float, boolean, string, char) and operators (arithmetic, comparison, logical, unary, binary), with no identifiers, function calls, method calls, field accesses, or macro invocations. Examples include `assert!(true)`, `assert!(1 == 1)`, `assert!(1 + 1 > 0)`, `assert_eq!(1, 1)`, and `assert_ne!(1, 2)`. Dynamic expressions such as `assert!(f() == 1)`, `assert_eq!(N, 4)` (named const), and `assert!(x.len() > 0)` are preserved as valid assertions.

#### Implicit & Weak Assertions
1. **Fallible tests with `?`:** A `?` operator (`try_expression`) within a test function returning `Result` or `Option` represents a failing execution path; each counts as a weak assertion. A test returning `Result` or `Option` with no `?` and no assertions remains vacuous.
2. **`.unwrap()` and `.expect(...)` calls:** Counted as weak assertions across all test bodies. In test code, calling `.unwrap()` or `.expect("msg")` on a fallible value acts as an invariant assertion that panics on `Err` or `None`. They increment `total_asserts` (preventing vacuous test flags on pure happy-path unwrapping) while not inflating `strong_asserts` (equality/pattern match).

## Golden-output gate design

The planned `golden-output` gate guards against stealth re-blessing or silent modification of committed test outputs, snapshots (e.g. `insta` `.snap`), and serialized test fixtures.
- **Diff inspection:** Compares base vs. head blobs for all modified or deleted paths matching the gate's `paths` pattern.
- **Scoped escape hatch:** Requires an explicit `allow-golden-update: <path> <reason>` or `discipline:allow(golden-output): <path> <reason>` directive parsed through `src/tokens.rs`.
- **Integrity synergy:** Complements AST gates (`vacuous-tests`, `assertion-reduction`) by ensuring that tests asserting against external serialized data cannot be weakened by mutating the baseline fixture.

## Reporter architecture & Zero-Dependency SHA-256

Discipline emits structured reports across standard developer and enterprise interfaces:
- **GitLab Code Quality (`gl-codequality.json`):** Code Climate JSON array consumed natively by GitLab Merge Request widgets.
- **JUnit XML (`junit.xml`):** Validated offline against Jenkins `junit-10.xsd` with testsuite-level `<properties>` and testcase-level examined / override execution diagnostics in `<system-out>`.
- **SARIF (`discipline.sarif`):** OASIS Static Analysis Results Interchange Format v2.1.0 schema-compliant report with rule configuration overrides and examined counter property bags in `runs[0].invocations[0]`.

### Pure-Rust SHA-256 Fingerprint Rationale
GitLab Code Quality issues require a unique, deterministic 32-byte hex fingerprint to track issues across commits and prevent duplicate alerts. Discipline maintains a pure-Rust, zero-dependency SHA-256 implementation (`src/report/gitlab.rs`) adhering strictly to NIST FIPS 180-4:
1. **Zero crypto dependency footprint:** Relying on external cryptography crates (`ring`, `openssl`, `sha2`) would pull in large dependency graphs, C or assembly toolchain requirements, and potential licensing friction that complicates static musl binary compilation.
2. **Deterministic invariant:** Fingerprints are computed as `SHA-256(check_name:path:line:title:message)`.
3. **High-assurance test vectors:** The implementation is verified directly in unit tests against official NIST Cryptographic Algorithm Validation Program (CAVP) test vectors (`test_sha256_nist_vectors`: empty input, single-block `abc`, and 448-bit multi-block vectors).

## Known limits

- Macro bodies are unparsed token trees: tests generated inside `proptest! { … }` and assertions nested inside another macro's arguments are invisible. `assert_helper_fns` and `extra_assert_macros` are the workaround.
- Syntax newer than the bundled grammar is a parse error, which blocks by design; `exempt_paths` is the escape.
- Untracked files are not part of a non-staged diff. CI sees only commits, so this affects local runs only.
- Content scanners stream text files line-by-line without arbitrary size caps; lossy UTF-8 conversion scans NUL-containing text files.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | every enabled gate ran and found nothing blocking |
| `1` | violations (errors, or warnings under `--fail-on-warnings`) |
| `2` | the check could not run: bad configuration, no repository, unresolvable base, unreadable input, a suite with no available gates |

