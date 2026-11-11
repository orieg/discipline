# Per-edit hook latency (`#982`)

Every agent edit hook runs a full `discipline check` as a child process (`READ: src/hook.rs::run_check_under`, which runs `check --format agent-prompt --quiet --json-out`). Its cost scales with the repository and with the change against the base, not with the edit. This document holds the method, the script and the figures for that cost, and the decision the figures feed.

**Status:** measured (section 4); the fast-path decision (section 5) is recorded on `#982`. Superseded when a fast path or cache for the edit hook ships and this benchmark is re-run against it.

---

## 1. What is timed

`scripts/measure_hook_latency.py` clones each repository into a scratch directory, commits a one-file change on a new branch (a comment line appended to the largest tracked source file under 300 KB), runs `discipline init` when the repository has no `discipline.toml`, installs the Claude Code hooks (`hook install --agent claude-code --observe`; an existing hook file is kept) and times these cells:

| Cell | Command | What it isolates |
|---|---|---|
| `check-one-file` | `check --base <base> --quiet` | A full check of a one-file change |
| `check-large` | `check --base <base>~N --quiet` (`N` = 200 commits, or the history length) | A full check of a large branch |
| `hook-post-edit` | a `PostToolUse` Edit payload piped through `hook run --agent claude-code --observe` | The edit hook as an agent runs it, observe-log write included |
| `hook-pre-tool` | the same payload through `hook run --agent claude-code --event pre-tool --observe` | The pre-tool hook |
| `git-diff-names-*`, `git-diff-*` | `git diff --name-only` and `git diff` over both ranges | The floor: what reading the change costs before any gate runs |
| `suite-<suite>` | `check --suite <suite>` for `agent-guard`, `hygiene`, `integrity`, `quality`, `verification`, `bench` on the one-file change | Where the time goes, by suite |

Every run sets `DISCIPLINE_NO_NETWORK=1`, so no forge request enters a timing. Exit code 2 (could not check) is recorded per cell; a cell whose runs exit 2 times a failure path, not a check.

