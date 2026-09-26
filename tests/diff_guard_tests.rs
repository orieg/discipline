mod common;
use common::{Repo, GOOD_LIB, GOOD_TEST};

#[test]
fn safety_comment_rejects_placeholders_and_accepts_short_invariants() {
    let repo = Repo::new();
    // 1. Hollow padding with >= 4 words fails
    repo.write(
        "src/lib.rs",
        &format!("{GOOD_LIB}\npub fn hollow_safety(p: *const u8) -> u8 {{\n    // SAFETY: this is totally fine ok\n    unsafe {{ *p }}\n}}\n"),
    );
    repo.commit("feat: add unsafe with hollow padding safety comment");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1, "hollow padding safety comment must fail");
    assert_eq!(
        run.titles("unsafe-safety-comment"),
        vec!["Unsafe Without SAFETY Comment"],
        "{}",
        run.stdout
    );

    // 2. Legitimate short invariant (< 4 words) passes
    repo.write(
        "src/lib.rs",
        &format!("{GOOD_LIB}\npub fn short_invariant(p: *const u8) -> u8 {{\n    // SAFETY: caller-checked non-null.\n    unsafe {{ *p }}\n}}\n"),
    );
    repo.commit("feat: use legitimate short invariant safety comment");
    let run = repo.check(&[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(run.code, 0);
    assert!(
        run.titles("unsafe-safety-comment").is_empty(),
        "{}",
        run.stdout
    );

    // 3. Configurable placeholders in [gates.unsafe-safety-comment]
    repo.write(
        "src/lib.rs",
        &format!("{GOOD_LIB}\npub fn custom_p(p: *const u8) -> u8 {{\n    // SAFETY: custom_forbidden\n    unsafe {{ *p }}\n}}\n"),
    );
    repo.commit("feat: test configurable placeholder");
    let run_custom = repo.check(&[
        "--config-override",
        "[gates.unsafe-safety-comment]\nplaceholders = [\"custom_forbidden\"]\n",
    ]);
    assert_eq!(
        run_custom.code, 1,
        "stdout: {}, stderr: {}",
        run_custom.stdout, run_custom.stderr
    );
    assert_eq!(
        run_custom.titles("unsafe-safety-comment"),
        vec!["Unsafe Without SAFETY Comment"]
    );
}

#[test]
fn test_suppression_rejects_commented_out_test() {
    let repo = Repo::new();
    // Comment out #[test] with // #[test]
    repo.write(
        "tests/a.rs",
        &GOOD_TEST.replace("#[test]\nfn orders", "// #[test]\nfn orders"),
    );
    repo.commit("test: suppress orders test by commenting out attribute");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1, "commented-out test must fail");
    assert_eq!(
        run.titles("ignored-tests"),
        vec!["Existing Test Skipped"],
        "{}",
        run.stdout
    );

    // Directive allows suppression
    repo.write(
        "body.md",
        "Summary\n\nallow-ignore: orders temporarily disabled pending fix\n",
    );
    let run = repo.check(&["--pr-body-file", "body.md"]);
    assert!(run.titles("ignored-tests").is_empty(), "{}", run.stdout);
}

#[test]
fn test_suppression_rejects_conditional_exclusion_cfg_not_ci() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        &GOOD_TEST.replace("#[test]\nfn orders", "#[test]\n#[cfg(not(ci))]\nfn orders"),
    );
    repo.commit("test: exclude from ci via cfg");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1, "cfg(not(ci)) test suppression must fail");
    assert_eq!(
        run.titles("ignored-tests"),
        vec!["Existing Test Skipped"],
        "{}",
        run.stdout
    );
}

#[test]
fn test_suppression_rejects_conditional_exclusion_cfg_not_test() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        &GOOD_TEST.replace(
            "#[test]\nfn orders",
            "#[test]\n#[cfg(not(test))]\nfn orders",
        ),
    );
    repo.commit("test: exclude from test via cfg");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1, "cfg(not(test)) test suppression must fail");
    assert_eq!(
        run.titles("ignored-tests"),
        vec!["Existing Test Skipped"],
        "{}",
        run.stdout
    );
}

