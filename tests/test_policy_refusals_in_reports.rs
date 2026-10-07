//! Policy refusals in the SARIF, JUnit and GitLab code-quality reports.
//!
//! A refusal is a decision of the run's policy that no gate owns: an override budget
//! that is exceeded, directive overrides without the approval the policy requires, a
//! hidden directive that was not read. The JSON report is the canonical record
//! (`policy_failures`, and a gate note for a hidden directive); the three reports here
//! carry one entry per refusal with a rule id, a location and fixed wording, and never
//! the text of a directive (`docs/ARCHITECTURE.md` §7).
//!
//! Each test drives the built binary once per format over one repository.

mod common;
use common::{FakeForge, Repo, Run, CONFIG_HEAD};
use discipline::cli::OutputFormat;
use discipline::guards::{CheckSummary, GateOutcome};
use discipline::refusals::{PolicyFailure, RefusalKind, REFUSALS};
use discipline::report::format_report_content;
use discipline::tokens::OverrideSource;
use serde_json::Value;

/// Text placed in a directive's subject and reason. No report entry for a refusal may
/// carry it.
const SENTINEL: &str = "REFUSAL-SENTINEL-481";

/// A repository and the environment its runs need.
struct Scenario {
    repo: Repo,
    env: Vec<(&'static str, String)>,
    /// Held so the fake forge and the event payload outlive the runs.
    _forge: Option<FakeForge>,
    _dir: Option<tempfile::TempDir>,
}

impl Scenario {
    fn plain(repo: Repo) -> Self {
        Scenario {
            repo,
            env: Vec::new(),
            _forge: None,
            _dir: None,
        }
    }