**Repositories:** this repository (889 tracked files at `efc84600` when measured) and `BurntSushi/ripgrep` (a public Rust repository of a different size, already in the precision benchmark's evaluation set). The script records each repository's head, base, file count, commit count and the number of files the large range changes.

**Runs:** one warm-up run, then 7 timed runs per cell (`--runs`, `--warmup`). The report is the median and the spread (minimum, maximum, interquartile range), plus every sample.

**Binary:** a release-profile build, `cargo build --release --locked`, its version recorded in the output.

## 2. Where the time goes

The binary exposes no per-gate timing (`READ`: the only `Instant` timers in `src/` are the command and git timeouts in `src/guards/command.rs` and `src/gitctx.rs`; the observe log entry in `src/hook_log.rs` records time, decision, exit code and finding counts, not a duration). The breakdown therefore comes from three sources:

1. **The git floor:** the `git-diff-*` cells.
2. **Per suite:** the `suite-*` cells. A suite's cost is its cell minus the shared start-up (configuration, opening the repository, the diff), which the smallest suite cell bounds from above.
3. **A profiler**, for the suites that dominate: `sample <pid> 5 -file out.txt` on macOS, or `perf record -g -- <command>` on Linux, on the `check-one-file` command. The frames name the parsing (`tree-sitter`), git (`libgit2`) and per-gate functions.

`check --timings` (`#995` step 1) now reports wall time and counters per gate and per shared phase, which replaces source 3 for a breakdown inside a suite (section 4.3).

## 3. Host requirement

Before the first run and before every run of every cell, the script checks the host and stops with exit 3 when it is not idle: `load1` above half the core count, or any process other than itself above 100% CPU. It never waits and never times on a busy host. A stopped run writes the cell it stopped at and the reason to the output JSON.

Run it with free disk of 20 GB or more and nothing else building:

```bash
cargo build --release --locked
python3 scripts/measure_hook_latency.py --binary target/release/discipline --repo . --repo https://github.com/BurntSushi/ripgrep --out hook-latency.json
```

The script's own checks: `python3 scripts/measure_hook_latency.py --test` (not wired into CI: `.github/workflows/` is a ratified path).

## 4. Figures

**Host (RUN, 2026-11-02):** one AWS `c7g.2xlarge` (Graviton3, 8 vCPU arm64, 16 GB, Amazon Linux 2023, kernel 6.18), instance `i-0b53f1b0b462f5c2b`, terminated after the run. Binary: `cargo build --release --locked` at `efc84600` (rustc 1.99.1, `discipline 0.29.0`). `load1` 0.68 / `load5` 0.93 / `load15` 0.50 at the preflight, CPU 100% idle, 15.1 GB of memory available, no process above 100% CPU; `load1` between 0.68 and 1.31 before every timed run (the timed process itself is about one core) and 1.26 after the last. Raw samples and host snapshots: [`results-hook-latency/`](results-hook-latency/hook-latency.json) (`hook-latency.json`, `preflight.txt`, `postflight.txt`). One warm-up and 7 timed runs per cell; seconds of wall time, median with minimum and maximum, all (measured).

| Cell | `orieg/discipline` (889 files) | `BurntSushi/ripgrep` (225 files) |
|---|---|---|
| `hook-post-edit` | **47.31** (47.25–47.35) | **9.34** (9.34–9.35) |
| `check-one-file` | 47.27 (47.24–47.31) | 9.32 (9.32–9.33) |
| `check-large` | 53.86 (53.82–53.87), exit 2, see below | 29.24 (29.22–29.25), 91 files changed |
| `hook-pre-tool` | 0.005 (0.005–0.006) | 0.005 (0.004–0.005) |
| `git-diff-one-file` / `git-diff-large` | 0.006 / 0.255 | 0.006 / 0.032 |
| `suite-integrity` | 40.52 (40.50–40.53) | 6.03 (6.03–6.03) |
| `suite-agent-guard` | 11.33 (11.32–11.34) | 4.63 (4.62–4.64) |
| `suite-hygiene` | 1.62 (1.61–1.62) | 0.86 (0.86–0.86) |
| `suite-quality` / `suite-verification` / `suite-bench` | 0.79 / 0.79 / 0.80 | 0.73 / 0.73 / 0.74 |

Commits measured: `orieg/discipline` at `efc84600` plus a one-line change to `src/ast/runner_collection.rs`, large base `05c3955f` (200 commits back, 440 files changed); `BurntSushi/ripgrep` at `91f5d22e` plus a one-line change to `crates/core/flags/defs.rs` (no `discipline.toml`, so `discipline init` defaults), large base `6887122e` (200 commits back). The spread is under 0.2% of the median in every `check` and hook cell.

**Where the time goes (measured, by suite):**

- **Git is not the cost.** Reading the one-file change takes 6 ms on both repositories (`git-diff-one-file`).
- **The cost is per repository, not per edit.** The one-file `check` costs 47.3 s on `orieg/discipline` and 9.3 s on `ripgrep`, and the hook adds nothing measurable over it (47.31 against 47.27). The large branch costs 14% more than one file on `orieg/discipline` and about three times as much on `ripgrep`, so most of the one-file cost is work over the whole repository rather than over the change.
- **`integrity` is 86% of it on `orieg/discipline`** (40.5 of 47.3 s) and 65% on `ripgrep` (6.0 of 9.3 s); `agent-guard` is most of the rest. The `quality`, `verification` and `bench` cells, about 0.73–0.80 s each, bound the shared start-up from above. The suite cells overlap that start-up, so they do not sum to the full check.
- **One core.** The `check` process ran at about 100% of one core on 8 idle cores (RUN: `ps` over SSM during `suite-integrity`), and the engine has no thread pool (READ: no `rayon` or equivalent in `Cargo.toml`; the `thread::spawn` calls in `src/` are for I/O, timeouts and `replay`).
- **Within `integrity`, `test-floor` carries the cost**: 40.05 s here and 4.72 s on `ripgrep` (measured; section 4.1).
- **`check-large` on `orieg/discipline` exited 2** (could not check) on every run, so its 53.9 s is the cost of reaching that failure, not of a check. The 200-commit range edits protected paths (`discipline.toml`, `.github/workflows/`), so `ratified-paths` refuses to judge it with the head's policy: `gates.ratified-paths must be judged by the base ref's policy (--policy-from base)` (RUN: the same command on the development host with the `discipline 0.29.0` release, exit 2). The run still costs 53.9 s before it reports that. A large-branch figure with `--policy-from base` is not measured; `ripgrep`'s `check-large` (exit 1, a completed check) is the large-branch figure. The script now keeps the stderr of an exit-2 run (`stderr_tail_exit_2`).

`hook-pre-tool` returns in 5 ms on both repositories: it does not run a full check.

### 4.1 Per-gate attribution of `integrity` (`#994` step 1)

**Host (RUN, 2026-11-02):** one AWS `c7g.2xlarge` (Graviton3, 8 vCPU arm64, 16 GB, Amazon Linux 2023), terminated after the run. Binary: `cargo build --release --locked` at `5a60d9f1` (`discipline 0.29.0`). At the preflight, `load1` was 0.92, no process was above 1% CPU, and 56 GB of disk was free. `load1` was at most 1.08 before every timed run and 1.03 after the last. Raw samples and host snapshots are in [`results-hook-latency/`](results-hook-latency/): `integrity-per-gate-*.json`, `integrity-preflight.txt` and `integrity-postflight.txt`. In the JSON, the clone path on the instance is replaced by the repository name.

**Method:** `scripts/measure_integrity_gates.py`. In a scratch clone, every integrity gate but one is set to `enabled = false`, and that configuration is committed as the base commit, so the base and head sides share one policy. The head commit appends one comment line to the file section 4 changed. Each cell times `check --base HEAD~1 --suite integrity --quiet` with `DISCIPLINE_NO_NETWORK=1`. Before every timed run, the script checks the host and stops (exit 3) unless `load1` is at most half the core count and no process is above 100% CPU. One warm-up, then **3 timed runs per cell**; every run exited 0. Seconds of wall time, median with minimum and maximum, all (measured):

| Gate left on | `orieg/discipline` | `BurntSushi/ripgrep` |
|---|---|---|
| none (the shared start-up) | 0.815 (0.815–0.817) | 0.754 (0.752–0.756) |
| all, as configured | 41.59 (41.54–41.65) | 6.19 (6.19–6.19) |
| **`test-floor`** | **40.86** (40.82–41.32) | **5.47** (5.47–5.48) |
| `test-budget` | 1.545 (1.542–1.547) | 1.436 (1.434–1.437) |
| each of the other 11 | 0.815–0.836 | 0.751–0.760 |

The gates add up: start-up plus each gate's increment matches the "all" cell to within 0.1 s on both repositories. Above start-up, `test-floor` costs 40.05 s, 98% of the suite's increment here, and 4.72 s (87%) on `ripgrep`. `test-budget` costs 0.73 s and 0.68 s. No other gate costs more than 0.02 s.

**Profile:** `perf record -F 199 --call-graph dwarf,32768` on the same host. The binary was rebuilt unstripped with line tables (`CARGO_PROFILE_RELEASE_STRIP=false CARGO_PROFILE_RELEASE_DEBUG=line-tables-only`), because the release profile strips. One run of `check --base HEAD~1 --suite integrity --quiet` on the one-line change, with `load1` at 1.46 and 1.22 before the two runs. The reports are `integrity-perf-<repo>-children.txt` (inclusive, frames above 1.5%) and `-self.txt` (exclusive, frames above 0.8%). Inclusive share of samples on `orieg/discipline` (8K samples) (measured):

| Frame | Share |
|---|---|
| `test_floor::evaluate_test_floor` | 85.2% |
| └ `count_base_workspace_ast_tests`: every base-side tracked file | 40.2% |
| └ `count_workspace_ast_tests`: every head-side tracked file | 40.0% |
| └ `SideVocabularies::base` / `::head`: runner rules, where `scan_rust_modules` parses every Rust file for `mod` declarations | 5.3% / 4.8% |
| `RustPack::extract`, all callers | 79.4% |
| └ tree-sitter parse (`parse_within_budget`) | 21.4% |
| └ `facts_to_method_checks` | 12.4% |
| └ `calls::count` | 12.3% |
| └ `caught_assertions::walk_tree` | 10.6% |
| └ `reach::dead_ranges` | 6.0% |

The rows under `extract` overlap, so they do not sum. On `ripgrep` (1K samples), the head and base counts take 30.7% and 30.3% of samples, `test_budget::read_ast_budgets` takes 10.3%, and `extract` 79.6%.

**What it shows:**
- This repository has no `[gates.test-floor]` table, so `test-floor` runs the zero-config static ratchet. That ratchet runs a full `extract` on every supported tracked file twice, once on the head side and once on the base side (READ: `src/guards/test_floor.rs` `measure_test_count` and `compare_with_base_static_count`).
- The count reads only `has_parse_errors`, `notes`, `tests.len()`, and each test's `ignored` and `conditional_ignore` (READ: `AstTestCount::add`).
- `reparsed_expression`, `tracked_files` and `changed_files` each fall below the 1.5% cut-off.
- `agent-guard` (11.3 s in section 4) is not profiled here.

The design that follows from this attribution is on `#994`; section 4.2 measures its engine changes.

The 60–120 s wall time reported for `check` and `diff` in the `#966` review was taken at a load around 60–70 and is not used here. An earlier attempt on the development host (Apple M1, 2026-11-01) was refused by the script at `load1` 29.54 (RUN, exit 3) and timed nothing.

### 4.2 Re-measurement after the engine fast path (`#994`)

**Change measured:** `test-floor`'s static counts key each file's contribution by path, git blob id and assertion vocabulary for one evaluation of the gate, so a file the change leaves alone, under the same vocabulary on both sides, is extracted once rather than once per side. The Rust module scans that build both sides' runner rules are kept for the run by the blob id of the source. The count reads a tests-only extraction (`LanguagePack::extract_test_counts`), and the Rust pack skips the analyses it never reads. Nothing is skipped or answered from a stale result: when the vocabularies differ, both sides are counted in full, and nothing is stored between runs.

**Host (RUN, 2026-11-02):** one AWS `c7g.2xlarge` (Graviton3, 8 vCPU arm64, 16 GB, Amazon Linux 2023, kernel 6.18), terminated after the run. Both binaries were built on it with `cargo build --release --locked` (rustc 1.99.1): `main` at `f45fa68f`, and the change at `f2fc9cfb`. The commits after `f2fc9cfb` on the branch add tests and a merge of `main` that brings in `#1000`'s `--timings` counters, which no timed command here turns on. `load1` was 1.09 at the preflight, with no process above 100% CPU. It was between 1.00 and 1.20 before every timed run, and 1.00 after the last. The timed process itself is about one core. Each repository is the script's scratch clone with its one-line change, as in section 4: `orieg/discipline` at `f2fc9cfb` (1,366 files, change to `src/hook.rs`), and `BurntSushi/ripgrep` at `91f5d22e` (change to `crates/core/flags/defs.rs`). The change was timed with 7 runs per cell and `main` with 3, after one warm-up each. Values are seconds of wall time, median with minimum and maximum, all (measured). Raw samples: [`fast-path-hook-latency.json`](results-hook-latency/fast-path-hook-latency.json) and [`fast-path-hook-latency-main.json`](results-hook-latency/fast-path-hook-latency-main.json). Host snapshots: `fast-path-preflight.txt`, `-midflight.txt` and `-postflight.txt`.

| Cell | `orieg/discipline`, `main` | `orieg/discipline`, change | `ripgrep`, `main` | `ripgrep`, change |
|---|---|---|---|---|
| **`hook-post-edit`** | 47.58 (47.48–47.61) | **19.60** (19.52–19.66) | 9.40 (9.38–9.47) | **6.35** (6.32–6.37) |
| `check-one-file` | 47.52 (47.49–47.55) | 19.62 (19.51–19.66) | 9.38 (9.38–9.47) | 6.35 (6.33–6.37) |
| `suite-integrity` | 40.88 (40.87–40.96) | 14.43 (14.42–14.51) | 6.07 (6.04–6.11) | 3.12 (3.11–3.13) |
| `suite-agent-guard` | 10.83 (10.79–10.92) | 7.22 (7.19–7.25) | 4.66 (4.66–4.69) | 4.19 (4.18–4.24) |
| `check-large` | 50.68 (50.66–50.69), exit 2 | 50.42 (50.37–50.47), exit 2 | 29.45 (29.33–29.45) | 27.53 (27.46–27.69) |
| `suite-hygiene` / `-quality` / `-verification` / `-bench` | 1.83 / 0.72 / 0.72 / 0.74 | 1.83 / 0.73 / 0.72 / 0.73 | 0.86 / 0.73 / 0.73 / 0.74 | 0.87 / 0.73 / 0.74 / 0.74 |
| `hook-pre-tool` | 0.005 | 0.005 | 0.004 | 0.004 |

`check-large` on `orieg/discipline` exits 2 for the reason given in section 4, on both binaries.

**Same output (RUN):** on the same host, each binary ran `check --base <base> --suite integrity --format json` with `DISCIPLINE_NO_NETWORK=1` on a one-line change to each repository's largest Rust file. The two reports are byte-identical on both repositories: findings, notes and every gate's `examined` (`test-floor` examined 4,309 tests on `orieg/discipline` and 496 on `ripgrep`). The reports are `fast-path-same-output-<repo>.json` and `-main.json`. The unit tests that compare the gate with and without the memo, and the tests-only extraction with the full one, are in `src/guards/test_floor.rs` (`count_memo_tests`).

**What it shows:**
- **`integrity` drops by 65% here** (40.88 to 14.43 s) and by 49% on `ripgrep` (6.07 to 3.12 s). `hook-post-edit` drops by 59% and 32%.
- **`agent-guard` drops too** (10.83 to 7.22 s here). It builds both sides' runner rules, and the module scans of an unchanged Rust file are now shared between them.
- **Against the pre-registered threshold:** 19.60 s and 6.35 s are still above the 2 s median, and both maxima (19.66 s, 6.37 s) are above 5 s. The engine changes shorten Stop, pre-commit and CI. They do not bring the edit hook under the threshold. As `#994` projected, that depends on `#995`'s event-scoped gate selection, which reports the change-set gates as not evaluated on Edit.
- **A change that adds, removes or renames a Rust test target** changes the runner rules, so the two vocabularies differ and both sides are counted in full, as before (`a_change_to_the_runner_rules_counts_both_sides_in_full`). The one-line changes timed here keep the rules equal. Sharing across vocabularies that differ only in rules the count does not read is not done.

### 4.3 Re-measurement after event-scoped selection (`#995` step 2)

**Change measured:** the check an agent's edit hook runs evaluates the `file` and `change set` gates. It evaluates a `repository` or `provenance` gate only when the change touches one of the gate's trigger paths, and reports the rest as not evaluated (`docs/GATES.md`, "Scope on an Agent Edit"). On these one-line changes, the edit leaves 13 gates out, `test-floor` and `test-budget` among them. Stop, pre-commit and CI still run every gate. The detached observe-mode check (`#995` step 3) is not in either binary measured here: `hook-post-edit` below is the time the agent waits for the check, as in sections 4 and 4.2.

**Host (RUN, 2026-11-03):** two AWS `c7g.2xlarge` instances, one per binary (Graviton3, 8 vCPU arm64, 16 GB, Amazon Linux 2023, kernel 6.18), driven over SSM only, set to terminate on shutdown, and verified `terminated` after each run. Each built its binary with `cargo build --release --locked` (rustc 1.99.1, `discipline 0.29.0`):

- `0d66c054` (`#1001`): step 2 alone, without `#994`'s engine change.
- `1de13bc9`: `main` with both `#994` (`#1002`) and step 2.

`load1` was 1.12 and 1.11 at the preflights, with no process above 1% CPU. It was between 1.00 and 1.34 before every timed run, and 1.04 and 1.00 after the last. The method is section 1's: `scripts/measure_hook_latency.py`, one warm-up and 7 timed runs per cell, `DISCIPLINE_NO_NETWORK=1`. The script changes the largest tracked source file under 300 KB, so the edited file on `orieg/discipline` is `src/ast/runner_collection.rs` at `0d66c054` and `src/hook.rs` at `1de13bc9` (1,367 and 1,376 files); on `ripgrep`, it is `crates/core/flags/defs.rs`. Values are seconds of wall time, median with minimum and maximum, all (measured). Raw samples: [`edit-scoped-hook-latency.json`](results-hook-latency/edit-scoped-hook-latency.json) (`0d66c054`) and [`edit-scoped-main-hook-latency.json`](results-hook-latency/edit-scoped-main-hook-latency.json) (`1de13bc9`). Host snapshots: `edit-scoped-preflight.txt`, `edit-scoped-postflight.txt` and the `edit-scoped-main-` pair.

| Cell | `orieg/discipline`, step 2 | `orieg/discipline`, `#994` + step 2 | `ripgrep`, step 2 | `ripgrep`, `#994` + step 2 |
|---|---|---|---|---|
| **`hook-post-edit`** | **12.72** (12.71–12.76) | **8.44** (8.42–8.47) | **4.84** (4.84–4.85) | **4.35** (4.34–4.35) |
| `check-one-file` (every gate) | 48.46 (48.40–48.51) | 19.73 (19.69–19.76) | 9.44 (9.42–9.46) | 6.35 (6.34–6.37) |
| `suite-integrity` | 41.23 (41.16–41.24) | 14.54 (14.50–14.57) | 6.08 (6.08–6.10) | 3.12 (3.11–3.13) |
| `suite-agent-guard` | 11.54 (11.52–11.55) | 7.29 (7.26–7.32) | 4.69 (4.68–4.70) | 4.20 (4.19–4.23) |
| `hook-pre-tool` | 0.005 | 0.005 | 0.004 | 0.004 |

**Against the pre-registered threshold (section 5): not met on either repository.** On `orieg/discipline`, the median 8.44 s is above 2 s and the maximum 8.47 s is above 5 s. On `ripgrep`, the median 4.35 s is above 2 s, while the maximum 4.35 s is under 5 s. Event scoping removes `test-floor` from the edit: 47.31 s became 12.72 s here, and 9.34 s became 4.84 s on `ripgrep`. With `#994` as well, the edit costs 8.44 s and 4.35 s, against 19.60 s and 6.35 s for `#994` alone (section 4.2).

**Where the edit's time goes (`--timings`, `#995` step 1):** one `check --timings` per event, at each commit, on its own one-line change. The changed files are `src/ast/runner_collection.rs` and `crates/core/flags/defs.rs`, with the hook's own `--policy-from base`. This is one run each, so the wall times are indicative; the counters are deterministic. Reports: `edit-scoped-timings-<repo>-<event>.json` and `edit-scoped-main-timings-<repo>-<event>.json` (the `timings` object, the counts, and the gates not evaluated). At `1de13bc9` (measured):

| Scope | `orieg/discipline`, edit | `orieg/discipline`, stop | `ripgrep`, edit | `ripgrep`, stop |
|---|---|---|---|---|
| total | 9.12 | 20.42 | 4.36 | 6.37 |
| `test-floor` (gate) | not evaluated | 10.53 (414 extractions) | not evaluated | 1.32 (97 extractions) |
| `vocabulary` (runner rules, both sides) | 2.58 | 2.55 | 0.37 | 0.38 |
| `agent-diff` (shared by five gates) | 1.88 | 1.88 | 0.72 | 0.73 |
| `review-routing` | 0.79 | 0.80 | 0.73 | 0.73 |
| full extractions, whole run | 15 | 431 | 11 | 110 |

- **The edit's remaining cost is the changed file, parsed over and over.** On `ripgrep`, the edit makes 11 full extractions of `defs.rs` (both sides of a one-file change, about 0.35 s each). These are `agent-diff` and `review-routing`, plus `stub-bodies`, `error-swallowing`, `suppression-delta` and `instruction-smuggling`, each of which extracts the file again. `pii` does the same on `orieg/discipline`. On `orieg/discipline`, the same pattern adds the `vocabulary` phase (2.58 s): both sides' runner rules, rebuilt on every run.
- **What would close the gap** (not done here): one extraction per file and side, shared by every gate of the run, and runner rules that are not rebuilt for a change that cannot alter them. These are engine changes, separate from `#995`'s scope, and they keep the not-across-runs constraint of section 5.

The `#995` detached check (step 3) changes what the agent waits for in observe mode, but not the check's own cost. The hook answers once it has written the `started` line, and the check's time is in the `finished` line's `duration_ms`. Enforcing mode still waits for the check, so the figures above are the cost of an enforcing edit hook.

### 4.4 The 0.32.0 code, with the enforcing edit hook timed (`#1056`)

**Host (RUN, 2026-11-10):** one AWS `c7g.2xlarge` (Graviton3, 8 vCPU arm64, 16 GB, Amazon Linux 2023, kernel 6.18), driven over SSM only and verified `terminated` after the run. `load1` was 1.13 before the first timed run and 1.04 after the last, with no process above 100% CPU. Binary: `cargo build --release --locked` at `87447255`, the code released as 0.32.0 (it reports 0.31.0: the version bump comes after it). Method: section 1, one warm-up and 7 timed runs per cell, `DISCIPLINE_NO_NETWORK=1`. Results: `results-hook-latency/release-0.32-hook-latency.json`.

**Cell added:** `hook-post-edit-enforcing`, the same payload through `hook run --agent claude-code` without `--observe`. Since `#1003` the observe-mode edit hook answers before its check runs, so `hook-post-edit` no longer times the check: it is what an agent waits for in observe mode. The enforcing cell is what it waits for when the hook can block.

Median seconds, min-max in brackets *(measured: AWS c7g.2xlarge, 87447255)*:

| Cell | `orieg/discipline` (1405 files) | `BurntSushi/ripgrep` (225 files) |
|---|---|---|
| `check-one-file` (every gate, as CI runs it) | 19.49 (19.47-19.53) | 6.28 (6.27-6.29) |
| `hook-post-edit-enforcing` (the gates an edit can trip; the agent waits) | **7.91** (7.88-7.92) | **4.28** (4.28-4.29) |
| `hook-post-edit` (observe mode; the check runs after the hook answers) | 0.02 (0.02-0.02) | 0.01 (0.01-0.01) |
| `hook-pre-tool` | 0.01 | 0.01 |

`check-one-file` exits 1 on `orieg/discipline`: the appended comment line trips a gate of this repository's configuration; the time is that of a full check either way. The enforcing figures are within about 0.5 s of section 4.3's `#994` + step 2 column, measured on an earlier commit of the same host type.

## 5. Decision it feeds

Pre-registered before any figure exists:

- A `--paths`-scoped fast path or a cache for the edit hook is written **only if** the `hook-post-edit` median on either repository is above 2 s (target), or its maximum is above 5 s (target). Below both, the per-edit cost is not the problem and no fast path is written.
- If it is written, the `suite-*` and profiler breakdown names what it skips or caches; a fast path that skips a gate the full check runs is reported by the edit hook as not checked, never as a pass.
- **Decision (applied as written):** the `hook-post-edit` median is 47.31 s on `orieg/discipline` and 9.34 s on `ripgrep` (measured), both above 2 s, and both maxima (47.35 s, 9.35 s) are above 5 s. **A fast path or cache for the edit hook is written.** The breakdown points it at the `integrity` suite first, after a profile names the gate or gates inside it.
- **After the engine fast path (`#994`, section 4.2):** `hook-post-edit` is 19.60 s here and 6.35 s on `ripgrep` (median, measured). Both medians are still above 2 s and both maxima above 5 s, so the threshold is not met. What remains is `#995`'s event scoping.
- Promoting `pretool --enforce` is not decided by latency: it is gated on refusal precision from the observe logs. `#838` supplies recall per gate.
