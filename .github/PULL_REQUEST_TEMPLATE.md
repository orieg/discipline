<!--
Keep prose factual and impersonal (AGENTS.md §2.5).
No time estimates (AGENTS.md §2.1), no hostnames, LAN IPs, or local workstation paths.
-->

## What & Why

<!-- Summarise what changed and where. Reference the issue this resolves (e.g., Fixes #123). -->

## Verification Evidence (RUN)

<!-- Paste command outputs and artifact paths demonstrating verification. -->

- [ ] `cargo fmt --check`: Clean (0 diffs)
- [ ] `cargo clippy --all-targets --locked -- -D warnings`: Clean (0 warnings)
- [ ] `cargo test --locked`: 100% passing tests
- [ ] `cargo run -- self-test`: Embedded positive and negative controls pass
- [ ] `cargo run -- check --base origin/main`: Gate sentinel evaluated cleanly

## Gate Contract Checklist (if adding or changing a gate)

- [ ] Gate registered in `src/config.rs::GATES` with stable kebab-case ID and documentation summary
- [ ] Dedicated settings table in `[gates.<id>]` with `enabled`, `severity`, `exempt_paths`
- [ ] Truthful `examined` count and clear `notes` for unanalysed files
- [ ] Unit tests with positive and negative controls
- [ ] End-to-end integration test in `tests/test_gates_e2e.rs` driving the real binary
- [ ] Mutation evidence recorded (broke detector, watched test fail, restored)
- [ ] Escape hatches route through `src/tokens.rs` with line-anchored validation
- [ ] Updated canonical documentation (`docs/GATES.md`, `docs/CONFIGURATION.md`, `README.md`)
