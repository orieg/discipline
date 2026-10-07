//! Cross-format report redaction integration test.
//!
//! Asserts the core security invariant: findings produced by secret, credential,
//! or PII detection NEVER echo the underlying matched sentinel text in ANY rendered
//! report format (`Terminal`, `GithubSummary`, `Json`, `Junit`, `Sarif`, `Gitlab`, `AgentPrompt`).
//!
//! Table-driven over `OutputFormat::value_variants()` with compile-time exhaustive match
//! to ensure any newly added format fails until explicitly covered.

mod common;
use clap::ValueEnum;
use common::Repo;
use discipline::cli::OutputFormat;
use discipline::config::{PiiGate, Severity, ShellSecretsGate};
use discipline::guards::hygiene::pii_rules;
use discipline::guards::shell_secrets::ShellSecretScanner;
use discipline::guards::{CheckSummary, GateOutcome};
use discipline::report::format_report_content;

const SENTINEL_AWS_KEY: &str = "AKIAIOSFODNN7EXAMPLE_SECRET_SENTINEL";
const SENTINEL_GHP_TOKEN: &str = "ghp_0123456789abcdef0123456789abcdef_SENTINEL"; // discipline:allow(pii)
const SENTINEL_PASSWORD: &str = "super_secret_password_sentinel_xyz123";
const SENTINEL_HOSTNAME: &str = "internal-production-vault.corp.sentinel";
const SENTINEL_HOMEPATH: &str = "/Users/secretdeveloperuser_sentinel/projects"; // discipline:allow(pii)
const SENTINEL_SLACK_TOKEN: &str =
    concat!("xoxb", "-012345678901-0123456789012-SENTINEL_TOKEN_SECRET");
const SENTINEL_LAN_IP: &str = "192.168.1.99"; // discipline:allow(pii)

// The fixed-format classes `pii` shares with `shell-secrets`. Assembled at compile time so
// this file carries no token-shaped literal.
const SENTINEL_LLM_KEY: &str = concat!("sk", "-", "SENTINELllmkey0123456789abc");
const SENTINEL_FINE_GRAINED: &str = concat!(
    "github_pat",
    "_",
    "SENTINELfinegrained0123456789",
    "SENTINELfinegrained0123456789",
    "SENTINELfinegrained01234"
);
const SENTINEL_ABIA_KEY: &str = concat!("ABIA", "SENTINEL01234567");
const SENTINEL_BEARER: &str = "SENTINELbearer0123456789";
const SENTINEL_PRIVATE_KEY_HEADER: &str =
    concat!("-----", "BEGIN SENTINELPGP PRIVATE KEY", "-----");

/// Compile-time check ensuring every variant of `OutputFormat` is handled.
/// Adding an 8th format will cause this function to fail to compile.
fn assert_format_exhaustiveness(format: OutputFormat) {
    match format {
        OutputFormat::Terminal => {}
        OutputFormat::GithubSummary => {}
        OutputFormat::Json => {}
        OutputFormat::Junit => {}
        OutputFormat::Sarif => {}
        OutputFormat::Gitlab => {}
        OutputFormat::AgentPrompt => {}
    }
}

