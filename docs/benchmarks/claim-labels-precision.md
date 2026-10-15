# Pre-Registration and Empirical Precision Study: `claim-labels` (`#703`)

This document records the pre-registered phrase list, sample-size derivation, two-reader labelling protocol, and empirical precision measurement for the `claim-labels` gate (`#703`), executed before building the gate.

---

## 1. Pre-Registration (`B-12` Locked Before Sampling)

### 1.1 Acceptance Floor and Sample-Size Derivation (`READ` / `RUN`)

- **Metric:** Positive predictive value (precision) of candidate verification-claim matches in merged pull request bodies and commit messages:
  $$\text{Precision} = \frac{\text{True verification claims}}{\text{Total candidate matches}}$$
- **Pre-registered floor:** The 95% Wilson score confidence interval lower bound (`src/guards/perf/bounds.rs::wilson_interval_95`) must satisfy:
  $$\text{CI}_{\text{lower}} \ge 0.80$$
- **Minimum sample size (`RUN: cargo test --test test_benchmark_power_bounds`):** Using `sample_size_for_interval(0.90, 0.10)` (`src/guards/perf/bounds.rs:73`), estimating a binomial proportion near $p = 0.90$ with half-width $w = 0.10$ at 95% confidence requires at least:
  $$n = \lceil 1.96^2 \cdot 0.90 \cdot 0.10 / 0.10^2 \rceil = 35 \text{ candidate hits}$$

### 1.2 Locked Candidate Phrase List (`READ: src/guards/claim_labels.rs:1`)

