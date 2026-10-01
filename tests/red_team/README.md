# Threat Model Verification & Red-Team Lab

This directory contains the red-team testing harness and attack corpus verifying discipline's threat model (`docs/ARCHITECTURE.md` §1.4), fail-closed invariants (§3), and security policies (`SECURITY.md`).

## Architecture & Isolation Contract

Host machines never execute untrusted attack repositories directly. All attacks are executed inside isolated, non-root, capability-dropped throwaway Docker containers on an internal network with no external internet route:

```text
 host (developer workspace: cargo, signed commits, test suite)
   │ builds Linux binary into volume `discipline-rt-target`
   ▼
 ┌──────────── docker network `discipline-rt-net` (--internal) ────────────┐
 │  rt-runner (per attack)  ──HTTP──▶  rt-gitea (gitea/gitea:1.24)         │
 │  discipline + git + curl  ──HTTP──▶  rt-forgejo (forgejo:12)            │
 │  lab-only credentials                                                   │
 └─────────────────────────────────────────────────────────────────────────┘
```

## Directory Layout

- `lab/`:
  - `up.sh`: Sets up internal Docker network `discipline-rt-net`, spins up mock forges (`rt-gitea`, `rt-forgejo`), and initializes test accounts (`owner`, `agent`, `stranger`).
  - `run.sh`: Executes a specific attack in a throwaway container (`rust:1.98` base, dropped privileges, isolated `/tmp` workspace).
- `attacks/`:
  - `cfg-01` to `cfg-03`: Configuration tampering and `policy_from: base` evasion probes.
  - `cfg-04` to `cfg-09`: `config-integrity` key forms and removals: an optional limit or switch removed (`max_noise_cv`, a gate's `allow_hidden`), a default filled in (`noise_floor_pct`), dotted and inline-table forms, a deleted table falling back to a default-off gate, a misspelt key (exit 2).
  - `esc-01` to `esc-05`: Directive escape hatch evasion probes (CRLF, Markdown formatting smuggling, placeholder reasons, commit subject vs body scoping).
  - `rat-01` to `rat-04`: Protected path owner ratification probes on live Gitea (author self-ratification, `refuse_author_ratification`, legitimate owner ratification, edited comments).
  - `sec-01`: `DISCIPLINE_NO_NETWORK=1` fail-closed verification.
  - `ast-01` to `ast-02`: AST assertion reduction and stealth test deletion probes.
- `findings.md`: Complete summary of attack results, verdicts (HOLDS / GAP / KNOWN / DOC), and remediation plans.

## Running the Lab

### 1. Build the Linux binary

```bash
docker volume create discipline-rt-target
docker run --rm --platform linux/arm64 -v "$PWD":/src:ro -v discipline-rt-target:/target \
  -e CARGO_TARGET_DIR=/target -w /src rust:1.98 cargo build --locked
```

### 2. Start the Lab Forges

```bash
bash tests/red_team/lab/up.sh
```

### 3. Execute Attack Probes

```bash
# Offline attack (no forge needed):
bash tests/red_team/lab/run.sh esc-01 none

# Forge attack on Gitea as agent:
bash tests/red_team/lab/run.sh rat-01 gitea agent
```

### 4. Automated Contract Tests

Threat model claims mapping and contract verification are integrated into the main test suite:

```bash
cargo test --test test_threat_model_claims
```
