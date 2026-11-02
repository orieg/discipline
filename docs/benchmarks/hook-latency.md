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

A per-gate timer in the JSON report would replace source 3; it is not part of this change.

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
- **Not attributed within `integrity`.** The profiler step of section 2 was not run, so which of its 13 gates (`config-integrity` through `version-lockstep`) carries the 40 s is not measured. That attribution is the first step of the fast-path work below.
- **`check-large` on `orieg/discipline` exited 2** (could not check) on every run, so its 53.9 s is the cost of reaching that failure, not of a check. The 200-commit range edits protected paths (`discipline.toml`, `.github/workflows/`), so `ratified-paths` refuses to judge it with the head's policy: `gates.ratified-paths must be judged by the base ref's policy (--policy-from base)` (RUN: the same command on the development host with the `discipline 0.29.0` release, exit 2). The run still costs 53.9 s before it reports that. A large-branch figure with `--policy-from base` is not measured; `ripgrep`'s `check-large` (exit 1, a completed check) is the large-branch figure. The script now keeps the stderr of an exit-2 run (`stderr_tail_exit_2`).

`hook-pre-tool` returns in 5 ms on both repositories: it does not run a full check.

The 60–120 s wall time reported for `check` and `diff` in the `#966` review was taken at a load around 60–70 and is not used here. An earlier attempt on the development host (Apple M1, 2026-11-01) was refused by the script at `load1` 29.54 (RUN, exit 3) and timed nothing.

## 5. Decision it feeds

Pre-registered before any figure exists:

- A `--paths`-scoped fast path or a cache for the edit hook is written **only if** the `hook-post-edit` median on either repository is above 2 s (target), or its maximum is above 5 s (target). Below both, the per-edit cost is not the problem and no fast path is written.
- If it is written, the `suite-*` and profiler breakdown names what it skips or caches; a fast path that skips a gate the full check runs is reported by the edit hook as not checked, never as a pass.
- **Decision (applied as written):** the `hook-post-edit` median is 47.31 s on `orieg/discipline` and 9.34 s on `ripgrep` (measured), both above 2 s, and both maxima (47.35 s, 9.35 s) are above 5 s. **A fast path or cache for the edit hook is written.** The breakdown points it at the `integrity` suite first, after a profile names the gate or gates inside it.
- Promoting `pretool --enforce` is not decided by latency: it is gated on refusal precision from the observe logs. `#838` supplies recall per gate.
