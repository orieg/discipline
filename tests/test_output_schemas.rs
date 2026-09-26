//! The JSON Schemas of what discipline prints (`check --format json`, `replay --json`)
//! are part of the 1.0 interface. Two checks hold them still:
//!
//! 1. A snapshot of every field's path, type and whether it is required. Renaming,
//!    removing or retyping a field in `src/output_schema.rs` fails here until the
//!    snapshot is edited on purpose (and the change recorded in docs/ROADMAP.md).
//! 2. Real output, from the binary, validated against the schema. A field renamed or
//!    retyped in the Rust structs without the schema following fails here.

mod common;

use common::{FakeForge, Repo};
use discipline::output_schema::{replay_schema, report_schema};
use serde_json::Value;

/// `path: type`, `?` marking a field that may be absent. `[]` is an array element,
/// `{*}` a map value, `<Variant>` a tagged-union variant.
const REPORT_FIELDS: &[&str] = &[
    "base: string",
    "baselined: integer",
    "could_not_check?: object",
    "could_not_check.detail: string",
    "could_not_check.gate: string|null",
    "could_not_check.reason: enum(configuration|baseline|repository|tool-missing|tool-timeout|toolchain-unavailable|forge|gate|internal)",
    "deprecations?: array",
    "deprecations[]: string",
    "errors: integer",
    "notes: integer",
    "outcomes: array",
    "outcomes[]: object",
    "outcomes[].baselined: integer",
    "outcomes[].enabled: boolean",
    "outcomes[].examined: integer",
    "outcomes[].gate: string",
    "outcomes[].inline_exemptions: integer",
    "outcomes[].notes: array",
    "outcomes[].notes[]: string",
    "outcomes[].overrides: array",
    "outcomes[].overrides[]: object",
    "outcomes[].overrides[].directive: string",
    "outcomes[].overrides[].gate: string",
    "outcomes[].overrides[].hidden: boolean",
    "outcomes[].overrides[].reason: string",
    "outcomes[].overrides[].source: oneOf",
    "outcomes[].overrides[].source<PrBody>: object",
    "outcomes[].overrides[].source<PrBody>.type: const(PrBody)",
    "outcomes[].overrides[].source<Commit>: object",
    "outcomes[].overrides[].source<Commit>.detail: string",
    "outcomes[].overrides[].source<Commit>.type: const(Commit)",
    "outcomes[].overrides[].source<Inline>: object",
    "outcomes[].overrides[].source<Inline>.detail: object",
    "outcomes[].overrides[].source<Inline>.detail.file: string",
    "outcomes[].overrides[].source<Inline>.detail.line: integer",
    "outcomes[].overrides[].source<Inline>.type: const(Inline)",
    "outcomes[].overrides[].source<MergedPrBody>: object",
    "outcomes[].overrides[].source<MergedPrBody>.detail: integer",
    "outcomes[].overrides[].source<MergedPrBody>.type: const(MergedPrBody)",
    "outcomes[].overrides[].subject: string",
    "outcomes[].suite: string",
    "outcomes[].violations: array",
    "outcomes[].violations[]: object",
    "outcomes[].violations[].code: string",
    "outcomes[].violations[].file: string|null",
    "outcomes[].violations[].fingerprint: string",
    "outcomes[].violations[].gate: string",
    "outcomes[].violations[].line: integer|null",
    "outcomes[].violations[].message: string",
    "outcomes[].violations[].remediation: string|null",
    "outcomes[].violations[].severity: enum(error|warning|note)",
    "outcomes[].violations[].title: string",
    "overrides: integer",
    "planned_gates: array",
    "planned_gates[]: string",
    "policy_failures?: array",
    "policy_failures[]: string",
    "schema_version: const(1)",
    "warnings: integer",
];