    fn run(&self, format: &str) -> Run {
        let env: Vec<(&str, &str)> = self.env.iter().map(|(k, v)| (*k, v.as_str())).collect();
        self.repo.run(
            &[
                "check",
                "--base",
                "main",
                "--policy-from",
                "base",
                "--format",
                format,
            ],
            &env,
        )
    }
}

fn repo_with_base_config(base_cfg: &str) -> Repo {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", base_cfg);
    repo.commit("chore: config");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo
}

/// A change that disables two gates and excuses both from its commit body; the reasons
/// carry the sentinel.
fn two_directive_overrides(directives: &str) -> Repo {
    let repo = repo_with_base_config(&format!("{CONFIG_HEAD}[directives]\n{directives}"));
    repo.write(
        "discipline.toml",
        &format!(
            "{CONFIG_HEAD}[directives]\n{directives}\
             [gates.vacuous-tests]\nenabled = false\n[gates.pii]\nenabled = false\n"
        ),
    );
    repo.commit(&format!(
        "chore: tune\n\nallow-gate-weakening: vacuous-tests {SENTINEL} macros assert for us\n\
         allow-gate-weakening: pii {SENTINEL} fixtures carry documentation addresses"
    ));
    repo
}

fn max_overrides() -> Scenario {
    Scenario::plain(two_directive_overrides("max_overrides = 1\n"))
}

fn max_inline_overrides() -> Scenario {
    let repo = repo_with_base_config(&format!(
        "{CONFIG_HEAD}[directives]\nmax_inline_overrides = 1\n"
    ));
    repo.write(
        "docs/plan.md",
        "Phase 2 (1 week). <!-- discipline:allow(time-estimates) -->\nPhase 3 (2 weeks). <!-- discipline:allow(time-estimates) -->\n",
    );
    repo.commit("docs: two inline overrides");
    Scenario::plain(repo)
}

/// Two directive overrides under `require_approval`, and a forge that has no review.
fn approval_required() -> Scenario {
    let repo = two_directive_overrides(
        "require_approval = true\nallowed_override_actors = [\"lead-481\"]\n",
    );
    let dir = tempfile::tempdir().unwrap();
    let event = dir.path().join("event.json");
    std::fs::write(
        &event,
        r#"{"pull_request": {"number": 7, "user": {"login": "author-481"}, "head": {"sha": "abc123"}}}"#,
    )
    .unwrap();
    let forge = FakeForge::start();
    forge.serve(
        "repos/o/r/pulls/7/reviews?per_page=100",
        serde_json::json!([]),
    );
    let env = vec![
        ("DISCIPLINE_FORGE_API_URL", forge.url()),
        ("GITHUB_REPOSITORY", "o/r".to_string()),
        ("GITHUB_EVENT_PATH", event.to_str().unwrap().to_string()),
    ];
    Scenario {
        repo,
        env,
        _forge: Some(forge),
        _dir: Some(dir),
    }
}

/// Two hidden directives in the pull request body and one in a commit body, none read.
fn hidden_directives() -> Scenario {
    let repo = repo_with_base_config(CONFIG_HEAD);
    repo.write("docs/plan.md", "# Plan\n\nPhase 1 then Phase 2.\nx\n");
    repo.commit(&format!(
        "docs: wording\n\n<!-- no-issue: {SENTINEL} commit -->"
    ));
    let body = format!(
        "Fix.\n<!-- no-issue: {SENTINEL} body -->\n<!-- allow-assertion-drop: {SENTINEL} second -->\n"
    );
    Scenario {
        repo,
        env: vec![("PR_BODY", body), ("PR_TITLE", "docs: wording".to_string())],
        _forge: None,
        _dir: None,
    }
}

/// What one scenario must put in each report.
struct Expect {
    name: &'static str,
    build: fn() -> Scenario,
    rule: &'static str,
    /// Entries of the rule the run reports.
    count: usize,
    /// Whether the refusal fails the run.
    blocks: bool,
    exit: i32,
}

const EXPECT: &[Expect] = &[
    Expect {
        name: "max_overrides",
        build: max_overrides,
        rule: "policy/max-overrides-exceeded",
        count: 1,
        blocks: true,
        exit: 1,
    },
    Expect {
        name: "max_inline_overrides",
        build: max_inline_overrides,
        rule: "policy/max-inline-overrides-exceeded",
        count: 1,
        blocks: true,
        exit: 1,
    },
    Expect {
        name: "require_approval",
        build: approval_required,
        rule: "policy/approval-required",
        count: 1,
        blocks: true,
        exit: 1,
    },
    Expect {
        name: "hidden directives",
        build: hidden_directives,
        rule: "policy/hidden-directive-refused",
        count: 3,
        blocks: false,
        exit: 0,
    },
];

/// The run's JSON report agrees with the scenario: the exit code, and the refusal in the
/// canonical record.
fn assert_canonical(e: &Expect, s: &Scenario) {
    let run = s.run("json");
    assert_eq!(run.code, e.exit, "{}: {}{}", e.name, run.stdout, run.stderr);
    let json = run.json();
    let failures = json["policy_failures"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if e.blocks {
        assert_eq!(failures.len(), e.count, "{}: {failures:?}", e.name);
        assert_eq!(json["errors"], 0, "{}: only the refusal fails it", e.name);
    } else {
        assert!(failures.is_empty(), "{}: {failures:?}", e.name);
        let notes = json["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|o| o["notes"].as_array().cloned().unwrap_or_default())
            .filter(|n| {
                n.as_str()
                    .is_some_and(|n| n.starts_with("hidden directive"))
            })
            .count();
        assert_eq!(notes, e.count, "{}", e.name);
    }
}

fn sarif_results(doc: &Value, rule: &str) -> Vec<Value> {
    doc["runs"][0]["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["ruleId"] == rule)
        .cloned()
        .collect()
}

#[test]
fn sarif_carries_one_result_per_refusal() {
    for e in EXPECT {
        let s = (e.build)();
        assert_canonical(e, &s);
        let run = s.run("sarif");
        assert_eq!(run.code, e.exit, "{}: {}", e.name, run.stderr);
        let doc: Value = serde_json::from_str(&run.stdout).unwrap();
        let results = sarif_results(&doc, e.rule);
        assert_eq!(
            results.len(),
            e.count,
            "{}: SARIF results for {}: {}",
            e.name,
            e.rule,
            run.stdout
        );
        let level = if e.blocks { "error" } else { "note" };
        let mut fingerprints = std::collections::BTreeSet::new();
        for r in &results {
            assert_eq!(r["level"], level, "{}: {r}", e.name);
            assert!(!r.to_string().contains(SENTINEL), "{}: {r}", e.name);
            let fp = r["partialFingerprints"]["disciplineFingerprint/v2"]
                .as_str()
                .unwrap_or_else(|| panic!("{}: no fingerprint: {r}", e.name));
            assert_eq!(fp.len(), 64, "{}", e.name);
            fingerprints.insert(fp.to_string());
        }
        assert_eq!(
            fingerprints.len(),
            e.count,
            "{}: shared fingerprint",
            e.name
        );
        // The rule the results name is described, as a gate's rule is.
        let rules = doc["runs"][0]["tool"]["driver"]["rules"]
            .as_array()
            .unwrap();
        let described: Vec<&Value> = rules.iter().filter(|r| r["id"] == e.rule).collect();
        assert_eq!(described.len(), 1, "{}: {rules:?}", e.name);
        for field in ["name", "shortDescription", "fullDescription", "helpUri"] {
            assert!(!described[0][field].is_null(), "{}: {field}", e.name);
        }
    }
}

/// The lines of the `policy` test suite, from its opening tag to its closing one.
fn junit_policy_suite(xml: &str) -> Vec<&str> {
    let mut inside = false;
    let mut out = Vec::new();
    for line in xml.lines() {
        if line.trim_start().starts_with("<testsuite name=\"policy\"") {
            inside = true;
        }
        if inside {
            out.push(line);
            if line.trim() == "</testsuite>" {
                break;
            }
        }
    }
    out
}

#[test]
fn junit_carries_one_test_case_per_refusal() {
    for e in EXPECT {
        let s = (e.build)();
        let run = s.run("junit");
        assert_eq!(run.code, e.exit, "{}: {}", e.name, run.stderr);
        let suite = junit_policy_suite(&run.stdout);
        assert!(
            !suite.is_empty(),
            "{}: no policy suite: {}",
            e.name,
            run.stdout
        );
        let code = e.rule.strip_prefix("policy/").unwrap();
        let cases: Vec<&&str> = suite
            .iter()
            .filter(|l| {
                l.trim_start()
                    .starts_with(&format!("<testcase name=\"{code}"))
            })
            .collect();
        assert_eq!(cases.len(), e.count, "{}: {suite:#?}", e.name);
        let names: std::collections::BTreeSet<&&&str> = cases.iter().collect();
        assert_eq!(names.len(), e.count, "{}: two cases share a name", e.name);
        let failed = suite
            .iter()
            .filter(|l| l.trim_start().starts_with("<failure "))
            .count();
        let expected_failures = if e.blocks { e.count } else { 0 };
        assert_eq!(failed, expected_failures, "{}: {suite:#?}", e.name);
        assert!(
            suite[0].contains(&format!(
                "tests=\"{}\" failures=\"{expected_failures}\"",
                e.count
            )),
            "{}: {}",
            e.name,
            suite[0]
        );
        assert!(
            !suite.join("\n").contains(SENTINEL),
            "{}: {suite:#?}",
            e.name
        );
        // The document's own totals count the refusals with the gates.
        let gates = s.run("json").json()["outcomes"].as_array().unwrap().len();
        let head = run.stdout.lines().nth(1).unwrap();
        assert!(
            head.contains(&format!("tests=\"{}\"", gates + e.count)),
            "{}: {head}",
            e.name
        );
        if e.blocks {
            assert!(!head.contains("failures=\"0\""), "{}: {head}", e.name);
        }
    }
}

#[test]
fn gitlab_carries_one_entry_per_refusal() {
    for e in EXPECT {
        let s = (e.build)();
        let run = s.run("gitlab");
        assert_eq!(run.code, e.exit, "{}: {}", e.name, run.stderr);
        let doc: Value = serde_json::from_str(&run.stdout).unwrap();
        let entries: Vec<&Value> = doc
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["check_name"] == e.rule)
            .collect();
        assert_eq!(entries.len(), e.count, "{}: {}", e.name, run.stdout);
        let severity = if e.blocks { "major" } else { "info" };
        let mut fingerprints = std::collections::BTreeSet::new();
        for i in &entries {
            assert_eq!(i["severity"], severity, "{}: {i}", e.name);
            assert!(!i.to_string().contains(SENTINEL), "{}: {i}", e.name);
            assert!(i["location"]["path"]
                .as_str()
                .is_some_and(|p| !p.is_empty()));
            assert!(i["location"]["lines"]["begin"].as_u64().is_some());
            let fp = i["fingerprint"].as_str().unwrap();
            assert_eq!(fp.len(), 64, "{}", e.name);
            fingerprints.insert(fp.to_string());
        }
        assert_eq!(
            fingerprints.len(),
            e.count,
            "{}: shared fingerprint",
            e.name
        );
        // The fingerprint SARIF carries for a refusal is the one GitLab carries.
        let sarif: Value = serde_json::from_str(&s.run("sarif").stdout).unwrap();
        let sarif_fps: std::collections::BTreeSet<String> = sarif_results(&sarif, e.rule)
            .iter()
            .filter_map(|r| r["partialFingerprints"]["disciplineFingerprint/v2"].as_str())
            .map(str::to_string)
            .collect();
        assert_eq!(sarif_fps, fingerprints, "{}", e.name);
    }
}

/// A refused hidden directive was never applied, so nothing of it but its place is in any
/// of the three reports: not its reason, not its subject, not its name. The reviewer
/// logins an approval refusal lists in the JSON report stay there.
#[test]
fn a_refusal_entry_names_no_directive_text_and_no_login() {
    let s = hidden_directives();
    for format in ["sarif", "junit", "gitlab"] {
        let out = s.run(format).stdout;
        for banned in [SENTINEL, "no-issue", "allow-assertion-drop"] {
            assert!(!out.contains(banned), "{format} carries `{banned}`: {out}");
        }
    }
    let s = approval_required();
    let json = s.run("json").json();
    let canonical = json["policy_failures"][0].as_str().unwrap();
    assert!(canonical.contains("lead-481"), "{canonical}");
    let sarif: Value = serde_json::from_str(&s.run("sarif").stdout).unwrap();
    let result = sarif_results(&sarif, "policy/approval-required")[0].to_string();
    let junit = s.run("junit").stdout;
    let suite = junit_policy_suite(&junit).join("\n");
    let gitlab: Value = serde_json::from_str(&s.run("gitlab").stdout).unwrap();
    let entry = gitlab
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["check_name"] == "policy/approval-required")
        .unwrap()
        .to_string();
    for (format, text) in [("sarif", result), ("junit", suite), ("gitlab", entry)] {
        for banned in ["lead-481", "author-481", "abc123", SENTINEL] {
            assert!(
                !text.contains(banned),
                "{format} carries `{banned}`: {text}"
            );
        }
    }
}

/// A refusal's fingerprint does not move when another refusal joins it, and a hidden
/// directive's entry is keyed by where it is.
#[test]
fn a_refusal_fingerprint_is_stable_and_keyed_by_its_place() {
    let fingerprints = |s: &Scenario, rule: &str| -> Vec<String> {
        let doc: Value = serde_json::from_str(&s.run("gitlab").stdout).unwrap();
        doc.as_array()
            .unwrap()
            .iter()
            .filter(|i| i["check_name"] == rule)
            .map(|i| i["fingerprint"].as_str().unwrap().to_string())
            .collect()
    };
    // The same budget refusal in two repositories is one identity.
    let a = fingerprints(&max_overrides(), "policy/max-overrides-exceeded");
    let b = fingerprints(&max_overrides(), "policy/max-overrides-exceeded");
    assert_eq!(a, b);

    // One hidden directive in the pull request body, then a second one after it: the
    // first keeps its fingerprint.
    let repo = repo_with_base_config(CONFIG_HEAD);
    repo.write("docs/plan.md", "# Plan\n\nPhase 1 then Phase 2.\nx\n");
    repo.commit("docs: wording");
    let with_body = |repo: Repo, body: &str| Scenario {
        repo,
        env: vec![
            ("PR_BODY", body.to_string()),
            ("PR_TITLE", "docs: wording".to_string()),
        ],
        _forge: None,
        _dir: None,
    };
    let one = with_body(repo, "Fix.\n<!-- no-issue: first -->\n");
    let alone = fingerprints(&one, "policy/hidden-directive-refused");
    assert_eq!(alone.len(), 1);
    let two = with_body(
        one.repo,
        "Fix.\n<!-- no-issue: first -->\n<!-- no-issue: second -->\n",
    );
    let both = fingerprints(&two, "policy/hidden-directive-refused");
    assert_eq!(both.len(), 2);
    assert_eq!(both[0], alone[0]);
    assert_ne!(both[0], both[1]);
}

/// A run with no refusal writes the three reports as it did: no policy rule, suite or entry.
#[test]
fn a_run_without_a_refusal_reports_no_policy_entry() {
    let repo = repo_with_base_config(CONFIG_HEAD);
    repo.write("docs/plan.md", "# Plan\n\nPhase 1 then Phase 2.\n");
    repo.commit("docs: wording");
    let s = Scenario::plain(repo);
    for format in ["sarif", "junit", "gitlab"] {
        let run = s.run(format);
        assert_eq!(run.code, 0, "{format}: {}", run.stderr);
        for marker in ["\"policy/", "name=\"policy\""] {
            assert!(!run.stdout.contains(marker), "{format}: {}", run.stdout);
        }
    }
}

/// A report with one gate that passed and every kind of refusal: the three that fail a
/// run, and hidden directives in a pull request body (two), a commit and a merged pull
/// request body. The sentences of `policy_failures` quote what the real ones quote.
fn summary_with_every_refusal() -> CheckSummary {
    let mut gate = GateOutcome::new("agents-md");
    gate.examined = 1;
    CheckSummary {
        schema_version: discipline::output_schema::REPORT_SCHEMA_VERSION,
        could_not_check: None,
        base: "origin/main".to_string(),
        errors: 0,
        warnings: 0,
        notes: 0,
        overrides: 0,
        baselined: 0,
        outcomes: vec![gate],
        planned_gates: Vec::new(),
        policy_failures: vec![
            PolicyFailure::new(
                RefusalKind::MaxInlineOverrides,
                format!("2 inline override(s) applied; {SENTINEL} allows 1"),
            ),
            PolicyFailure::new(
                RefusalKind::MaxOverrides,
                format!("2 directive override(s) applied; {SENTINEL} allows 1"),
            ),
            PolicyFailure::new(
                RefusalKind::ApprovalRequired,
                format!("2 directive override(s) await an approving review by one of: {SENTINEL}"),
            ),
        ],
        refused_hidden_directives: vec![
            OverrideSource::PrBody,
            OverrideSource::Commit("0123456789abcdef0123456789abcdef01234567".to_string()),
            OverrideSource::PrBody,
            OverrideSource::MergedPrBody(7),
        ],
        deprecations: Vec::new(),
        directive_notes: Vec::new(),
        unused_directives: Vec::new(),
    }
}

fn golden(format: OutputFormat, file: &str) {
    let rendered =
        format_report_content(&summary_with_every_refusal(), format, false, false).unwrap() + "\n";
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/report")
        .join(file);
    if std::env::var("DISCIPLINE_BLESS_REPORT_GOLDENS").is_ok_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &rendered).unwrap();
    }
    let golden =
        std::fs::read_to_string(&path).expect("run with DISCIPLINE_BLESS_REPORT_GOLDENS=1");
    assert!(
        rendered == golden,
        "the report changed; if intended, rerun with DISCIPLINE_BLESS_REPORT_GOLDENS=1 and review the diff of tests/fixtures/report/{file}\n{rendered}"
    );
}

