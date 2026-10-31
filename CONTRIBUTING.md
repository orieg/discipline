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

When adding or modifying a gate, the contribution must satisfy the complete nine-item contract of [`AGENTS.md`](AGENTS.md) §3.4 (full statement: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) §3 and §9). The list below restates it item by item, in the same order; `tests/test_docs.rs` (`contributing_restates_the_gate_contract_item_by_item`) fails when the two lists diverge.

1. **Gate Registration:** a stable kebab-case id in `src/config.rs::GATES` and its own `[gates.<id>]` table with `enabled`, `severity` and `exempt_paths`. A gate that is designed but not implemented is registered with `available: false`; it is never accepted as configuration.
2. **Truthful Outcome:** return a `GateOutcome` with a truthful `examined` count and a `notes` entry for anything the gate could not verify. Every finding kind it reports is registered in `src/findings.rs` with a kebab-case code that names what it protects; the code is frozen once released, the title is not.
3. **Tests:** a **unit test** with a positive and a negative control; an **end-to-end test** in `tests/test_gates_e2e.rs` driving the real binary; a **`self-test` case** in `src/selftest.rs`, so a deployed binary can verify its own discriminators.
4. **Mutation evidence:** break the detector, watch a test fail, restore it. State in the pull request which test killed the mutant.
5. **Escape Hatch:** any escape hatch goes through `src/tokens.rs` (line-anchored, placeholder-rejecting, scoped). No in-source override comments.
6. **Secrets:** if a finding could echo a secret or a user name, report the location only.
7. **Documentation:** `docs/GATES.md` and `README.md` list the gate with its true status.
8. **Stable Names:** the gate's new interface names (gate id, finding codes, configuration keys, directives) are recorded in `tests/fixtures/v1_surface.json` (`DISCIPLINE_BLESS_SURFACE=1 cargo test --test test_stability_contract`). A name recorded there is never removed or renamed before a major version (`docs/ARCHITECTURE.md` §3.2).
9. **Evasion corpus:** a new evasion shape, a way a change weakens a test or the run without tripping a gate, goes into `tests/fixtures/evasion_corpus.json` first, as a `known_miss` naming its issue. The change that closes it sets the case to `caught` with the exact finding codes; `tests/test_evasion_corpus.rs` fails until the verdict and the gate's "What it does NOT catch" list in `docs/GATES.md` agree with the binary.

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