const REPLAY_FIELDS: &[&str] = &[
    "blocked: integer",
    "cases: integer",
    "cases_detail: array",
    "cases_detail[]: object",
    "cases_detail[].actor: string|null",
    "cases_detail[].blocking_gates: array",
    "cases_detail[].blocking_gates[]: string",
    "cases_detail[].detail?: string",
    "cases_detail[].directives_from: string",
    "cases_detail[].pr: integer|null",
    "cases_detail[].reason?: enum(configuration|baseline|repository|tool-missing|tool-timeout|toolchain-unavailable|forge|gate|internal)",
    "cases_detail[].refused_overrides: array",
    "cases_detail[].refused_overrides[]: string",
    "cases_detail[].sha: string",
    "cases_detail[].subject: string",
    "cases_detail[].verdict: enum(passed|blocked|could_not_check)",
    "cases_detail[].warning_gates: array",
    "cases_detail[].warning_gates[]: string",
    "could_not_check: integer",
    "could_not_check_by_reason: object",
    "could_not_check_by_reason{*}: array",
    "could_not_check_by_reason{*}[]: string",
    "errors_by_gate: object",
    "errors_by_gate{*}: array",
    "errors_by_gate{*}[]: string",
    "passed: integer",
    "refused_overrides_by_gate: object",
    "refused_overrides_by_gate{*}: array",
    "refused_overrides_by_gate{*}[]: string",
    "schema_version: const(1)",
    "warnings_by_gate: object",
    "warnings_by_gate{*}: integer",
];

fn resolve<'a>(root: &'a Value, node: &'a Value) -> &'a Value {
    match node.get("$ref").and_then(Value::as_str) {
        Some(r) => {
            let name = r.strip_prefix("#/$defs/").expect("local $ref");
            &root["$defs"][name]
        }
        None => node,
    }
}

fn type_of(node: &Value) -> String {
    if let Some(e) = node.get("enum") {
        let vals: Vec<String> = e
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        return format!("enum({})", vals.join("|"));
    }
    if let Some(c) = node.get("const") {
        return match c {
            Value::String(s) => format!("const({s})"),
            other => format!("const({other})"),
        };
    }
    if node.get("oneOf").is_some() {
        return "oneOf".into();
    }
    match &node["type"] {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>()
            .join("|"),
        _ => "any".into(),
    }
}

fn flatten(root: &Value, node: &Value, path: &str, optional: bool, out: &mut Vec<String>) {
    let node = resolve(root, node);
    let mark = if optional { "?" } else { "" };
    if !path.is_empty() {
        out.push(format!("{path}{mark}: {}", type_of(node)));
    }
    if let Some(variants) = node.get("oneOf").and_then(Value::as_array) {
        for v in variants {
            let tag = v["properties"]["type"]["const"].as_str().unwrap();
            flatten(root, v, &format!("{path}<{tag}>"), false, out);
        }
        return;
    }
    if let Some(props) = node.get("properties").and_then(Value::as_object) {
        let required: Vec<&str> = node["required"]
            .as_array()
            .map(|r| r.iter().map(|v| v.as_str().unwrap()).collect())
            .unwrap_or_default();
        for (k, v) in props {
            let child = if path.is_empty() {
                k.clone()
            } else {
                format!("{path}.{k}")
            };
            flatten(root, v, &child, !required.contains(&k.as_str()), out);
        }
    }
    if let Some(items) = node.get("items") {
        flatten(root, items, &format!("{path}[]"), false, out);
    }
    if let Some(map) = node.get("additionalProperties").filter(|v| v.is_object()) {
        flatten(root, map, &format!("{path}{{*}}"), false, out);
    }
}

fn fields(schema: &Value) -> Vec<String> {
    let mut out = Vec::new();
    flatten(schema, schema, "", false, &mut out);
    out
}

