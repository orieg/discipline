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

## Governance

Discipline is maintained by `orieg` as project lead and repository administrator.

- **Roles & Permissions:** The maintainer holds repository administrator privileges, manages repository settings and rulesets (`main protection`, `release tags`, `major tags`), and holds sole access to protected environment secrets and deploy keys (`release` environment deploy keys `HOMEBREW_TAP_DEPLOY_KEY` and `MAJOR_TAG_DEPLOY_KEY`, and `package-signing` environment secrets `REPO_SIGNING_KEY` and `REPO_SIGNING_PASSPHRASE` as detailed in [`docs/ARCHITECTURE.md` §8.4](docs/ARCHITECTURE.md#84-repository-settings-the-pipelines-rely-on)). Gate-weakening paths are assigned to `@orieg` in [`CODEOWNERS`](CODEOWNERS).
- **Collaborator Access:** Any future collaborator or reviewer access requires vetting before granting write or administrative privileges, adhering to the principle of least privilege. Permissions and branch ruleset review requirements (such as required approving pull request reviews) will be revisited when a second regular reviewer joins the project.
- **Decision-Making:** Architectural choices, gate additions, and contract changes follow the RFC and issue-driven consensus model documented in [`AGENTS.md`](AGENTS.md) and [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md). Suspected vulnerabilities and security disclosures follow the private reporting process documented in [`SECURITY.md`](SECURITY.md).

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

### Developer Certificate of Origin (DCO)

All contributions to Discipline must comply with the [Developer Certificate of Origin (DCO)](https://developercertificate.org/). Contributors certify that they wrote the code or have the right to submit it under the project's open source licenses (MIT OR Apache-2.0).

To certify compliance, include a `Signed-off-by` trailer in every commit message using the `-s` / `--signoff` flag:

```bash
git commit -s -S -m "type(scope): description"
```

Commits should also be cryptographically signed with `-S` per repository conventions.

- **Merge Commits:** When updating a local branch via a merge commit, ensure the merge commit also carries the signoff trailer by running `git merge --signoff` (or rebase using `git rebase --signoff`).
- **Dependabot Commits:** Automated dependency updates opened by Dependabot automatically include a `Signed-off-by: dependabot[bot] <support@github.com>` trailer in compliance with DCO policies.
- **Enforcement:** Compliance is enforced by the `commit-provenance` gate configured in `discipline.toml` (`required_trailers = ["Signed-off-by"]`). Pull requests containing commits without a valid signoff trailer are blocked by CI.

---

## Security

Do not report suspected security vulnerabilities through public issues or pull requests. Please refer to [`SECURITY.md`](SECURITY.md) for our private disclosure channel.

---

## License

By contributing to Discipline, you agree that your contributions will be licensed under the project's dual license: **MIT OR Apache-2.0**.
