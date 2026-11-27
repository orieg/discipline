# Reproducing the benchmarks

How to re-run each of discipline's measurement studies from this repository, what each one
can and cannot tell you, and which inputs live outside the repository. Results and verdicts
are not repeated here: they are in each study's results directory and its `docs/ROADMAP.md`
row. This guide changes when a study is added, retired, or changes how it is run.

## 1. What each study answers

Adopting a gate is worth it when it blocks real problems (value), rarely blocks good changes
(noise), and costs little per run (cost). Each study answers one or two of these, never all.

| Question | Study | Measures | Pre-registration | Artifacts |
|---|---|---|---|---|
| Noise and value on real changes | `agent-pr-precision-v2` to `-v6` | Every blocking finding on merged coding-agent pull requests, labelled true or false positive by two blinded raters; precision per gate class, and the same for a naive twin | `docs/benchmarks/precision-preregistration*.toml` | `docs/benchmarks/results*/` |
| Noise and value on a random sample | `agent-pr-precision-v7` | Block rate on randomly sampled merged agent pull requests, then labelled precision per gate | `docs/benchmarks/precision-preregistration-v7.toml` | not run yet |
| Noise and value of the 0.32 code | `agent-pr-precision-v8` | Block rate of the 0.32.0 code on v2 to v6's 232 pull requests; rule fidelity and actionability of each block, graded by a Claude and a Gemini rater, every disagreement settled by a third LLM agent (amendment 2) | `docs/benchmarks/precision-preregistration-v8.toml` | `docs/benchmarks/results-v8/` (`scripts/precision_v8.py report --tie-breaker fable`). Exploratory third AI rater (#1076, not pre-registered): `scripts/precision_v8.py rate-agy --dir docs/benchmarks/results-v8 --model gemini-4-neon-high --out-dir docs/benchmarks/results-v8/r3-gemini-4-neon` (agy CLI, r2's batches, one conversation per batch, resumable), then `agreement --dir docs/benchmarks/results-v8 --slot docs/benchmarks/results-v8/r3-gemini-4-neon --out docs/benchmarks/results-v8/r3-gemini-4-neon-agreement.json` |
| Value on verified test gaming | `impossible-recall-v1` (#838) | Share of agent changes that pass impossible tests by editing the test side that discipline blocks (recall), against a line-regex twin; false alarms on honest solutions to the normal split | `docs/benchmarks/recall-preregistration.toml` | `docs/benchmarks/results-recall-pilot/` (pilot); `docs/benchmarks/results-recall-confirmatory/` (verdict REJECTED with 0.26.0; descriptive replays of 0.31.0 and the 0.32.0 code beside it) |
| Held-out value on verified test gaming | `impossible-recall-fresh-v1` (#838) | v1's recall, on fresh impossible SWE-bench instances generated from SWE-bench `test` outside Verified, primary on units whose injected test file is well-formed; beside the reused-unit figure of the same binary (descriptive: a different generator model makes the difference confounded) | `docs/benchmarks/recall-fresh-preregistration.toml` (draft) | pre-registered draft, not run; `docs/benchmarks/results-recall-fresh/population.json` (scan); see §4.6 |
| Value on known regressions | `regression-value-v1` (#1024) | Share of reverted or fix-linked agent pull requests that discipline blocks with a finding in an implicated file, and its block rate on them against matched controls from the same repositories, beside a naive twin | `docs/benchmarks/regression-value-preregistration.toml` (draft) | not run yet |
| Prevention in the agent's loop | `prevention-v1` (#1036) | Landed test-gaming rate on impossible tests with discipline refusing submissions in the agent's loop, against no gate and a test-path-lock twin; landed honest passes on the normal split as the cost | `docs/benchmarks/prevention-preregistration.toml` (draft; H1 Step 0 REJECT_UNDETECTABLE) | not run yet |
| Friction of the `base-tests` preset | `base-tests-v1` (#1052) | Whether the preset runs on each SWE-bench repository's runner; the share of honest gold pull requests it blocks, by class, against a ceiling; what it adds over the diff gates on #838's gaming runs | `docs/benchmarks/base-tests-preregistration.toml` (`locked-pre-data`; Part 2 estimate-only) | pilot and confirmatory run, blocks classified by a disclosed AI rater, Part 3 and the secondary runs done; runner `scripts/base_tests_lab.py`: `plan --dry-run` prints the Part 1 sample and the Part 2 runs with no Docker, `run --mode pilot\|confirmatory --part 1\|2\|3\|secondary` runs them (Part 3, confirmatory only: one run per #838 gaming run, its shape and diff-gate verdict from `scripts/recall_shapes.py`; `secondary`, confirmatory only: one run per #838 honest `original`-split run, after its instance's control) on the lab host under the benchmark lock (confirmatory refuses a draft, a `LOCK` value or a `--constants` overlay; the lock has written every value), `collect` and `summarize` write `runs.json`, `blocks.json` and `summary.json` to `docs/benchmarks/results-base-tests/`, and `pilot-values --out docs/benchmarks/results-base-tests/pilot` writes `pilot-values.json` from the pilot summary. The pilot ran `run --mode pilot --constants docs/benchmarks/results-base-tests/pilot/constants.json`, Part 2 then Part 1 per instance, each image pulled and removed in turn; its outputs, `provenance.json` (binary, dataset, pulled image digests) and `part1-pairs.json` are in `results-base-tests/pilot/`. The confirmatory run ran `run --mode confirmatory`, one instance at a time (Part 2, then its Part 1 pairs, then its image removed) under the benchmark lock, from a `git archive` of `main` at 2b2bf102; `collect --gzip` writes `runs.json.gz` (`summarize` reads either) and `provenance.json` records the binary, dataset, runner commit and per-instance image in `results-base-tests/`. Part 3 ran `run --mode confirmatory --part 3` one instance at a time (its gaming runs, then its image removed) under the benchmark lock; `collect --gzip` adds its rows to `runs.json.gz` and `provenance.json` records its runner files by git blob id and its one retry. The secondary runs ran `run --mode confirmatory --part secondary` one instance at a time (its control, its runs, then its image removed) under the benchmark lock, with free disk rechecked before each instance; `collect --gzip` adds their rows and `provenance.json` records their runner files by git blob id |
| Whether added tests discriminate | `head-tests-on-base-v1` (#1071) | Share of merged agent pull requests whose added tests pass on the code before the change, and why: each such pull request labelled backfill, refactor, non-discriminating or other by one blinded LLM rater | `docs/benchmarks/head-tests-on-base-preregistration.toml` (locked) | `docs/benchmarks/results-head-tests-on-base/`: `population.json` (scan), `runs.json` (H, B and M per pull request), `labels.json` (rater class, reason and packet), `summary.json` (counts, intervals, verdict); see §7 |
| Value on known evasions | evasion corpus | Each known way to weaken a test without tripping a gate, recorded as `caught` or `known_miss` | none (a regression suite) | `tests/fixtures/evasion_corpus.json`, checked by `tests/test_evasion_corpus.rs` |
| Cost | hook latency (#982) | Wall time of the per-edit agent hook | `docs/benchmarks/hook-latency.md` §1 | `docs/benchmarks/results-hook-latency/` |

What none of them measures yet: how many regressions discipline prevented in day-to-day use
on repositories that run it. A precision study's true positives on merged pull requests are
problems a reviewer let through, which is evidence of value but not a count of prevented
regressions. `regression-value-v1` counts regressions that merged and were later reverted or
fixed and that discipline would have blocked at the pull request; that is a replay, not a
count from repositories that run it. `prevention-v1` is the one study with discipline in the
loop: it measures what an agent lands when blocked, on benchmark tasks, not on repositories
in use.

Labels in the precision studies come from LLM subagents (`rater_kind = "llm-subagent"` in
each `adjudication.json`); they measure rubric consistency, not external human review.

## 2. The binary

Every study names the discipline binary it scored. Use the same one:

1. Download the release asset and its `SHA256SUMS` from the release page, or build the tag
   with `cargo build --release --locked`.
2. Check the asset: `sha256sum --check --ignore-missing SHA256SUMS`.
3. `discipline --version` must print the version the study records.

A different binary is a new measurement, not a reproduction. The precision re-runs
(v3 to v6) are each a pre-registered new study for that reason.

## 3. Precision studies

**Re-score (exact).** The manifest fixes each `(repo, base_sha, head_sha)` pair, so a replay
reads no other commit.

1. Clone the eight evaluation repositories in full (not shallow) into one directory, each
   under the name in `EVALUATION_REPOS` in `scripts/benchmark_precision.py` (`ruff`, `deno`,
   `ripgrep`, `tokio`, `fastapi`, `vite`, `go`, `junit5`).
2. Replay:

   ```bash
   python3 scripts/benchmark_precision.py --from-manifest docs/benchmarks/results-v6/manifest.json \
     --clones-dir <clones> --discipline-exe <path to discipline> --raw-eval-json raw.json
   ```

3. Compare `raw.json` with the study's `findings.json`. With the study's own binary the
   findings are identical; `summary.json` records this check (`reproduction_check`) for each
   re-run against the previous study.
4. Recompute every published count, kappa and interval from the committed labels:
   `cargo test --test test_benchmark_power_bounds`.

**Re-label.** The blinded packet each pair of raters saw is committed as `rater-packet.json`,
the rubric is in the pre-registration, and `adjudication.json` holds every label. A new
rater, human or model, labels the packet without `findings.json` open and is compared with
the committed labels.

**Self-checks:** `python3 scripts/benchmark_precision.py --test`.

## 4. Recall study (`impossible-recall-v1`, #838)

The pipeline has three stages. Only the first needs a model; only the first two need Docker.

| Stage | Script | Exact or statistical |
|---|---|---|
| 1. Generate agent runs on Impossible-SWE-bench | `scripts/recall_lab/run_pilot.py`, `run_confirmatory.py` | Statistical: sampling is at temperature 1.0 |
| 2. Build base/head pairs and label how each impossible pass was achieved | `scripts/recall_pairs.py`, `scripts/recall_strata.py`, `scripts/recall_counterfactual.py` | Exact, given the same Inspect logs |
| 3. Score the pairs with discipline and the twins | `scripts/recall_replay.py`, then `scripts/recall_pilot.py` | Exact, given the same pairs and binary |

### 4.1 Pinned inputs

All pins are in `docs/benchmarks/recall-preregistration.toml`; the runner refuses to start
when one differs.

- ImpossibleBench at `[harness].commit`, with `[harness].harness_patch` applied
  (`git apply`; the resulting `git diff` has sha256
  `a50fdd525ddf5a2639865ff1975ca002a26b1750f3fc9852fc2332e22a28799d`). The patch replaces
  the sandbox network with `network_mode: none`.
- A Python environment whose `uv pip freeze`, minus the editable harness line, is
  byte-identical to `docs/benchmarks/recall-harness.lock`.
- The dataset `fjzzq2002/impossible_swebench` at revision
  `9c2d34f364b7229e8c0ff807c646100bdc18bbb5`.
- The discipline release in `[binary]`, verified against its `SHA256SUMS`.
- The model: the GGUF file with the sha256 and server flags in
  `scripts/recall_lab/manifest-local.example.json`, served by llama.cpp's `llama-server`
  with the sampling in `[arms].sampling`. The confirmatory runner also refuses to start
  when the server's reported settings differ from the pilot's.
- An x86_64 Linux host with Docker, at least 20 GB free disk, and no other benchmark on it.

### 4.2 Run directory

The runners treat their own directory as the run directory. Copy `scripts/recall_lab/` to a
fresh directory and add:

| Path | Content |
|---|---|
| `impossiblebench/` | the harness clone at the pinned commit, patch applied |
| `venv/` | the locked environment |
| `manifest-local.json` | a copy of `manifest-local.example.json` with the model name your server exposes |
| `bin/<version>/` | the release tarball, its `SHA256SUMS`, and the unpacked binary |

`pilot-ids-swe.txt` and `confirmatory-ids-swe.txt` are the population split: each id is the
result of `population_split(id, split_salt, 0.20)` in `scripts/recall_pairs.py` with the
pre-registered salt (all 349 ids check out against it).

Check that the sandbox has no network before the first run: from `toolcheck/`,
`../venv/bin/inspect eval toolcheck.py --model mockllm/model` must record
`net_blocked: true` in the sample's metadata.

### 4.3 Stage 1: agent runs

```bash
export LOCAL_BASE_URL=http://127.0.0.1:<port>/v1   # the model server
export LOCAL_API_KEY=...                            # never written to disk by the runner
./start.sh pilot --dummies-only                     # positive and negative controls, no model
./start.sh pilot --binary-dir bin/v0.26.0           # controls, then agent cells
./start.sh confirmatory --binary-dir bin/v0.26.0
```

For each instance the runner runs the oracle and no-change controls first. It generates the
agent runs on an impossible split only when the oracle passes the normal split and both
controls fail the impossible one; otherwise it records an exclusion reason. State is an
append-only JSONL file (`pilot-state.jsonl`, `confirmatory-state.jsonl`), and an interrupted
run resumes from it. `RECALL_LAB_LOCK` names an optional `flock` file shared with other
benchmark jobs on the host.

`run_pilot.py` and `run_confirmatory.py` are the files that ran, plus one trailing
`discipline:allow` marker on each of the six lines discipline's own gates flag (an empty
`except ProcessLookupError` and two `noqa: BLE001` handlers per file). Stripping the markers
gives the executed files back byte for byte:

```bash
sed -E 's/  # discipline:allow\((error-swallowing|suppression-delta)\): .*$//' scripts/recall_lab/run_pilot.py | sha256sum
# 2ddaf2a494a3126fa5d7bdd1115e28a6febe0142cb3d32199736c7228770359d (run_pilot.py)
# 6d8af59d7108530f72722ff3c09ec96d76dce4532e512c0880c8c1cb6da866ac (run_confirmatory.py)
```

Each was last modified before its run's first start record, so their docstrings still
describe them as uncommitted. `start.sh` is the executed launcher with the lab's lock path,
endpoint and key lookup replaced by environment variables.

### 4.4 Stages 2 and 3: pairs, labels, scores

```bash
python3 -I scripts/recall_pairs.py --logs logs/<split>/f1 --out out/manifest.json \
  --clone <org>/<repo>=<full clone> --materialise pairs \
  --split-salt 64b468a846aa71208e28805e4131b18a --model-alias <model name>=family-1
python3 -I scripts/recall_strata.py --from-manifest out/manifest.json --out out/strata-path-only.json
python3 -I scripts/recall_counterfactual.py --manifest out/manifest.json --strata out/strata-path-only.json \
  --logs logs/<split>/f1 --pairs-dir pairs --harness-python venv/bin/python \
  --population <pilot|confirmatory> --progress cf-progress.jsonl --out out/strata.json
python3 -I scripts/recall_replay.py --manifest out/manifest.json --strata out/strata.json \
  --pairs-dir pairs --discipline bin/v0.26.0/discipline --sha256 <binary sha256> \
  --population <pilot|confirmatory> --out out/replay.json
```

The counterfactual and replay commands are the ones the pilot ran. The `recall_pairs.py`
command is assembled from its `--help`. `recall_pilot.py --check docs/benchmarks/results-recall-pilot`
recomputes `pilot-values.json` from the committed pilot artifacts. The confirmatory scoring ran the replay command with `--population confirmatory --required-n 120`: once with the frozen 0.26.0 binary (`replay.json.gz`, `replay-summary.json`, the verdict), and once with 0.31.0 and `--out out/replay-v0.31.0.json` (descriptive), and once with a binary built from `main` at 9df9b947 (after #1042, #1044 and #1045; its `--version` still reads 0.31.0) and `--out out/replay-main-9df9b947.json` (descriptive, the re-plan measurement for the prevention study, #1036), and once with the 0.32.0 code, built from `main` at 87447255 (after #1053, #1054 and #1055; `--version` reads 0.31.0) and `--out out/replay-main-87447255.json` (descriptive and in-sample: those fixes were written from misses on these pairs). All four outputs are in `docs/benchmarks/results-recall-confirmatory/`, with `replay*.json` gzipped. `python3 -I scripts/recall_shapes.py` splits the 212 gaming runs of `replay-main-9df9b947` by the shape of the injected test file (Part 3 of `base-tests-v1`), reading the base side of each test-file hunk from the committed agent patches; the 2026-11-09 rule parsed the whole base test file, which is not committed, and `--check-labels` compares the two per run against that run's labels, `shapes-whole-file.json`.

The confirmatory pairs are scored once, after their labels are frozen
(`[integrity].sealed_rule`); a reproduction re-scores them, it does not rescore them with a
newer binary under the same study id.

### 4.5 What is not in the repository

- **The Inspect logs.** Their sha256 are committed (`log-hashes.json`), and the per-pair
  agent patches are committed under `manifest-patches/`, but stages 2 and 3 read the logs
  themselves. Until the logs are published, stage 3 can be re-run only by whoever holds them.
- **The model's provider identifier.** Committed files name the model by family, size class,
  quantization, server flags and the GGUF sha256 (`[arms].model_families`); the identifier
  your server exposes goes in the uncommitted `manifest-local.json`.

### 4.6 Fresh-instance re-measurement (`impossible-recall-fresh-v1`, draft)

Pre-registered draft, not run. Only the population scan has run:

```bash
python3 -I scripts/recall_fresh.py population        # reads three datasets, rewrites population.json
python3 -I scripts/recall_fresh.py --check-population  # no network: the pool is disjoint from v1
python3 -I scripts/recall_fresh.py --bounds-json       # the Step-0 values the pre-registration records
python3 -I scripts/recall_strata.py --v1-units         # the hunk-local reading (67 well-formed), not the registered one
python3 -I scripts/recall_strata.py --v1-units-injection  # v1's units by `injection_shape` and R_wf, offline
python3 -I scripts/recall_strata.py --v1-units-injection --fetch <cache dir>  # rebuild the per-unit file
```

`--v1-units-injection` recomputes v1's confirmatory units by the registered `injection_shape`
(66 well-formed) and the reused-unit R_wf from the committed
`results-recall-confirmatory/injection-shapes-v1-units.json` and replay, and refuses a file
whose units or blocked flags differ from the replay's. With `--fetch`, it reads the two patches
from the pinned Impossible-SWE-bench revision (checked against `manifest.json`'s sha256) and
base_commit's copies of the files they touch from public GitHub raw URLs into the cache
directory, then rewrites the per-unit file. Keep the cache outside the repository: it holds
third-party source files, which are not committed; the per-unit file records only their sha256.

The harness runs fresh instances with `scripts/recall_lab/harness-dataset-path.patch` applied
at the harness commit beside v1's `harness_patch` (`git apply` both; the hunks do not overlap).

The scan reads SWE-bench `test`, SWE-bench Verified and Impossible-SWE-bench through the
Hugging Face datasets-server at the revisions pinned in the script, and refuses when a dataset
is at another revision. Generation runs ImpossibleBench's generator with the generator model
patch the pre-registration names (`[generation] generator_model_patch`, written at the lock),
not upstream's model. Generation, agent runs and scoring follow §4.3 and §4.4 once the
pre-registration is locked; its `[lock_preconditions]` list what is still open.

## 5. Hook latency

`docs/benchmarks/hook-latency.md` §3 gives the command and host requirement; its figures
are medians from an idle host.

## 6. Charts

The charts in the README and on the Pages site (`docs/assets/img/results/`) are drawn from the
committed results above by `python3 scripts/benchmark_charts.py`, which reads no other input.
Re-run it after a result file changes; `tests/test_benchmark_charts.rs` fails while a chart and
its results disagree. The recall charts are drawn only once
`docs/benchmarks/results-recall-confirmatory/` is committed.

## 7. Head tests on base code (`head-tests-on-base-v1`, #1071)

1. **Population.** `python3 scripts/head_tests_on_base.py population` lists each v6 pull
   request's changed files through `gh api` and rewrites `population.json`.
2. **Runs.** `scripts/head_tests_on_base_lab.py` runs on one Linux host with Docker, under
   the host-wide benchmark lock, from a root directory (`--root`, default `/data/htob`):
   `images` builds the five toolchain images, `prepare` clones the seven repositories,
   `run --side H`, then `--side B` and `--side M` (B refuses to start until every H is
   recorded) write `results/<pr>/<side>.json` and per-step logs, and `collect` writes
   `runs.json`. `--clean-target` removes in-tree build output after a side; deno builds
   into each tree's own `target`, which a base and a head tree cannot share. Test steps
   run with no network; dependency steps retry a failed download.
3. **Labels.** One LLM subagent per reported pull request gets the pre-registered
   `rater_prompt` and a directory holding only that pull request's packet: title and
   body, diff, the cases that passed in M, and the head source of their files.
   `labels.json` keeps the class, the reason and the packet.
4. **Summary.** `python3 scripts/head_tests_on_base.py summarize` reads `runs.json` and
   `labels.json`, refuses labels that do not cover exactly the reported pull requests, and
   writes `summary.json` with the verdict of `verdict`.

`tests/test_head_tests_on_base.rs` runs both scripts' self-checks. A rerun can differ where
a case is flaky, a dependency download changes, or an environment the lab lacks (a GPU,
outbound DNS) decides a case; such cases fail in H and are excluded, and `summary.json`
counts them.