#[test]
fn test_cross_format_redaction_pins_sentinel_exclusion() {
    let mut shell_outcome = GateOutcome::new("shell-secrets");
    let scanner = ShellSecretScanner::new(&ShellSecretsGate::default()).unwrap();

    // 1. Scan lines with known sentinels using ShellSecretScanner
    let shell_lines = [
        format!("export AWS_ACCESS_KEY_ID={SENTINEL_AWS_KEY}"),
        format!("export GITHUB_TOKEN={SENTINEL_GHP_TOKEN}"),
        format!("SLACK_API_TOKEN={SENTINEL_SLACK_TOKEN}"),
        format!("mysql -u root -p{SENTINEL_PASSWORD} -h localhost"),
    ];

    for (idx, line) in shell_lines.iter().enumerate() {
        let rule = scanner.check_line(line).unwrap_or_else(|| {
            panic!("expected shell rule hit on line: {line}");
        });
        shell_outcome.push(
            Severity::Error,
            rule.kind(),
            Some("deploy.sh"),
            Some(idx + 1),
            rule.message().to_string(),
            rule.remediation(),
        );
    }
    assert_eq!(shell_outcome.violations.len(), 4);

    // 2. Scan lines with PII sentinels
    let mut pii_settings = PiiGate::default();
    pii_settings
        .hostname_denylist
        .push(SENTINEL_HOSTNAME.to_string());
    pii_settings.redact_lan_ips = true;
    let rules = pii_rules(&pii_settings).unwrap();

    let mut pii_outcome = GateOutcome::new("pii");
    let pii_samples = [
        (
            format!("Path: {SENTINEL_HOMEPATH}/data.bin"),
            "home-directory path",
        ),
        (
            format!("Host: https://{SENTINEL_HOSTNAME}/api"),
            "denylisted hostname",
        ),
        (
            format!("Target LAN IP: {SENTINEL_LAN_IP}"),
            "private LAN address",
        ),
        (
            format!("key = {SENTINEL_LLM_KEY}"),
            "OpenAI or Anthropic API key",
        ),
        (
            format!("token = \"{SENTINEL_FINE_GRAINED}\""),
            "GitHub token",
        ),
        (format!("id = {SENTINEL_ABIA_KEY}"), "AWS access key ID"),
        (
            format!("Authorization: Bearer {SENTINEL_BEARER}"),
            "literal Authorization Bearer token",
        ),
        (
            SENTINEL_PRIVATE_KEY_HEADER.to_string(),
            "private key header",
        ),
    ];

    for (idx, (line, expected_rule)) in pii_samples.iter().enumerate() {
        let rule = rules
            .iter()
            .find(|r| r.label == *expected_rule && r.re.is_match(line))
            .unwrap_or_else(|| panic!("expected PII rule hit for {expected_rule} in: {line}"));

        let detail = match (rule.redact, true) {
            (false, _) => format!("{} `[raw]`", rule.label),
            _ => format!("{} (match not echoed)", rule.label),
        };
        pii_outcome.push(
            Severity::Error,
            &discipline::findings::HOST_OR_PII_LEAK,
            Some("config.json"),
            Some(idx + 1),
            format!("Found a {detail}."),
            "Redact before committing.",
        );
    }
    assert_eq!(pii_outcome.violations.len(), 8);

    // Assemble summary
    let summary = CheckSummary {
        schema_version: discipline::output_schema::REPORT_SCHEMA_VERSION,
        could_not_check: None,
        base: "origin/main".to_string(),
        errors: 12,
        warnings: 0,
        notes: 0,
        overrides: 0,
        baselined: 0,
        outcomes: vec![shell_outcome, pii_outcome],
        planned_gates: Vec::new(),
        policy_failures: Vec::new(),
        refused_hidden_directives: Vec::new(),
        deprecations: Vec::new(),
        directive_notes: Vec::new(),
        unused_directives: Vec::new(),
    };

    let all_sentinels = [
        SENTINEL_AWS_KEY,
        SENTINEL_GHP_TOKEN,
        SENTINEL_PASSWORD,
        SENTINEL_HOSTNAME,
        SENTINEL_HOMEPATH,
        SENTINEL_SLACK_TOKEN,
        SENTINEL_LAN_IP,
        SENTINEL_LLM_KEY,
        SENTINEL_FINE_GRAINED,
        SENTINEL_ABIA_KEY,
        SENTINEL_BEARER,
        SENTINEL_PRIVATE_KEY_HEADER,
    ];

    // Assert that EVERY OutputFormat excludes ALL sentinels
    for format in OutputFormat::value_variants() {
        assert_format_exhaustiveness(*format);

        let rendered = format_report_content(&summary, *format, false, false)
            .unwrap_or_else(|e| panic!("failed to render report format {:?}: {e:#}", format));

        for sentinel in all_sentinels {
            assert!(
                !rendered.contains(sentinel),
                "SECURITY VIOLATION: sentinel `{sentinel}` was leaked in output format `{:?}`!\nRendered output:\n{}",
                format,
                rendered
            );
        }
    }
}

