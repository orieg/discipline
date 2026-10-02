# Security Policy

## Reporting a Vulnerability

Report privately through GitHub's **Security → Report a vulnerability** form on this repository:
[https://github.com/orieg/discipline/security/advisories/new](https://github.com/orieg/discipline/security/advisories/new).

That opens a private security advisory visible only to the maintainers, which is the proper channel for anything that should not be published in a public issue.

Please do not open a public issue for a suspected security vulnerability. Public issues are reserved for reproducible bugs with non-sensitive test cases, build failures, or general inquiries regarding the threat model.

A high-quality vulnerability report includes:
- The entry point (CLI argument parser, GitHub Action composite runner, TOML configuration deserializer, or tree-sitter AST fact extractor).
- The input repository, commit sequence, or file diff that triggers the unexpected behaviour.
- A minimal reproducing test case or fixture.
- Observed vs. expected behaviour.

Reports are acknowledged and triaged upon receipt. Verified fixes are published in the next release with appropriate credit unless anonymity is requested. Progress is communicated directly within the private advisory.

## Supported Versions

Security fixes land on `main` and ship with the next release. Only the latest stable released version is actively supported:

| Version | Supported |
|---|---|
| The latest release ([releases](https://github.com/orieg/discipline/releases/latest)) | Yes |
| Any earlier release | No: upgrade to the latest |

## Threat Model

This section is the vulnerability scope: what counts as a flaw in discipline itself. What discipline protects a repository against (careless and rule-evading agents, self-granted waivers, a change that loosens its own policy), its trust boundaries, and the risks it leaves to others are in [`docs/ARCHITECTURE.md` §1.4](docs/ARCHITECTURE.md#14-threat-model).

Discipline is a universal CI/CD gatekeeper and AI coding agent diff sentinel designed to execute in local developer workstations, container runtimes (Docker, Podman), and hosted CI runners (GitHub Actions, Gitea, Forgejo, GitLab CI, Argo Workflows).

Discipline compiles to a standalone static binary with no external runtime dependency. It inspects git repositories and diffs locally. Its only network access is the in-process HTTPS client in `src/forge.rs` (rustls, no OpenSSL): read-only forge API calls for the features that need them, and one opt-in write (`check --comment`). `DISCIPLINE_NO_NETWORK=1` keeps every request off the network. A forge's answer is an untrusted input like any other below.

### Untrusted Inputs (In-Scope Vulnerabilities)

A memory safety violation, panic, denial-of-service, or remote code execution triggered by any of the following is treated as a security vulnerability:

- **Git Repositories & Diffs:** Arbitrary branch names, commit hashes, author headers, commit messages, diff content, and tree objects parsed via `libgit2`. Malformed repositories or maliciously crafted packfiles must not trigger buffer overflows, uncontrolled recursion, or out-of-bounds memory access.
- **Source Code Parsed by Tree-Sitter:** Untrusted, incomplete, or deliberately adversarial source code in any supported language (Rust, Python, JavaScript, TypeScript, Go, Java, C#, C, C++, Ruby, PHP). Tree-sitter grammars and parser wrappers must fail closed or record parse errors gracefully without panics or memory corruption.
- **Configuration Files:** User-supplied `discipline.toml` files. The TOML parser must reject invalid or malicious schemas (e.g. deeply nested tables, huge integers, duplicate keys) without crashing.
- **Forge API Answers:** JSON from GitHub, GitLab, Gitea or Forgejo (pull requests, reviews, issues, comments). A malformed, partial or oversized answer must fail closed (exit 2), never read as clean, and the client never follows a redirect on a write.
- **Repository Git Configuration:** A repository's own `.git/config`, what it includes, and its hooks. Discipline reads repositories through `libgit2`, which runs none of the commands they can name. The one external `git` process, the CI base fetch, overrides hooks, the file-system monitor, command transports and the upload-pack program, and replaces a repository-set `core.sshCommand`, `credential.helper` or `core.askPass` with the runner's own. A command that runs anyway is a vulnerability.
- **Inline Directives & Markers:** Arbitrary text lines containing `discipline:allow(...)` markers. The directive parser must enforce line-anchored syntax, reject malformed strings, and disallow delimiter injection.

### Caller & Environment Contracts (Out-of-Scope)

The following operational characteristics are caller contracts, not vulnerabilities:

- **Subprocess Execution in `command` Gate:** The `command` gate executes commands explicitly declared by repository owners in `discipline.toml`. While Discipline bounds runtime with timeouts and passes arguments directly without shell string interpretation, it trusts the execution environment configured by the repository administrator.
- **Local Filesystem Permissions:** Discipline operates with the privileges of the user running the CLI binary or CI runner. Protecting the host filesystem outside the repository checkout is the responsibility of the container runtime or CI host.
- **Resource Consumption Proportional to Diff Size:** Analyzing large diffs or multi-gigabyte repositories consumes CPU and memory proportional to the number of AST nodes and files examined. Work out of proportion to the input (a file, history, configuration or forge answer built to exhaust the gate) is not this case: it is a denial of service and in scope.
- **Secret Echo Prevention:** Discipline scans files for potential leaks (PII, hostnames, passwords). Findings identify file paths and line numbers only; secret values are never printed to terminal reports or CI summaries.

## Dependencies & Supply Chain

Discipline maintains an explicit dependency allow-list and automated scan policy in `deny.toml`.

### Dependency Scanning Policy

The supply-chain scanner (`cargo-deny`) runs automatically on every pull request (in `ci.yml` job `supply-chain`) and as a mandatory gate before cutting any release (in `release.yml` job `verify`). It enforces:
- **Advisories:** Checks all dependencies against the RustSec Advisory Database. Any known security vulnerability or yanked crate fails the build immediately.
- **Licenses:** Enforces a strict allow-list (`MIT`, `Apache-2.0`, `Unicode-3.0`, `Unlicense`, `Zlib`). Any disallowed or non-conforming license fails the build.
- **Bans:** Blocks wildcard dependency versions and reports multiple version duplicates.
- **Sources:** Blocks unknown package registries and untrusted git repositories (`unknown-registry = "deny"`, `unknown-git = "deny"`).

**Threshold & Exceptions:** Any finding at `deny` severity blocks the pull request and the release verification job. Exceptions are never granted globally; they must be recorded as scoped, reasoned exception entries in `deny.toml` (e.g. `[[licenses.exceptions]]` for low-level TLS dependencies such as `ring` and `webpki-roots`).

### Vulnerability Exploitability eXchange (VEX) & Advisory Disposition

When an upstream security advisory is published for a crate in the dependency graph but the specific vulnerable codepath or mechanism does not affect Discipline:
- The determination is documented with technical justification.
- The advisory is recorded in `deny.toml` under `[advisories.ignore]` with an explicit rationale explaining why Discipline is not affected.
- Because `deny.toml` is committed to the public repository, these entries serve as the machine-verifiable VEX statement published alongside the code.
- If a broader public statement is warranted, a repository-level GitHub Security Advisory with "Not Affected" status is published.

## Static Analysis & Code Scanning Policy

Automated static analysis runs across multiple layers to catch defects before code is merged:

- **Clippy (`cargo clippy --all-targets --locked -- -D warnings`):** Runs on every pull request and push to `main` as part of the `ci-gate` rollup. Any compiler warning or linter deviation blocks the merge.
- **Discipline Sentinel:** The sentinel checks its own diffs on every pull request (`dogfood` job), enforcing fail-closed AST invariant gates against assertion reduction, vacuous tests, safety justifications on unsafe blocks, and test floor regressions.
- **CodeQL Advanced (`security-extended`):** Runs on pull requests, pushes to `main`, and on a weekly schedule (`.github/workflows/codeql.yml`) across Rust, Actions, and Python. As documented in [`docs/ARCHITECTURE.md` §8.3](docs/ARCHITECTURE.md#83-codeql-security-pipeline-githubworkflowscodeqlyml), CodeQL runs in an advisory mode so that heuristic false positives or upstream query modifications do not block unrelated merges. However, any verified true positive is classified as a blocking security defect that must be remediated before the next release.
