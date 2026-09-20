# Contributing to Discipline

Discipline is a high-assurance universal CI/CD gatekeeper and AI coding agent diff sentinel built in Rust. Contributions from both human engineers and AI coding agents are welcome.

This guide outlines our engineering and quality standards so proposed changes align smoothly with repository invariants.

---

## Before Writing Code

1. **Read [`AGENTS.md`](AGENTS.md).**
   [`AGENTS.md`](AGENTS.md) is the single canonical engineering, architectural, and safety standard for this repository. It defines our core principles, the 12 Fail-Closed Invariants (F1–F12), and the testing contract.
2. **Open an issue first.**
   For non-trivial features, architectural modifications, or new gates, open an issue using the [issue templates](.github/ISSUE_TEMPLATE) before writing code. This ensures alignment on design and scope.
3. **No Time Estimates.**
   Do not include time estimates, durations, or calendar projections in issues, PRs, commit messages, code comments, or documentation ([`AGENTS.md`](AGENTS.md) §2.1). Describe ordering, dependencies, and gate criteria instead.

---

## Local Quality Gates

Every pull request must pass all four quality checks locally before submission:

```bash
# 1. Format verification
cargo fmt --check

# 2. Strict linter verification
cargo clippy --all-targets --locked -- -D warnings

# 3. Full test suite execution
cargo test --locked

# 4. Embedded self-tests
cargo run --locked -- self-test

# 5. Discipline sentinel gate check on itself
cargo run -- check --base origin/main
```

---

## Gate Contract Discipline

When adding or modifying a gate, the contribution must satisfy the complete 4-point contract ([`AGENTS.md`](AGENTS.md) §3.4 and [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) §3):

1. **Gate Registration:** Register the gate in `src/config.rs::GATES` with its kebab-case identifier, documentation summary, and configuration schema table in `[gates.<id>]`.
2. **Deterministic Outcome:** Return a truthful `GateOutcome` recording the exact number of items examined, violations detected, and notes for unanalysed files.
3. **4-Point Test Coverage:**
   - **Unit Tests:** Detector-level tests with both positive and negative controls.
   - **End-to-End Tests:** Integration test in `tests/test_gates_e2e.rs` exercising the compiled binary on throwaway git repositories.
   - **Mutation Evidence:** Deliberately break the detector, observe test failure, restore it, and document which test killed the mutant.
   - **Self-Test Case:** Embedded check in `src/selftest.rs` allowing deployed binaries to verify their own discriminators.
4. **Escape Hatch:** Any override directive must pass through `src/tokens.rs` with line-anchored, placeholder-rejecting validation.

---

## Pull Request Guidelines

- **Branches:** Branch from `main` using descriptive prefixes (`feat/…`, `fix/…`, `ci/…`, `docs/…`).
- **Commits:** Follow [Conventional Commits](https://www.conventionalcommits.org/) (`type(scope): description`). Commits should be atomic and purposeful.
- **Label Claims RUN or READ:** In pull request bodies, indicate whether behaviour was observed by executing commands (**RUN**) or inferred from source inspection (**READ**).
- **No Agent Scratch State:** Never commit scratch notes, planner files, session logs, or temporary directories ([`AGENTS.md`](AGENTS.md) §4).
- **Privacy & Host Leaks:** Never leak local paths (`/Users/...`, `/home/...`), internal hostnames, or LAN IPs.

---

## Security

Do not report suspected security vulnerabilities through public issues or pull requests. Please refer to [`SECURITY.md`](SECURITY.md) for our private disclosure channel.

---

## License

By contributing to Discipline, you agree that your contributions will be licensed under the project's dual license: **MIT OR Apache-2.0**.