fn assert_snapshot(name: &str, schema: &Value, snapshot: &[&str]) {
    let actual = fields(schema);
    let expected: Vec<String> = snapshot.iter().map(|s| s.to_string()).collect();
    assert!(
        actual == expected,
        "{name}: the schema's fields changed. A rename, removal or type change of an \
         existing field breaks the 1.0 interface; an added field is a minor change. Update \
         the snapshot on purpose and record the change in docs/ROADMAP.md.\nactual:\n{}",
        actual
            .iter()
            .map(|f| format!("    \"{f}\","))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn report_schema_fields_match_snapshot() {
    assert_snapshot("report", &report_schema(), REPORT_FIELDS);
}

#[test]
fn replay_schema_fields_match_snapshot() {
    assert_snapshot("replay", &replay_schema(), REPLAY_FIELDS);
}

#[test]
fn committed_schema_files_match_the_generator() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for (file, schema) in [
        ("discipline.report.schema.json", report_schema()),
        ("discipline.replay.schema.json", replay_schema()),
    ] {
        let committed: Value =
            serde_json::from_str(&std::fs::read_to_string(root.join(file)).unwrap()).unwrap();
        assert_eq!(committed, schema, "{file}: run `discipline docs --write`");
    }
}

/// The subset of JSON Schema the two schemas use: `type`, `enum`, `const`, `pattern`, `minimum`,
/// `properties` / `required` / `additionalProperties`, `items`, local `$ref`, `oneOf`.
fn validate(root: &Value, node: &Value, v: &Value, at: &str, errs: &mut Vec<String>) {
    let node = resolve(root, node);
    if let Some(variants) = node.get("oneOf").and_then(Value::as_array) {
        let matching = variants
            .iter()
            .filter(|s| {
                let mut e = Vec::new();
                validate(root, s, v, at, &mut e);
                e.is_empty()
            })
            .count();
        if matching != 1 {
            errs.push(format!("{at}: matches {matching} oneOf variants: {v}"));
        }
        return;
    }
    if let Some(e) = node.get("enum").and_then(Value::as_array) {
        if !e.contains(v) {
            errs.push(format!("{at}: {v} not in {e:?}"));
        }
    }
    if let Some(c) = node.get("const") {
        if c != v {
            errs.push(format!("{at}: {v} is not {c}"));
        }
    }
    let types: Vec<&str> = match &node["type"] {
        Value::String(s) => vec![s.as_str()],
        Value::Array(a) => a.iter().map(|t| t.as_str().unwrap()).collect(),
        _ => vec![],
    };
    if !types.is_empty() {
        let actual = match v {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(n) if n.is_u64() || n.is_i64() => "integer",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        if !types.contains(&actual) {
            errs.push(format!("{at}: {actual} where {types:?} is required"));
            return;
        }
    }
    if let (Some(p), Some(s)) = (node.get("pattern").and_then(Value::as_str), v.as_str()) {
        if !regex::Regex::new(p).unwrap().is_match(s) {
            errs.push(format!("{at}: `{s}` does not match `{p}`"));
        }
    }
    if let (Some(min), Some(n)) = (node.get("minimum").and_then(Value::as_i64), v.as_i64()) {
        if n < min {
            errs.push(format!("{at}: {n} below {min}"));
        }
    }
    if let Value::Object(obj) = v {
        let props = node.get("properties").and_then(Value::as_object);
        for r in node["required"].as_array().into_iter().flatten() {
            if !obj.contains_key(r.as_str().unwrap()) {
                errs.push(format!("{at}: required `{r}` missing"));
            }
        }
        for (k, val) in obj {
            let child = format!("{at}.{k}");
            match props.and_then(|p| p.get(k)) {
                Some(s) => validate(root, s, val, &child, errs),
                None => match node.get("additionalProperties") {
                    Some(Value::Bool(false)) => errs.push(format!("{child}: not in the schema")),
                    Some(s) if s.is_object() => validate(root, s, val, &child, errs),
                    _ => {}
                },
            }
        }
    }
    if let (Value::Array(items), Some(s)) = (v, node.get("items")) {
        for (i, item) in items.iter().enumerate() {
            validate(root, s, item, &format!("{at}[{i}]"), errs);
        }
    }
}

fn assert_valid(schema: &Value, v: &Value) {
    let mut errs = Vec::new();
    validate(schema, schema, v, "$", &mut errs);
    assert!(errs.is_empty(), "{errs:#?}\n{v:#}");
}

#[test]
fn check_output_conforms_to_the_report_schema() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "tests/a.rs",
        "#[test]\nfn kept() { assert_eq!(1 + 1, 2); }\n",
    );
    repo.commit("test: base");
    repo.git(&["checkout", "-q", "-B", "work"]);
    // A vacuous test (error), a deleted-then-ignored test lifted from the PR body, and an
    // override from a commit message: every override source a local run can produce.
    repo.write(
        "tests/a.rs",
        "#[test]\n#[ignore]\nfn kept() { assert_eq!(1 + 1, 2); }\n\n#[test]\nfn ghost() {}\n",
    );
    repo.commit("test: park kept\n\nallow-vacuous-test: ghost placeholder for the fixture loader");
    repo.write(
        "body.md",
        "allow-ignore: kept flaky on the shared runner, tracked upstream\n",
    );
    let run = repo.check(&["--pr-body-file", "body.md"]);
    let report: Value = serde_json::from_str(&run.stdout).unwrap();
    assert!(
        report["overrides"].as_u64().unwrap() >= 2,
        "the fixture should carry overrides from two sources: {report:#}"
    );
    assert_valid(&report_schema(), &report);

    // A finding that blocks, so violations carry every field.
    let blocked = repo.check(&[]);
    assert_eq!(blocked.code, 1, "{}", blocked.stdout);
    assert_valid(
        &report_schema(),
        &serde_json::from_str(&blocked.stdout).unwrap(),
    );

    // Exit 2: stdout and `--json-out` get the same report, with no outcomes and the reason.
    repo.write(
        "discipline.toml",
        "[meta]\nversion = 1\nname = \"t\"\n[gates.no-such-gate]\nenabled = true\n",
    );
    let out = repo.file("fatal.json");
    let fatal = repo.run(
        &[
            "check",
            "--format",
            "json",
            "--base",
            "main",
            "--json-out",
            out.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(fatal.code, 2, "{}", fatal.stderr);
    let report: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(report["outcomes"], serde_json::json!([]), "{report:#}");
    assert_eq!(
        report["could_not_check"]["reason"], "configuration",
        "{report:#}"
    );
    assert!(
        fatal.stderr.contains(
            report["could_not_check"]["detail"]
                .as_str()
                .unwrap()
                .trim_end()
        ),
        "the detail is the error on stderr: {report:#}\n{}",
        fatal.stderr
    );
    assert_eq!(
        serde_json::from_str::<Value>(&fatal.stdout).unwrap(),
        report,
        "stdout carries the report --json-out writes"
    );
    assert_valid(&report_schema(), &report);
}

#[test]
fn replay_output_conforms_to_the_replay_schema() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "tests/a.rs",
        "#[test]\nfn kept() { assert_eq!(1 + 1, 2); }\n",
    );
    repo.commit("test: base (#1)");
    repo.write("tests/a.rs", "#[test]\nfn kept() {}\n");
    repo.commit("test: gut kept (#2)");
    repo.write("docs/notes.md", "# Notes\n");
    repo.commit("docs: notes (#3)");
    let weakening = repo.git_output(&["rev-parse", "HEAD~1"]).trim().to_string();
    // A forge that knows #2's author, so a case carries `actor` and `pr` from the forge,
    // under a configuration that refuses the override: every field is populated.
    let api = FakeForge::start();
    api.serve(
        &format!("repos/o/r/commits/{weakening}/pulls"),
        serde_json::json!([{
            "number": 2,
            "merged_at": "2026-09-21T00:00:00Z",
            "user": {"login": "outsider"},
            "body": "allow-assertion-drop: kept replaced by an integration test",
            "head": {"sha": "feedbeef"}
        }]),
    );
    let cfg = repo.file("candidate.toml");
    std::fs::write(
        &cfg,
        "[meta]\nversion = 1\nname = \"t\"\n[directives]\nfail_on_overrides = true\nallowed_override_actors = [\"lead\"]\n",
    )
    .unwrap();
    let url = api.url();
    let run = repo.run(
        &[
            "replay",
            "--last",
            "2",
            "--ref",
            "main",
            "--json",
            "--config",
            cfg.to_str().unwrap(),
        ],
        &[
            ("GITHUB_REPOSITORY", "o/r"),
            ("DISCIPLINE_FORGE_API_URL", url.as_str()),
            ("GITHUB_TOKEN", "t"),
        ],
    );
    assert_eq!(run.code, 0, "{}", run.stderr);
    let summary: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(
        summary["refused_overrides_by_gate"]["assertion-reduction"],
        serde_json::json!(["#2"]),
        "{summary:#}"
    );
    assert_valid(&replay_schema(), &summary);

    // A forge that answers 403: the blocked case becomes `could_not_check` with a detail.
    let denied = FakeForge::start();
    let url = denied.url();
    let run = repo.run(
        &["replay", "--last", "2", "--ref", "main", "--json"],
        &[
            ("GITHUB_REPOSITORY", "o/r"),
            ("DISCIPLINE_FORGE_API_URL", url.as_str()),
        ],
    );
    let summary: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(summary["could_not_check"], 1, "{summary:#}");
    assert_valid(&replay_schema(), &summary);
}