#[test]
fn test_live_repository_cross_format_redaction_pins_sentinel_exclusion() {
    let repo = Repo::new();

    // 0. Enable full LAN IP redaction in config
    repo.write("discipline.toml", "[gates.pii]\nredact_lan_ips = true\n");

    // 1. Introduce shell secrets in a shell script
    repo.write(
        "scripts/deploy.sh",
        &format!(
            "#!/usr/bin/env bash\nexport AWS_ACCESS_KEY_ID={SENTINEL_AWS_KEY}\nexport GITHUB_TOKEN={SENTINEL_GHP_TOKEN}\nSLACK_API_TOKEN={SENTINEL_SLACK_TOKEN}\nmysql -u root -p{SENTINEL_PASSWORD} -h localhost\n"
        ),
    );

    // 2. Introduce PII in a config file
    repo.write(
        "config/settings.json",
        &format!(
            "{{\n  \"home\": \"{SENTINEL_HOMEPATH}/data.bin\",\n  \"vault\": \"https://{SENTINEL_HOSTNAME}/api\",\n  \"lan\": \"{SENTINEL_LAN_IP}\"\n}}\n"
        ),
    );

    // 3. Fixed-format tokens in files `shell-secrets` never reads (markdown, Python,
    // YAML, plain text): `pii` is the only gate that sees them.
    repo.write(
        "docs/setup.md",
        &format!("Set the key to {SENTINEL_LLM_KEY} first.\n"),
    );
    repo.write(
        "src/client.py",
        &format!("TOKEN = \"{SENTINEL_FINE_GRAINED}\"\n"),
    );
    repo.write("notes/ids.txt", &format!("id {SENTINEL_ABIA_KEY}\n"));
    repo.write(
        "keys/header.txt",
        &format!("{SENTINEL_PRIVATE_KEY_HEADER}\n"),
    );
    repo.write(
        "config/api.yaml",
        &format!("Authorization: Bearer {SENTINEL_BEARER}\n"),
    );

    repo.commit("feat: add service configs");

    let all_sentinels = [
        SENTINEL_AWS_KEY,
        SENTINEL_GHP_TOKEN,
        SENTINEL_PASSWORD,
        SENTINEL_HOSTNAME,
        SENTINEL_HOMEPATH,
        SENTINEL_SLACK_TOKEN,
        SENTINEL_LAN_IP,
        SENTINEL_LLM_KEY,
        SENTINEL_FINE_GRAINED,
        SENTINEL_ABIA_KEY,
        SENTINEL_BEARER,
        SENTINEL_PRIVATE_KEY_HEADER,
    ];

    let formats = [
        ("terminal", OutputFormat::Terminal),
        ("github-summary", OutputFormat::GithubSummary),
        ("json", OutputFormat::Json),
        ("junit", OutputFormat::Junit),
        ("sarif", OutputFormat::Sarif),
        ("gitlab", OutputFormat::Gitlab),
        ("agent-prompt", OutputFormat::AgentPrompt),
    ];

    for (fmt_str, fmt_variant) in formats {
        assert_format_exhaustiveness(fmt_variant);

        let run = repo.run(
            &["check", "--format", fmt_str, "--base", "main"],
            &[("DISCIPLINE_HOSTNAME_DENYLIST", SENTINEL_HOSTNAME)],
        );

        // Violations must cause non-zero exit (errors present)
        assert_eq!(
            run.code, 1,
            "format `{fmt_str}` must fail due to secrets/pii violations\nstdout:\n{}\nstderr:\n{}",
            run.stdout, run.stderr
        );

        if fmt_str == "json" {
            // `pii` reports each file `shell-secrets` never reads, by location and class.
            let report: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
            let outcomes = report["outcomes"].as_array().unwrap();
            let pii = outcomes.iter().find(|o| o["gate"] == "pii").unwrap();
            let files: Vec<&str> = pii["violations"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v["file"].as_str().unwrap())
                .collect();
            for expected in [
                "docs/setup.md",
                "src/client.py",
                "notes/ids.txt",
                "config/api.yaml",
                "keys/header.txt",
            ] {
                assert!(files.contains(&expected), "{expected} not in {files:?}");
            }
        }

        for sentinel in all_sentinels {
            assert!(
                !run.stdout.contains(sentinel),
                "SECURITY VIOLATION: sentinel `{sentinel}` was leaked in stdout for format `{fmt_str}`!\nStdout:\n{}",
                run.stdout
            );
            assert!(
                !run.stderr.contains(sentinel),
                "SECURITY VIOLATION: sentinel `{sentinel}` was leaked in stderr for format `{fmt_str}`!\nStderr:\n{}",
                run.stderr
            );
        }
    }
}