#[test]
fn the_junit_report_of_every_refusal_matches_the_golden_file() {
    golden(OutputFormat::Junit, "policy_refusals.junit.xml");
}

#[test]
fn the_sarif_report_of_every_refusal_matches_the_golden_file() {
    golden(OutputFormat::Sarif, "policy_refusals.sarif.json");
}

#[test]
fn the_gitlab_report_of_every_refusal_matches_the_golden_file() {
    golden(OutputFormat::Gitlab, "policy_refusals.gitlab.json");
}

/// The level each format gives each kind of refusal, with and without
/// `--fail-on-warnings`: the format's error level for one that fails the run, its lowest
/// for one that does not.
#[test]
fn each_format_maps_a_refusal_to_its_level_whatever_fail_on_warnings_says() {
    let summary = summary_with_every_refusal();
    for fail_on_warnings in [false, true] {
        let render =
            |format| format_report_content(&summary, format, fail_on_warnings, false).unwrap();
        let sarif: Value = serde_json::from_str(&render(OutputFormat::Sarif)).unwrap();
        let gitlab: Value = serde_json::from_str(&render(OutputFormat::Gitlab)).unwrap();
        let junit = render(OutputFormat::Junit);
        let suite = junit_policy_suite(&junit);
        assert!(
            junit.contains("<testsuites name=\"discipline\" tests=\"8\" failures=\"3\""),
            "{junit}"
        );
        assert!(
            suite[0].contains("tests=\"7\" failures=\"3\" errors=\"0\" skipped=\"0\""),
            "{}",
            suite[0]
        );
        for info in REFUSALS {
            let rule = info.rule_id();
            let (level, severity) = if info.blocks {
                ("error", "major")
            } else {
                ("note", "info")
            };
            let results = sarif_results(&sarif, &rule);
            assert!(!results.is_empty(), "{rule}");
            assert!(results.iter().all(|r| r["level"] == level), "{rule}");
            let entries: Vec<&Value> = gitlab
                .as_array()
                .unwrap()
                .iter()
                .filter(|i| i["check_name"] == rule.as_str())
                .collect();
            assert_eq!(entries.len(), results.len(), "{rule}");
            assert!(entries.iter().all(|i| i["severity"] == severity), "{rule}");
            // Each test case of the kind is followed by a failure, or by a note.
            let follows: Vec<&str> = suite
                .windows(2)
                .filter(|w| {
                    w[0].trim_start()
                        .starts_with(&format!("<testcase name=\"{} [", info.code))
                })
                .map(|w| w[1].trim_start())
                .collect();
            assert_eq!(follows.len(), results.len(), "{rule}");
            let opening = if info.blocks {
                "<failure message=\"".to_string()
            } else {
                "<system-err>[note] ".to_string()
            };
            assert!(
                follows.iter().all(|l| l.starts_with(&opening)),
                "{rule}: {follows:?}"
            );
        }
        // Every entry of the three reports has its own fingerprint.
        let prints: std::collections::BTreeSet<&str> = gitlab
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["fingerprint"].as_str().unwrap())
            .collect();
        assert_eq!(prints.len(), 7);
    }
}