/// A repository whose check reports several findings across files and gates, with a note
/// on stderr (no discipline.toml).
fn noisy_repo() -> Repo {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "tests/a.rs",
        "#[test]\nfn kept() { assert_eq!(1 + 1, 2); }\n",
    );
    repo.commit("test: base");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write(
        "tests/a.rs",
        "#[test]\nfn kept() {}\n\n#[test]\nfn ghost() {}\n",
    );
    repo.write("tests/b.rs", "#[test]\nfn other() { assert!(true); }\n");
    repo.write("docs/plan.md", "# Plan\n\nShip it in 3 weeks.\n");
    repo.write(
        "src/lib.rs",
        "pub fn read(p: *const u8) -> u8 {\n    unsafe { *p }\n}\n",
    );
    repo.commit("feat: several findings");
    repo
}

#[test]
fn the_same_tree_gives_byte_identical_reports() {
    let repo = noisy_repo();
    for format in ["json", "sarif"] {
        let first = repo.run(&["check", "--format", format, "--base", "main"], &[]);
        let second = repo.run(&["check", "--format", format, "--base", "main"], &[]);
        assert_eq!(first.code, 1, "{}", first.stdout);
        assert_eq!(
            first.stdout, second.stdout,
            "{format} output differs between runs"
        );
    }
}

