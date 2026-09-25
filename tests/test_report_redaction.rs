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
const SENTINEL_SLACK_TOKEN: &str = "xoxb-012345678901-0123456789012-SENTINEL_TOKEN_SECRET";
const SENTINEL_LAN_IP: &str = "192.168.1.99"; // discipline:allow(pii)

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
            rule.title(),
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
            "Host / PII Leak",
            Some("config.json"),
            Some(idx + 1),
            format!("Found a {detail}."),
            "Redact before committing.",
        );
    }
    assert_eq!(pii_outcome.violations.len(), 3);

    // Assemble summary
    let summary = CheckSummary {
        base: "origin/main".to_string(),
        errors: 7,
        warnings: 0,
        notes: 0,
        overrides: 0,
        baselined: 0,
        outcomes: vec![shell_outcome, pii_outcome],
        planned_gates: Vec::new(),
        policy_failures: Vec::new(),
        deprecations: Vec::new(),
    };

    let all_sentinels = [
        SENTINEL_AWS_KEY,
        SENTINEL_GHP_TOKEN,
        SENTINEL_PASSWORD,
        SENTINEL_HOSTNAME,
        SENTINEL_HOMEPATH,
        SENTINEL_SLACK_TOKEN,
        SENTINEL_LAN_IP,
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

    repo.commit("feat: add service configs");

    let all_sentinels = [
        SENTINEL_AWS_KEY,
        SENTINEL_GHP_TOKEN,
        SENTINEL_PASSWORD,
        SENTINEL_HOSTNAME,
        SENTINEL_HOMEPATH,
        SENTINEL_SLACK_TOKEN,
        SENTINEL_LAN_IP,
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