/// A policy refusal's entry in the SARIF, JUnit and GitLab reports is a rule id, a place
/// and fixed wording (`src/refusals.rs`). The sentence the terminal and the JSON report
/// print can quote a reviewer's login and a commit; a hidden directive that was not read
/// has a name, a subject and a reason. None of it reaches those three reports, and the
/// terminal and JSON reports say what they said before.
#[test]
fn a_policy_refusal_reaches_sarif_junit_and_gitlab_without_the_text_it_quotes() {
    use discipline::refusals::{PolicyFailure, RefusalKind};
    use discipline::tokens::OverrideSource;
    const QUOTED: &str = "REFUSAL_SENTENCE_SENTINEL";
    let summary = CheckSummary {
        schema_version: discipline::output_schema::REPORT_SCHEMA_VERSION,
        could_not_check: None,
        base: "origin/main".to_string(),
        errors: 0,
        warnings: 0,
        notes: 0,
        overrides: 0,
        baselined: 0,
        outcomes: vec![GateOutcome::new("agents-md")],
        planned_gates: Vec::new(),
        policy_failures: vec![
            PolicyFailure::new(RefusalKind::MaxOverrides, QUOTED),
            PolicyFailure::new(RefusalKind::MaxInlineOverrides, QUOTED),
            PolicyFailure::new(RefusalKind::ApprovalRequired, QUOTED),
        ],
        refused_hidden_directives: vec![OverrideSource::PrBody],
        deprecations: Vec::new(),
        directive_notes: Vec::new(),
        unused_directives: Vec::new(),
    };
    for format in OutputFormat::value_variants() {
        assert_format_exhaustiveness(*format);
        let rendered = format_report_content(&summary, *format, false, false).unwrap();
        let quotes = rendered.contains(QUOTED);
        match format {
            OutputFormat::Junit | OutputFormat::Sarif | OutputFormat::Gitlab => {
                assert!(
                    !quotes,
                    "{format:?} quotes a refusal's sentence:\n{rendered}"
                );
                for code in [
                    "max-overrides-exceeded",
                    "max-inline-overrides-exceeded",
                    "approval-required",
                    "hidden-directive-refused",
                ] {
                    assert!(
                        rendered.contains(code),
                        "{format:?} lacks {code}:\n{rendered}"
                    );
                }
            }
            OutputFormat::Terminal | OutputFormat::GithubSummary | OutputFormat::Json => {
                assert!(
                    quotes,
                    "{format:?} no longer prints the refusal:\n{rendered}"
                );
            }
            // The agent prompt lists findings to repair; it never carried a refusal.
            OutputFormat::AgentPrompt => assert!(!quotes, "{rendered}"),
        }
    }
}