#[test]
fn vacuous_assertions_in_new_test_rejected() {
    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        &format!("{GOOD_TEST}\n#[test]\nfn vacuous_true() {{\n    assert!(true);\n}}\n"),
    );
    repo.commit("test: add vacuous assert!(true)");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    assert_eq!(
        run.titles("vacuous-tests"),
        vec!["Vacuous Test Added"],
        "{}",
        run.stdout
    );

    // Same with assert_eq!(x, x)
    repo.write(
        "tests/a.rs",
        &format!("{GOOD_TEST}\n#[test]\nfn vacuous_identity() {{\n    let x = 42;\n    assert_eq!(x, x);\n}}\n"),
    );
    repo.commit("test: add vacuous assert_eq!(x, x)");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    assert_eq!(
        run.titles("vacuous-tests"),
        vec!["Vacuous Test Added"],
        "{}",
        run.stdout
    );

    // Same with assert_eq!(1, 1)
    repo.write(
        "tests/a.rs",
        &format!("{GOOD_TEST}\n#[test]\nfn vacuous_consts() {{\n    assert_eq!(1, 1);\n}}\n"),
    );
    repo.commit("test: add vacuous assert_eq!(1, 1)");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1);
    assert_eq!(
        run.titles("vacuous-tests"),
        vec!["Vacuous Test Added"],
        "{}",
        run.stdout
    );
}

