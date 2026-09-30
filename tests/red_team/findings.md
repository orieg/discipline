# Red-Team Threat Model Findings: Threat Model Claims (§1.4)

## Summary of Findings
- Total claims tested: 15
- **HOLDS**: 14
- **GAP**: 1 (Severity S1)
- **KNOWN**: 0
- **DOC**: 0

## Findings Table
| ID | Claim (short) | Source file:line | Code file:fn | Pinning test | Verdict | Severity | Evidence (command + key output lines) |
|---|---|---|---|---|---|---|---|
| `cfg-01` | PR disabling gates in `discipline.toml` is blocked under `--policy-from base` | `docs/ARCHITECTURE.md:14` | `src/guards/integrity.rs:evaluate_config_integrity` | `tests/test_gates_e2e.rs::e2e_gate_config_integrity` | HOLDS | - | `discipline check --base main --policy-from base` -> exit 1 (`config-integrity/gate-weakened`, `config-integrity/guard-gate-loosened`) |
| `cfg-02` | Disabling `config-integrity` in head without `policy-from base` is evaluated anyway | `docs/ARCHITECTURE.md:14` | `src/guards/integrity.rs:evaluate_config_integrity` | `tests/test_gates_e2e.rs::e2e_gate_config_integrity` | HOLDS | - | `discipline check --base main` -> exit 1 (`note: this change disables config-integrity; evaluated anyway because the base configuration enables it`) |
| `cfg-03` | Deleting `discipline.toml` entirely is blocked | `docs/ARCHITECTURE.md:14` | `src/guards/deletion_rationale.rs` | `tests/test_gates_e2e.rs::e2e_gate_deletion_rationale` | HOLDS | - | `discipline check --base main` -> exit 1 (`deletion-rationale/file-deleted-without-rationale`) |
| `esc-01` | Directives with CRLF line endings parse and apply cleanly | `docs/ARCHITECTURE.md:14`, `src/tokens.rs:3` | `src/tokens.rs:parse_directives_with_names` | `tests/test_threat_model_claims.rs::sec_directive_crlf_parsing` | HOLDS | - | `discipline check --base main` -> exit 0 (`override applied: removes: tests/old.rs migrating to new test suite`) |
| `esc-02` | Markdown blockquotes, fenced code, and HTML comments reject smuggled directives | `src/tokens.rs:4-6` | `src/tokens.rs:parse_directives_with_names` | `tests/test_threat_model_claims.rs::sec_directive_markdown_smuggling_defense` | HOLDS | - | Blockquote exit 1, Fenced exit 1, HTML comment exit 1 (`hidden directive ... in PR body ignored`) |
| `esc-03` | Directive reason must be non-empty and not a placeholder | `src/tokens.rs:7`, `docs/GATES.md` | `src/tokens.rs:parse_directives_with_names` | `tests/test_threat_model_claims.rs::sec_directive_placeholder_gap_proof` | **GAP** | **S1** | `removes: tests/old.rs` (no reason) exits 0; `removes: tests/old.rs todo` exits 0 because subject is included in `caps[3]` and bypasses `is_placeholder` check |
| `esc-04` | Directives in commit subject line (line 0) are ignored | `docs/ARCHITECTURE.md:14`, `AGENTS.md:4` | `src/tokens.rs:parse_directives_with_names` | `tests/test_threat_model_claims.rs::sec_directive_subject_line_ignored` | HOLDS | - | `discipline check --base main` -> exit 1 (`deletion-rationale/file-deleted-without-rationale`) |
| `esc-05` | Directives in PR title are not accepted as overrides | `docs/ARCHITECTURE.md:14`, `AGENTS.md:4` | `src/tokens.rs` | `tests/test_gates_e2e.rs` | HOLDS | - | `discipline check --base main --pr-title ...` -> exit 1 (`deletion-rationale/file-deleted-without-rationale`) |
| `rat-01` | PR author commenting on closing issue is rejected if not in `ratifiers` / in `agent_logins` | `docs/ARCHITECTURE.md:14` | `src/ratification.rs:refuse_comment` | `tests/test_pr_policy.rs::agent_logins_cannot_ratify_even_if_in_ratifiers` | HOLDS | - | `discipline check` against live Gitea -> exit 1 (`note [ratified-paths] Ratification Author Not Accepted ... is by agent`) |
| `rat-02` | Author self-ratification refused even if author is in `ratifiers` | `docs/ARCHITECTURE.md:14`, `src/ratification.rs:583` | `src/ratification.rs:refuse_comment` | `tests/test_pr_policy.rs::ratified_paths_refuse_author_ratification_rejects_author` | HOLDS | - | `discipline check` against live Gitea -> exit 1 (`note [ratified-paths] Ratification By The Pull Request's Author`) |
| `rat-03` | Legitimate owner ratification in closing issue comment accepted | `docs/ARCHITECTURE.md:14`, `docs/GATES.md` | `src/ratification.rs:judge` | `tests/test_pr_policy.rs::ratified_paths_owner_comment_ratifies` | HOLDS | - | `discipline check` against live Gitea -> exit 0 (`protected.txt ratified in issue:owner/rat-test-3#1/comment:7 by owner`) |
| `rat-04` | Comment edited after posting is rejected on Gitea | `docs/ARCHITECTURE.md:14` | `src/ratification.rs:refuse_comment` | `tests/test_pr_policy.rs::ratified_paths_edited_comment_refused` | HOLDS | - | `discipline check` against live Gitea -> exit 1 (`note [ratified-paths] Ratification Comment Not Accepted: it was edited after it was posted`) |
| `sec-01` | `DISCIPLINE_NO_NETWORK=1` keeps every request off network and fails closed with exit 2 | `docs/ARCHITECTURE.md:3` (F2/F12), `AGENTS.md:3.3` | `src/forge.rs:HttpApi::fetch` | `tests/test_pr_policy.rs::no_network_flag_prevents_calls` | HOLDS | - | `DISCIPLINE_NO_NETWORK=1 discipline check` -> exit 2 (`forge-unavailable: network access is disabled`) |
| `ast-01` | Assertion reduction detected in existing test and lifted by scoped directive | `docs/GATES.md` | `src/guards/agent_diff.rs` | `tests/test_gates_e2e.rs::e2e_gate_assertion_reduction` | HOLDS | - | Without directive exit 1 (`assertion-reduction/assertion-count-decreased`); with directive exit 0 |
| `ast-02` | Stealth test function deletion detected by both deletion-rationale and test-floor | `docs/GATES.md` | `src/guards/deletion_rationale.rs`, `src/guards/test_floor.rs` | `tests/test_gates_e2e.rs::e2e_gate_deletion_rationale` | HOLDS | - | Stealth test removal triggers exit 1 on both `deletion-rationale` and `test-floor` |

