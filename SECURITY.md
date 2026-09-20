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

| Version | Supported | Notes |
|---|---|---|
| `0.4.x` | Yes | Current stable release series |
| `0.3.x` | Yes | Maintenance support |
| `< 0.3.0` | No | Upgrade to `0.3.x` or later |

## Threat Model

Discipline is a universal CI/CD gatekeeper and AI coding agent diff sentinel designed to execute in local developer workstations, container runtimes (Docker, Podman), and hosted CI runners (GitHub Actions, Gitea, Forgejo, GitLab CI, Argo Workflows).

Discipline compiles to a standalone static binary with zero external runtime dependencies and **no network connectivity features** (it does not link OpenSSL, TLS, or HTTP clients). It inspects git repositories and diffs locally.

### Untrusted Inputs (In-Scope Vulnerabilities)

A memory safety violation, panic, denial-of-service, or remote code execution triggered by any of the following is treated as a security vulnerability:

- **Git Repositories & Diffs:** Arbitrary branch names, commit hashes, author headers, commit messages, diff content, and tree objects parsed via `libgit2`. Malformed repositories or maliciously crafted packfiles must not trigger buffer overflows, uncontrolled recursion, or out-of-bounds memory access.
- **Source Code Parsed by Tree-Sitter:** Untrusted, incomplete, or deliberately adversarial source code in any supported language (Rust, Python, JavaScript, TypeScript, Go, Java, C#, C, C++, Ruby, PHP). Tree-sitter grammars and parser wrappers must fail closed or record parse errors gracefully without panics or memory corruption.
- **Configuration Files:** User-supplied `discipline.toml` files. The TOML parser must reject invalid or malicious schemas (e.g. deeply nested tables, huge integers, duplicate keys) without crashing.
- **Inline Directives & Markers:** Arbitrary text lines containing `discipline:allow(...)` markers. The directive parser must enforce line-anchored syntax, reject malformed strings, and disallow delimiter injection.

### Caller & Environment Contracts (Out-of-Scope)

The following operational characteristics are caller contracts, not vulnerabilities:

- **Subprocess Execution in `command` Gate:** The `command` gate executes commands explicitly declared by repository owners in `discipline.toml`. While Discipline bounds runtime with timeouts and passes arguments directly without shell string interpretation, it trusts the execution environment configured by the repository administrator.
- **Local Filesystem Permissions:** Discipline operates with the privileges of the user running the CLI binary or CI runner. Protecting the host filesystem outside the repository checkout is the responsibility of the container runtime or CI host.
- **Resource Consumption Proportional to Diff Size:** Analyzing large diffs or multi-gigabyte repositories consumes CPU and memory proportional to the number of AST nodes and files examined.
- **Secret Echo Prevention:** Discipline scans files for potential leaks (PII, hostnames, passwords). Findings identify file paths and line numbers only; secret values are never printed to terminal reports or CI summaries.

## Dependencies & Supply Chain

Discipline maintains an explicit dependency allow-list in `deny.toml`. Every dependency is vetted against supply-chain advisories, non-permissive licenses, and wildcard requirements. Dependency trees are audited automatically on every pull request.