#[test]
fn cli_check_format_junit_and_output_file() {
    let repo = Repo::new();
    let out_file = repo.file("junit-report.xml");
    let run = repo.run(
        &[
            "check",
            "--base",
            "main",
            "--format",
            "junit",
            "-o",
            out_file.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(run
        .stdout
        .starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    assert!(out_file.exists());
    let content = std::fs::read_to_string(&out_file).unwrap();
    assert!(content.contains("<testsuites name=\"discipline\""));
    assert!(content.contains("<testcase name=\"agents-md\""));
    assert!(content.contains("<property name=\"examined\""));

    // Validate against vendored junit-10.xsd
    let schema_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/schemas/junit-10.xsd");
    let script_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/schemas/validate.py");
    let val = std::process::Command::new("python3")
        .args([
            script_path.to_str().unwrap(),
            "junit",
            out_file.to_str().unwrap(),
            schema_path.to_str().unwrap(),
        ])
        .output()
        .expect("validate.py succeeds");
    assert_eq!(
        val.status.code(),
        Some(0),
        "stdout: {}, stderr: {}",
        String::from_utf8_lossy(&val.stdout),
        String::from_utf8_lossy(&val.stderr)
    );

    // With a failure
    repo.write(
        "tests/a.rs",
        &format!("{GOOD_TEST}\n#[test]\nfn vacuous_fail() {{\n    assert!(true);\n}}\n"),
    );
    repo.commit("test: add vacuous assert");
    let run_fail = repo.run(
        &[
            "check",
            "--base",
            "main",
            "--format",
            "junit",
            "-o",
            out_file.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(run_fail.code, 1);
    let fail_content = std::fs::read_to_string(&out_file).unwrap();
    assert!(fail_content.contains("<failure message=\"Vacuous Test Added\""));
    assert!(fail_content.contains("cannot fail"));
    assert!(fail_content.contains("<property name=\"examined\""));

    let val_fail = std::process::Command::new("python3")
        .args([
            script_path.to_str().unwrap(),
            "junit",
            out_file.to_str().unwrap(),
            schema_path.to_str().unwrap(),
        ])
        .output()
        .expect("validate.py succeeds");
    assert_eq!(val_fail.status.code(), Some(0));
}

#[test]
fn cli_check_format_sarif_and_output_file() {
    let repo = Repo::new();
    let out_file = repo.file("sarif-report.sarif");
    let run = repo.run(
        &[
            "check",
            "--base",
            "main",
            "--format",
            "sarif",
            "-o",
            out_file.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let parsed: serde_json::Value = serde_json::from_str(&run.stdout).expect("valid SARIF json");
    assert_eq!(parsed["version"], "2.1.0");
    assert_eq!(parsed["runs"][0]["tool"]["driver"]["name"], "discipline");
    assert!(out_file.exists());
    let file_content = std::fs::read_to_string(&out_file).unwrap();
    let file_parsed: serde_json::Value =
        serde_json::from_str(&file_content).expect("valid SARIF file json");
    assert_eq!(file_parsed["version"], "2.1.0");
    assert!(file_parsed["runs"][0]["invocations"][0]["properties"]["totalExamined"].is_number());

    // Validate against vendored sarif-schema-2.1.0.json
    let schema_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/schemas/sarif-schema-2.1.0.json");
    let script_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/schemas/validate.py");
    let val = std::process::Command::new("python3")
        .args([
            script_path.to_str().unwrap(),
            "sarif",
            out_file.to_str().unwrap(),
            schema_path.to_str().unwrap(),
        ])
        .output()
        .expect("validate.py succeeds");
    assert_eq!(
        val.status.code(),
        Some(0),
        "stdout: {}, stderr: {}",
        String::from_utf8_lossy(&val.stdout),
        String::from_utf8_lossy(&val.stderr)
    );

    // With a failure
    repo.write(
        "tests/a.rs",
        &format!("{GOOD_TEST}\n#[test]\nfn vacuous_fail() {{\n    assert!(true);\n}}\n"),
    );
    repo.commit("test: add vacuous assert");
    let run_fail = repo.run(
        &[
            "check",
            "--base",
            "main",
            "--format",
            "sarif",
            "-o",
            out_file.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(run_fail.code, 1);
    let fail_parsed: serde_json::Value =
        serde_json::from_str(&run_fail.stdout).expect("valid SARIF failure json");
    assert!(!fail_parsed["runs"][0]["results"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        fail_parsed["runs"][0]["results"][0]["ruleId"],
        "vacuous-tests/vacuous-test-added"
    );

    let val_fail = std::process::Command::new("python3")
        .args([
            script_path.to_str().unwrap(),
            "sarif",
            out_file.to_str().unwrap(),
            schema_path.to_str().unwrap(),
        ])
        .output()
        .expect("validate.py succeeds");
    assert_eq!(val_fail.status.code(), Some(0));
}

#[test]
fn cli_check_format_gitlab_codequality() {
    let repo = Repo::new();
    let gl_file = repo.file("gl-custom.json");
    let run = repo.run(
        &[
            "check",
            "--base",
            "main",
            "--format",
            "gitlab",
            "-o",
            gl_file.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let parsed: Vec<serde_json::Value> =
        serde_json::from_str(&run.stdout).expect("valid code quality json");
    assert!(parsed.is_empty());
    assert!(gl_file.exists());

    // With a failure
    repo.write(
        "tests/a.rs",
        &format!("{GOOD_TEST}\n#[test]\nfn vacuous_fail() {{\n    assert!(true);\n}}\n"),
    );
    repo.commit("test: add vacuous assert");

    let explicit_report = repo.file("gl-explicit.json");
    let run_fail = repo.run(
        &[
            "check",
            "--base",
            "main",
            "--format",
            "gitlab",
            "-o",
            gl_file.to_str().unwrap(),
            "--report-gitlab",
            explicit_report.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(run_fail.code, 1);
    let fail_parsed: Vec<serde_json::Value> =
        serde_json::from_str(&run_fail.stdout).expect("valid code quality failure json");
    assert_eq!(fail_parsed.len(), 1);
    assert_eq!(
        fail_parsed[0]["check_name"],
        "vacuous-tests/vacuous-test-added"
    );
    assert!(fail_parsed[0]["description"]
        .as_str()
        .unwrap()
        .starts_with("Agent Guard:"));
    assert_eq!(fail_parsed[0]["severity"], "major");
    assert_eq!(fail_parsed[0]["location"]["path"], "tests/a.rs");
    assert_eq!(fail_parsed[0]["fingerprint"].as_str().unwrap().len(), 64);

    // Verify explicit --report-gitlab file
    assert!(explicit_report.exists());
    let exp_content = std::fs::read_to_string(&explicit_report).unwrap();
    let exp_parsed: Vec<serde_json::Value> = serde_json::from_str(&exp_content).unwrap();
    assert_eq!(exp_parsed.len(), 1);
}

#[test]
fn cli_gitlab_ci_auto_detection() {
    let repo = Repo::new();

    // Create a feature branch
    repo.git(&["checkout", "-b", "feat/mr-branch"]);
    repo.write(
        "tests/a.rs",
        &format!("{GOOD_TEST}\n#[test]\nfn vacuous_fail() {{\n    assert!(true);\n}}\n"),
    );
    repo.commit("test: add vacuous assert on branch");

    // Run WITHOUT --base flag, but with GITLAB_CI and CI_MERGE_REQUEST_TARGET_BRANCH_NAME
    let run = repo.run(
        &["check"],
        &[
            ("GITLAB_CI", "true"),
            ("CI_MERGE_REQUEST_TARGET_BRANCH_NAME", "main"),
        ],
    );
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);

    // Verify default GitLab MR widget files were automatically generated
    let gl_cq = repo.file("gl-codequality.json");
    let gl_junit = repo.file("junit.xml");
    let gl_sast = repo.file("gl-sast-report.json");

    assert!(
        gl_cq.exists(),
        "gl-codequality.json should be auto-created in GitLab CI"
    );
    assert!(
        gl_junit.exists(),
        "junit.xml should be auto-created in GitLab CI"
    );
    assert!(
        !gl_sast.exists(),
        "gl-sast-report.json should NOT be auto-created in GitLab CI"
    );

    let cq_content = std::fs::read_to_string(&gl_cq).unwrap();
    let cq_parsed: Vec<serde_json::Value> = serde_json::from_str(&cq_content).unwrap();
    assert_eq!(cq_parsed.len(), 1);
    assert_eq!(
        cq_parsed[0]["check_name"],
        "vacuous-tests/vacuous-test-added"
    );

    let junit_content = std::fs::read_to_string(&gl_junit).unwrap();
    assert!(junit_content.contains("<failure message=\"Vacuous Test Added\""));
    assert!(!junit_content.is_empty());
    assert!(!gl_sast.exists());
    assert_eq!(cq_parsed[0]["severity"], "major");
    assert_eq!(cq_parsed[0]["location"]["path"], "tests/a.rs");
    assert_eq!(cq_parsed[0]["location"]["lines"]["begin"], 15);
    assert_eq!(cq_parsed[0]["fingerprint"].as_str().unwrap().len(), 64);
    assert_eq!(run.code, 1);
}

#[test]
fn cli_forgejo_actions_auto_detection() {
    let repo = Repo::new();

    // Create a feature branch and delete tests/a.rs
    repo.git(&["checkout", "-b", "feat/forgejo-pr"]);
    repo.git(&["rm", "-q", "tests/a.rs"]);
    repo.commit("chore: remove tests/a.rs");

    // Create a mock Forgejo event JSON payload
    let event_file = repo.file("forgejo_event.json");
    std::fs::write(
        &event_file,
        r#"{"pull_request": {"title": "chore: remove tests/a.rs (#101)", "body": "removes: tests/a.rs superseded by updated test harness\nallow-test-shrink: tests/a.rs superseded by updated test harness\n"}}"#,
    )
    .unwrap();

    // 1. Without FORGEJO_EVENT_PATH, the deletion fails under FORGEJO_BASE_REF
    let run_fail = repo.run(
        &["check"],
        &[("FORGEJO_ACTIONS", "true"), ("FORGEJO_BASE_REF", "main")],
    );
    assert_eq!(run_fail.code, 1);
    assert!(run_fail.stdout.contains("File Deleted Without Rationale"));

    // 2. With FORGEJO_EVENT_PATH, the PR body directive is extracted and lifts the deletion violation
    let run_pass = repo.run(
        &["check"],
        &[
            ("FORGEJO_ACTIONS", "true"),
            ("FORGEJO_BASE_REF", "main"),
            ("FORGEJO_EVENT_PATH", event_file.to_str().unwrap()),
        ],
    );
    assert_eq!(run_pass.code, 0, "{}{}", run_pass.stdout, run_pass.stderr);
    assert!(run_pass.stdout.contains("Status: PASS"));
}

#[test]
fn cli_gitea_actions_auto_detection() {
    let repo = Repo::new();

    repo.git(&["checkout", "-b", "feat/gitea-pr"]);
    repo.git(&["rm", "-q", "tests/a.rs"]);
    repo.commit("chore: remove tests/a.rs");

    let event_file = repo.file("gitea_event.json");
    std::fs::write(
        &event_file,
        r#"{"pull_request": {"title": "chore: remove tests/a.rs (#102)", "body": "removes: tests/a.rs Gitea event justification\nallow-test-shrink: tests/a.rs Gitea event justification\n"}}"#,
    )
    .unwrap();

    let run_pass = repo.run(
        &["check"],
        &[
            ("GITEA_ACTIONS", "true"),
            ("GITEA_BASE_REF", "main"),
            ("GITEA_EVENT_PATH", event_file.to_str().unwrap()),
        ],
    );
    assert_eq!(run_pass.code, 0, "{}{}", run_pass.stdout, run_pass.stderr);
    assert!(run_pass.stdout.contains("Status: PASS"));
}