---

## Detailed GAP Analysis

### GAP 1: Directive Reason Placeholder & Empty Rationale Bypass (`esc-03`)
- **Severity**: **S1** (Silent merge of eroded code / waiver granted without valid rationale)
- **Reproduction**: `$SCRATCH/attacks/esc-03/run.sh`
- **Root Cause**:
  In `src/tokens.rs::parse_directives_with_names` (lines 862-872):
  ```rust
  if let Some(caps) = re.captures(line) {
      let reason = clean_reason(&caps[3]);
      if !is_placeholder(&reason) { ... }
  }
  ```
  And in `src/tokens.rs::reason_names` (lines 1184-1193):
  ```rust
  let unquoted_reason = reason.trim_start_matches(['"', '\'', '`']);
  if let Some(rest) = unquoted_reason.strip_prefix(trimmed_subject) {
      if rest.is_empty()
          || rest.starts_with(|c: char| (!is_token_char(c) && c != ':') || c == '@')
      {
          return true;
      }
  }
  ```
  When an adversary writes `removes: tests/old.rs` (no reason given) or `removes: tests/old.rs todo` (placeholder given after subject):
  1. `caps[3]` contains `"tests/old.rs"` or `"tests/old.rs todo"`.
  2. `is_placeholder(&reason)` checks if the entire string is one of `PLACEHOLDERS` (`todo`, `tbd`, etc.). Because the subject is prefixed, it does not match.
  3. `reason_names` strips `trimmed_subject`, leaving `rest` either empty (`""`) or with a placeholder (`" todo"`).
  4. Because `rest.is_empty()` returns `true`, the override is granted without requiring an actual non-placeholder rationale.
- **Proposed Fix**:
  When extracting directives that require a subject and a rationale, or when checking whether `rest` after stripping `trimmed_subject` is valid:
  - If `rest.trim()` is empty, the directive must NOT cover the subject (an explicit rationale is mandatory per `docs/GATES.md`).
  - `is_placeholder(rest.trim())` must be checked on the remaining rationale text after stripping the subject.
- **Protected Path Impact**:
  Touches `src/tokens.rs` only. Does NOT touch any protected configuration paths (`discipline.toml`, `AGENTS.md`, etc.). Can be fixed in a standard remediation PR accompanied by the unit test `sec_directive_placeholder_gap_proof` in `tests/test_threat_model_claims.rs`.