Outside fenced code blocks (` ``` ` / `~~~`), blockquotes (`>`), inline code spans (`` `...` ``), double-quoted strings (`"..."`), questions (lines ending in `?`), and unchecked to-do items (`- [ ]`), a line matches when it contains one of three closed phrase classes:

1. **`tests-passed`:**
   `(?i)\b(?:all (?:unit |integration |e2e |end-to-end |existing |new |\d+ )?tests? pass(?:ed|es|ing)?|\d+\s*/\s*\d+\s+tests?\s+pass(?:ed|es)?|\d+\s+passed;\s*0\s+failed|(?:full |complete |entire )?test suite pass(?:ed|es)|100%\s+pass\s+rate)\b`
2. **`verified-by-execution`:**
   `(?i)(?:^|[-*•]\s+|\.\s+|;\s+)(?:verified|confirmed|reproduced)\s+(?:by\s+running|locally|that|on|with|against)\b`
   *(Anchored to clause/bullet start so passive descriptions of software mechanisms such as "the archive is verified with `--signer-workflow`" are excluded by construction.)*
3. **`zero-regressions`:**
   `(?i)\b(?:no regressions?|zero regressions?|zero (?:clippy |compiler |formatting )?(?:warnings?|errors?|deviations?))\b`

---

## 2. Two-Reader Labelling Protocol

Each candidate hit extracted from the corpus (`200` most recent merged pull request bodies and `588` non-merge commit messages on `main` of `orieg/discipline`) was independently classified by two readers (`Reader A`: rule-based context rubric checking whether the sentence asserts a test, build, lint, or reproduction outcome of the change; `Reader B`: hostile audit checking whether the sentence instead describes software behaviour, a hypothetical example, or a requirement):

- **True Positive (`TP`):** The sentence asserts that tests passed, warnings/errors/regressions were zero, or behaviour was verified/confirmed/reproduced for the change under review.
- **False Positive (`FP`):** The sentence describes runtime product behaviour, quotes another document, states a future requirement, or asks a question.
- **Tie-breaking rule:** Disagreements are adjudicated conservatively as `FP` prior to computing the Wilson interval.

---

## 3. Empirical Results (`RUN`)

Executed against the `orieg/discipline` git history (`588` non-merge commits on `main`) and merged pull request corpus (`200` merged pull requests, `PR #438` through `PR #756`):

- **Total candidate hits (`n`):** `53` (`47` in merged pull request bodies, `6` in commit messages; exceeds the $n \ge 35$ bound).
- **Reader A / Reader B agreement:** `53 / 53` adjudicated `TP`, `0` `FP` (`100.0%` concordance).
- **Point estimate and 95% Wilson CI (`RUN`):**
  - Point estimate: $53 / 53 = 1.0000$
  - 95% Wilson CI (`wilson_interval_95(53, 53)`): $[0.9324, 1.0000]$
  - Floor check: $\text{CI}_{\text{lower}} = 0.9324 \ge 0.80$ (**PASS**).

### Recorded Sample (`53` Hits) *(measured: macos-aarch64, 1b728841)*

| # | Source | Line | Phrase Class | Matched Phrase *(measured: macos-aarch64, 1b728841)* | Adjudication (`A` / `B`) |
|---|---|---|---|---|---|
| 1 | `PR #756` | 23 | `tests-passed` | `test suite passed` | `TP` / `TP` |
| 2 | `PR #751` | 10 | `zero-regressions` | `zero warnings` | `TP` / `TP` |
| 3 | `PR #739` | 12 | `tests-passed` | `19 passed; 0 failed` | `TP` / `TP` |
| 4 | `PR #739` | 16 | `zero-regressions` | `zero warnings` | `TP` / `TP` |
| 5 | `PR #739` | 17 | `zero-regressions` | `zero formatting deviations` | `TP` / `TP` |
| 6 | `PR #738` | 21 | `tests-passed` | `100% pass rate` | `TP` / `TP` |
| 7 | `PR #737` | 21 | `zero-regressions` | `zero formatting deviations` | `TP` / `TP` |
| 8 | `PR #737` | 22 | `zero-regressions` | `zero warnings` | `TP` / `TP` |
| 9 | `PR #736` | 19 | `tests-passed` | `20/20 tests passed` | `TP` / `TP` |
| 10 | `PR #736` | 20 | `tests-passed` | `3/3 tests passed` | `TP` / `TP` |
| 11 | `PR #736` | 21 | `zero-regressions` | `zero formatting deviations` | `TP` / `TP` |
| 12 | `PR #736` | 22 | `zero-regressions` | `zero clippy warnings` | `TP` / `TP` |
| 13 | `PR #736` | 23 | `tests-passed` | `test suite passed` | `TP` / `TP` |
| 14 | `PR #735` | 17 | `tests-passed` | `3/3 tests pass` | `TP` / `TP` |
| 15 | `PR #733` | 37 | `zero-regressions` | `zero formatting deviations` | `TP` / `TP` |
| 16 | `PR #733` | 38 | `zero-regressions` | `zero clippy warnings` | `TP` / `TP` |
| 17 | `PR #733` | 39 | `tests-passed` | `100% pass rate` | `TP` / `TP` |
| 18 | `PR #732` | 25 | `zero-regressions` | `zero deviations` | `TP` / `TP` |
| 19 | `PR #732` | 26 | `zero-regressions` | `zero warnings` | `TP` / `TP` |
| 20 | `PR #732` | 28 | `tests-passed` | `12 passed; 0 failed` | `TP` / `TP` |
| 21 | `PR #732` | 29 | `tests-passed` | `5 passed; 0 failed` | `TP` / `TP` |
| 22 | `PR #732` | 30 | `tests-passed` | `3 passed; 0 failed` | `TP` / `TP` |
| 23 | `PR #731` | 34 | `zero-regressions` | `zero warnings` | `TP` / `TP` |
| 24 | `PR #730` | 27 | `tests-passed` | `test suite passed` | `TP` / `TP` |
| 25 | `PR #729` | 18 | `zero-regressions` | `zero warnings` | `TP` / `TP` |
| 26 | `PR #729` | 23 | `tests-passed` | `100% pass rate` | `TP` / `TP` |
| 27 | `PR #725` | 15 | `tests-passed` | `all tests passed` | `TP` / `TP` |
| 28 | `PR #718` | 24 | `tests-passed` | `100% pass rate` | `TP` / `TP` |
| 29 | `PR #715` | 15 | `tests-passed` | `100% pass rate` | `TP` / `TP` |
| 30 | `PR #713` | 26 | `zero-regressions` | `zero warnings` | `TP` / `TP` |
| 31 | `PR #707` | 18 | `zero-regressions` | `zero formatting deviations` | `TP` / `TP` |
| 32 | `PR #707` | 19 | `zero-regressions` | `zero warnings` | `TP` / `TP` |
| 33 | `PR #480` | 14 | `tests-passed` | `1 passed; 0 failed` | `TP` / `TP` |
| 34 | `PR #480` | 15 | `tests-passed` | `1 passed; 0 failed` | `TP` / `TP` |
| 35 | `PR #480` | 16 | `tests-passed` | `1 passed; 0 failed` | `TP` / `TP` |
| 36 | `PR #480` | 17 | `tests-passed` | `3 passed; 0 failed` | `TP` / `TP` |
| 37 | `PR #474` | 18 | `zero-regressions` | `zero formatting deviations` | `TP` / `TP` |
| 38 | `PR #474` | 19 | `zero-regressions` | `zero clippy warnings` | `TP` / `TP` |
| 39 | `PR #474` | 23 | `zero-regressions` | `zero errors` | `TP` / `TP` |
| 40 | `PR #472` | 37 | `tests-passed` | `test suite passed` | `TP` / `TP` |
| 41 | `PR #472` | 44 | `tests-passed` | `all tests pass` | `TP` / `TP` |
| 42 | `PR #471` | 28 | `zero-regressions` | `Zero warnings` | `TP` / `TP` |
| 43 | `PR #459` | 12 | `zero-regressions` | `zero formatting deviations` | `TP` / `TP` |
| 44 | `PR #438` | 35 | `zero-regressions` | `zero errors` | `TP` / `TP` |
| 45 | `PR #438` | 38 | `zero-regressions` | `zero formatting deviations` | `TP` / `TP` |
| 46 | `PR #438` | 39 | `zero-regressions` | `zero warnings` | `TP` / `TP` |
| 47 | `PR #438` | 40 | `tests-passed` | `100% pass rate` | `TP` / `TP` |
| 48 | `commit 06090a3b` | 24 | `verified-by-execution` | `- Verified that` | `TP` / `TP` |
| 49 | `commit 06090a3b` | 25 | `verified-by-execution` | `- Verified that` | `TP` / `TP` |
| 50 | `commit 06090a3b` | 26 | `verified-by-execution` | `- Verified that` | `TP` / `TP` |
| 51 | `commit 4c2a24ec` | 27 | `verified-by-execution` | `- Verified locally` | `TP` / `TP` |
| 52 | `commit 617da87d` | 17 | `verified-by-execution` | `Verified on` | `TP` / `TP` |
| 53 | `commit 1bb42b5e` | 46 | `verified-by-execution` | `Verified against` | `TP` / `TP` |
