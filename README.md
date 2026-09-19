# discipline

[![CI](https://github.com/orieg/discipline/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/orieg/discipline/actions/workflows/ci.yml)
[![Documentation](https://img.shields.io/badge/docs-orieg.github.io%2Fdiscipline-blue.svg)](https://orieg.github.io/discipline/)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-blue.svg)](#license)
[![Rust 1.90+](https://img.shields.io/badge/rustc-1.90%2B-orange.svg)](Cargo.toml)
[![Status](https://img.shields.io/badge/status-pre--release-yellow.svg)](docs/PRD.md)
[![pre-commit](https://img.shields.io/badge/pre--commit-enabled-brightgreen?logo=pre-commit&logoColor=white)](.pre-commit-hooks.yaml)

**CI gatekeeper and AI coding agent diff sentinel.** One static binary, the same in GitHub Actions, GitLab CI/CD, Gitea Actions, a pre-commit hook, and an agent's inner loop. Full documentation and interactive guides: [orieg.github.io/discipline](https://orieg.github.io/discipline/).

Coding agents in an iterate-until-green loop weaken assertions, add tests that assert nothing, mark tests `#[ignore]`, delete what is in the way, drop `// SAFETY:` comments, and — when a gate blocks them — edit the gate. `discipline` inspects the *change* (tree-sitter over a `git2` merge-base diff) and refuses those moves, with the fail-closed engineering distilled from [`orieg/expanse`](https://github.com/orieg/expanse).

> **Status: pre-release.** No version is published yet. Eleven gates are implemented and tested; verification and benchmark gates are planned and the binary refuses to pretend otherwise. See [`docs/PRD.md`](docs/PRD.md) for the roadmap and §11 for known limits.

## Gates

`discipline gates` prints this table with each gate's effective state.

| Gate | Suite | Languages | Rule |
|---|---|---|---|
| `assertion-reduction` | agent-guard | Rust, Python, JS/TS, PHPT | assertion count and strength may not drop in an existing test |
| `vacuous-tests` | agent-guard | Rust, Python, JS/TS, PHPT | a new test needs a non-tautological assertion |
| `ignored-tests` | agent-guard | Rust, Python, JS/TS, PHPT | a test may not become ignored or skipped |
| `unsafe-safety-comment` | agent-guard | Rust | `unsafe` needs a `// SAFETY:` comment; deleting one is caught |
| `deletion-rationale` | agent-guard | any | deleted files and removed tests need a scoped `removes:` |
| `agents-md` | agent-guard | any | `AGENTS.md` exists; `CLAUDE.md` / `GEMINI.md` do not fork it |
| `time-estimates` | hygiene | any | no calendar or duration estimates in markdown or the PR body |
| `pii` | hygiene | any | no home paths, LAN addresses, or denylisted hostnames in tracked text |
| `agent-scratch` | hygiene | any | agent scratch state is never tracked |
| `config-integrity` | integrity | any | a change cannot weaken its own `discipline.toml` without saying so |
| `golden-output` | integrity | any | committed snapshots and golden files cannot be modified or deleted without a scoped `allow-golden-update:` |

Seven gates work on a repository in any language. The four AST gates use a per-language pack; Rust, Python, JavaScript / TypeScript, and PHPT ship today, and Java / Kotlin, C / C++ and Go are planned. When a change touches source in a language without a pack, the AST gates **say so in the report** rather than showing a clean zero.

### Language packs & detection boundaries

The four AST gates (`assertion-reduction`, `vacuous-tests`, `ignored-tests`, `unsafe-safety-comment`) operate via tree-sitter AST extraction. Only packs that pass rigorous verification (F3–F6) ship today (Rust, Python, JavaScript / TypeScript, PHPT). Other language packs (Java / Kotlin, C / C++, Go) are planned and report as unanalysed source when touched.

Per-pack detection capabilities and boundaries:
- **Rust pack:**
  - *Detected:* `#[test]`, `tokio::test`, `async_std::test`; standard assertions (`assert!`, `assert_eq!`, `assert_ne!`, `matches!`, `assert_matches!`), tautological comparisons (`assert_eq!(x, x)`, `assert!(true)`); `#[ignore]`, `#[cfg_attr(..., ignore)]`; `unsafe` blocks and preceding `// SAFETY:` doc comments.
  - *Not detected yet:* Tests dynamically generated inside macro bodies (e.g. `proptest! { ... }`, `quickcheck! { ... }`); custom assert macros or helper functions unless registered in `extra_assert_macros` / `assert_helper_fns`.
- **Python pack:**
  - *Detected:* `def test_*`, `TestCase` methods; `assert`, `self.assert*`, and assertion context managers (`with self.assertRaises(...)`, `assertLogs`, `assertWarns`); tautological assertions (`assert True`, `self.assertEqual(1, 1)`); function and class decorators (`@pytest.mark.skip`, `@pytest.mark.skipif`, `@unittest.skip`, `@unittest.skipIf`) and module/class-level `pytestmark` skip markers.
  - *Not detected yet:* Dynamic test parametrizations (`@pytest.mark.parametrize` counts test definitions, not expanded invocations); assertions inside external helper functions unless declared in `assert_helper_fns`; class inheritance outside `unittest.TestCase`; dynamic runtime skip calls (`pytest.skip(...)`) inside test bodies.
- **JavaScript / TypeScript pack:**
  - *Detected:* `it(...)`, `test(...)`, describe suite nesting; standard matchers (`expect(...).toBe(...)`, `toEqual`, `toMatch`, `toBeTruthy`, etc.), tautological matchers (`expect(true).toBe(true)`, `expect(1).toBe(1)`); matcher weakening (`toBe` -> `toBeTruthy`, `toEqual` -> `toBeDefined`); skip helpers (`it.skip`, `test.skip`, `xit`, `xtest`, `describe.skip`, `xdescribe`).
  - *Not detected yet:* Programmatic test generators (e.g. `test.each(...)` or `items.forEach(...)` test generation loops); custom Jest/Vitest/Chai matchers unless registered in `assert_helper_fns` / `extra_assert_macros`; dynamic conditional skips (early `return`).
- **Golden (PHPT) pack:**
  - *Detected:* Standard PHPT sections (`--TEST--`, `--FILE--`, `--EXPECT--`, `--EXPECTF--`, `--EXPECTREGEX--`, `--SKIPIF--`, `--XFAIL--`); empty expectation sections; skips via `--SKIPIF--` and `--XFAIL--`.
  - *Not detected yet:* Dynamic runtime logic inside `--FILE--` or PHP script execution inside `--SKIPIF--`; multiple logical test cases embedded within a single `.phpt` file.

### Micro-benchmark regression tracking (`bench-regression`)

The `bench-regression` gate watches benchmark output files across revisions, comparing performance metrics against the merge-base baseline with configurable tolerance (`tolerance_pct = 0.5` by default):
- **Conservative confidence intervals (Vershynin 2018 §2; Brook 2014):** Gated on conservative interval clearing ($\Delta_{min} = \frac{L_{head} - U_{base}}{U_{base}} \times 100\%$), never bare point estimates. If 95% confidence intervals overlap, $\Delta_{min} \le 0\%$, correctly recognizing that data cannot reject the null hypothesis of no regression.
- **Deterministic instruction counts (IAI / Callgrind):** Parses `events: Ir` and `summary: <instructions>` from Callgrind output files (`callgrind.*`, `*.callgrind`).
- **Rust Criterion estimates:** Parses Criterion JSON files (`estimates.json`, `**/criterion/**`), extracting point estimates and `mean.confidence_interval`.
- **Go benchmarks:** Parses standard Go benchmark text (`go test -bench`), extracting nanoseconds per operation (`<name> ... <value> ns/op`). Automatically normalizes `GOMAXPROCS` suffixes (`BenchmarkSearch-8` -> `BenchmarkSearch`) so overrides match without guessing CPU count.
- **Python pytest-benchmark:** Parses `pytest-benchmark` JSON outputs (`benchmarks[].stats.mean`), tracking sub-millisecond execution times.
- **Google Benchmark (C / C++):** Parses JSON outputs generated via `--benchmark_format=json`, tracking `cpu_time` or `real_time` with declared `time_unit`.
- **Runner & host provenance:** Evaluates benchmark host tags; flags cross-host comparisons unless explicitly authorized via `--allow-cross-host-bench`.
- **Fail-closed contract:** Missing baseline artifacts exit `2`; unparseable artifacts exit `2`; deleted benchmark artifacts without `allow-regression:` exit `1` (generic `removes:` does not lift benchmark deletions); new or renamed benchmarks lacking baseline exit `1`.
- **Scoped override:** `allow-regression: <benchmark-or-file> <reason>` permits intentional algorithmic trade-offs when documented in the PR body or commit message.

## Fail-closed by construction

- Exit `0` pass, `1` violations, **`2` could not check**. An unresolvable base ref, a shallow clone, a missing repository, or a bad config is `2`, never an empty diff.
- Every report lists each gate's state and **how many items it examined**, plus the planned gates under "not checked".
- A planned gate cannot be enabled. Unknown config keys are errors.
- Override directives are line-anchored and scoped: prose *about* a directive never arms it.

## Use it

### GitHub Actions

```yaml
on:
  pull_request:
    types: [opened, synchronize, reopened, edited]

jobs:
  discipline:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
        with:
          fetch-depth: 0          # the merge base must be reachable
      - uses: orieg/discipline@v0
```

> **Note on `edited`:** GitHub Actions does not trigger workflows on PR description edits by default. Specifying `types: [opened, synchronize, reopened, edited]` ensures that updating the PR body (such as adding a `removes:` directive or resolving a PR-body hygiene finding) immediately re-runs the gate without requiring an empty commit.

### GitLab CI/CD

Native integration with GitLab Merge Requests. When running under GitLab CI (`$GITLAB_CI == "true"`), Discipline automatically detects merge request base refs and generates Code Quality diffs and JUnit test summaries.

Use the reusable CI/CD Catalog Component ([`templates/discipline.gitlab-ci.yml`](templates/discipline.gitlab-ci.yml)):

```yaml
include:
  - component: $CI_SERVER_FQDN/orieg/discipline/discipline@v0.1.0
    inputs:
      stage: test
```

Or configure a standalone job using the pre-built container:

```yaml
discipline:gate:
  stage: test
  image:
    name: ghcr.io/orieg/discipline:latest
    entrypoint: [""]
  variables:
    GIT_STRATEGY: clone
    GIT_DEPTH: 0
  rules:
    - if: $CI_PIPELINE_SOURCE == "merge_request_event"
    - if: $CI_COMMIT_BRANCH == $CI_DEFAULT_BRANCH
  before_script:
    - git fetch origin $CI_MERGE_REQUEST_TARGET_BRANCH_NAME --depth=100 || true
  script:
    - discipline check
  artifacts:
    reports:
      codequality: gl-codequality.json
      junit: junit.xml
    paths:
      - gl-codequality.json
      - junit.xml
    when: always
```

### Gitea Actions

The action is shell-only (no JavaScript runtime), so the same step works under `act_runner`. For runners without internet access, build or mirror the binary and pass `binary_path` or `download_url`.

```yaml
      - uses: https://github.com/orieg/discipline@v0
        with:
          binary_path: /opt/discipline/discipline
```

### pre-commit

```yaml
repos:
  - repo: https://github.com/orieg/discipline
    rev: v0.1.0
    hooks:
      - id: discipline            # builds with cargo; `discipline-system` uses a binary on PATH
```

Or as a plain git hook: `discipline check --staged`. Staged mode runs before a commit message exists, so findings that an override directive could lift are warnings locally and errors in CI.

### Container (Docker)

A minimal, statically linked non-root Alpine container image is available:

```bash
docker build -t discipline .
docker run --rm -v "$PWD":/workspace discipline check --base origin/main
```

### Argo Workflows

Use [`templates/argo-workflow-template.yaml`](templates/argo-workflow-template.yaml) to run `discipline` as a pre-merge gate in GitOps pipelines, capturing JUnit XML and SARIF reports:

```yaml
- name: run-discipline-gate
  templateRef:
    name: discipline-sentinel
    template: discipline-gate
  arguments:
    parameters:
      - name: repo-url
        value: "https://github.com/my-org/my-repo.git"
      - name: target-branch
        value: "main"
      - name: source-branch
        value: "feat/my-feature"
```

### CLI

```bash
discipline check --base origin/main        # everything, against the merge base
```

```bash
discipline check --staged                  # the index against HEAD
```

```bash
discipline gates                           # registry and effective state
```

```bash
discipline self-test                       # embedded positive / negative controls
```

## Configuration

With no configuration, every available gate runs at severity `error`. Everything is tunable, and nothing turns off silently: a disabled gate shows as `OFF` in every report.

Precedence, lowest to highest: built-in defaults → `discipline.toml` → `config_override` → `enable` / `disable` → `DISCIPLINE_HOSTNAME_DENYLIST`. Tables merge, **lists append**, scalars replace.

```toml
[meta]
version = 1
name = "my-project"

[gates.time-estimates]
enabled = false                        # any gate can be switched off ...

[gates.vacuous-tests]
severity = "warning"                   # ... or reported without blocking
assert_helper_fns = ["check_invariants"]

[gates.pii]
lan_ips = false
hostname_denylist = ["buildbox-7"]     # whole-token, never echoed in reports
extra_patterns = ['[a-z0-9._-]+@corp\.example']
exempt_paths = ["docs/archive/**"]
```

The same from a workflow, without touching the file:

```yaml
      - uses: orieg/discipline@v0
        with:
          disable: time-estimates
          hostname_denylist: ${{ secrets.DOCS_HOSTNAME_DENYLIST }}
          fail_on_warnings: true
          config_override: |
            [gates.pii]
            extra_patterns = ['[a-z0-9._-]+@corp\.example']
```

A single line can opt out with `discipline:allow(<gate-id>)` (for markdown, inside an HTML comment). The report counts how many lines did.

Loosening an existing `discipline.toml` (disabling a gate, lowering a severity, growing an exemption list, shrinking a denylist) is itself a violation of `config-integrity` unless the change says why. Full schema: [`docs/PRD.md` §5](docs/PRD.md).

## Override directives

On its own line in the PR body or a commit message; the reason must name what it covers. Directives are audit-trailed in reports and step outputs.

```
removes: tests/legacy/ replaced by the property suite
allow-assertion-drop: inserts_in_order second case moved to proptest
allow-ignore: big_alloc needs the new allocator first
allow-gate-weakening: vacuous-tests suite asserts through snapshot macros
allow-golden-update: tests/snapshots/result.snap re-blessed output
```

Directives policy can be configured via `[directives]` in `discipline.toml`:
- `sources = ["pr-body", "commits"]` — allowed channels for directives.
- `allow_hidden = false` — when false (default), HTML-comment-wrapped directives are rejected.
- `fail_on_overrides = false` — when true (or via `--fail-on-overrides`), any applied override fails the check, requiring explicit human sign-off.

## Action outputs

`status`, `errors`, `warnings`, `failed_gates`, `overrides`, `overridden_gates`, `report` (path of the JSON report), `install_error`.

## Development

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

Every gate needs a unit test, an end-to-end test through the binary, a mutation that the suite kills, and a `self-test` case; see [`AGENTS.md`](AGENTS.md) §3.4. Releases are cut by pushing a `vX.Y.Z` tag ([`docs/PRD.md` §8.2](docs/PRD.md)).

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