#[test]
fn stdout_carries_only_the_requested_format() {
    let repo = noisy_repo();
    for format in ["json", "sarif"] {
        let run = repo.run(
            &[
                "check",
                "--format",
                format,
                "--base",
                "main",
                "--fail-on-warnings",
            ],
            &[],
        );
        assert!(
            !run.stderr.is_empty(),
            "the fixture should print notes on stderr"
        );
        let parsed: Result<Value, _> = serde_json::from_str(&run.stdout);
        assert!(
            parsed.is_ok(),
            "{format}: stdout is not a single JSON document:\n{}",
            run.stdout
        );
    }
    // A run that cannot start prints one JSON document saying why (the error is also
    // on stderr), and nothing on stdout in a format with no such field.
    repo.write("discipline.toml", "not = [valid\n");
    let broken = repo.run(&["check", "--format", "json", "--base", "main"], &[]);
    assert_eq!(broken.code, 2, "{}", broken.stderr);
    let report: Value = serde_json::from_str(&broken.stdout)
        .unwrap_or_else(|e| panic!("exit 2 prints one JSON document: {e}\n{}", broken.stdout));
    assert_eq!(report["could_not_check"]["reason"], "configuration");
    assert_eq!(report["outcomes"], serde_json::json!([]));
    for format in ["terminal", "sarif", "agent-prompt"] {
        let run = repo.run(&["check", "--format", format, "--base", "main"], &[]);
        assert_eq!(run.code, 2, "{format}: {}", run.stderr);
        assert_eq!(run.stdout, "", "{format}: exit 2 prints no partial report");
    }
}
