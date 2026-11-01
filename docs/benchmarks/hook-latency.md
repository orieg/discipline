# Per-edit hook latency (`#982`)

Every agent edit hook runs a full `discipline check` as a child process (`READ: src/hook.rs::run_check_under`, which runs `check --format agent-prompt --quiet --json-out`). Its cost scales with the repository and with the change against the base, not with the edit. This document holds the method, the script and the figures for that cost, and the decision the figures feed.

**Status:** method and script committed; **figures not measured** (section 4). Superseded when the figures in section 4 are filled from an idle host and the fast-path decision in section 5 is recorded on `#982`.

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

**Repositories:** this repository (887 tracked files at `2ea7a3f9`) and `BurntSushi/ripgrep` (a public Rust repository of a different size, already in the precision benchmark's evaluation set). The script records each repository's head, base, file count, commit count and the number of files the large range changes.

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

**Not measured: host not idle.** On 2026-11-01 the development host (Apple M1, 8 cores, 16 GB) was shared with other sessions building and replaying this repository: `load1` 119.38 at the first snapshot and 90.49 at the preflight, CPU 0.6% idle, 338 MB of memory unused (`RUN: uptime`, `top -l 1`), with a display utility at 95% CPU and two `discipline replay` runs and an installed `discipline check` hook each at 63–80% CPU (`RUN: ps -A -o %cpu,%mem,command`). The script refused to time on it (`RUN: python3 scripts/measure_hook_latency.py --repo .`, exit 3, `load1 29.54 > 4`).

The 60–120 s wall time reported for `check` and `diff` in the `#966` review was taken at a load around 60–70 and is not used here.

This section is filled from a run on an idle host: the markdown table the script prints, the host line from its JSON (`host`, `host_before`, `host_after`), and each figure labelled `(measured)`.

## 5. Decision it feeds

Pre-registered before any figure exists:

- A `--paths`-scoped fast path or a cache for the edit hook is written **only if** the `hook-post-edit` median on either repository is above 2 s (target), or its maximum is above 5 s (target). Below both, the per-edit cost is not the problem and no fast path is written.
- If it is written, the `suite-*` and profiler breakdown names what it skips or caches; a fast path that skips a gate the full check runs is reported by the edit hook as not checked, never as a pass.
- Promoting `pretool --enforce` is not decided by latency: it is gated on refusal precision from the observe logs. `#838` supplies recall per gate.
