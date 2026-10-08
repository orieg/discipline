//! Fixtures the doctor unit tests share.

use super::{Finding, LocalFacts};
use crate::forge::{CannedApi, Forge, ForgeKind};

pub(super) const WF: &str = r#"
name: CI
on:
  pull_request:
    types: [opened, synchronize, reopened, edited]
permissions:
  contents: read
jobs:
  lint:
    runs-on: ubuntu-latest
    steps: [{ run: cargo clippy }]
  gate:
    name: Discipline
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: orieg/discipline@v0
  report:
    needs: gate
    runs-on: ubuntu-latest
    steps: [{ run: echo done }]
  ci-gate:
    if: always()
    needs: [lint, gate, report]
    runs-on: ubuntu-latest
    steps:
      - env:
          NEEDS: ${{ toJson(needs) }}
        run: echo "$NEEDS" | jq -e 'all(.[]; .result == "success")'
"#;

pub(super) fn wf(content: &str) -> Vec<(String, String)> {
    vec![(".github/workflows/ci.yml".to_string(), content.to_string())]
}

pub(super) fn forge(kind: ForgeKind) -> Forge {
    Forge {
        kind,
        url: "https://git.example.com".into(),
        repo: "o/r".into(),
    }
}

pub(super) fn github(rules: serde_json::Value, bypass: serde_json::Value) -> CannedApi {
    let mut c = CannedApi::default();
    c.responses
        .insert("github:repos/o/r/rules/branches/main".into(), rules);
    c.responses.insert(
        "github:repos/o/r/rulesets/7".into(),
        serde_json::json!({ "bypass_actors": bypass }),
    );
    c.responses.insert(
        "github:repos/o/r/branches/main".into(),
        serde_json::json!({"protected": true, "protection": {"enabled": false}}),
    );
    c.responses.insert(
        "github:repos/o/r".into(),
        serde_json::json!({"default_branch": "main"}),
    );
    c
}

pub(super) fn full_rules() -> serde_json::Value {
    serde_json::json!([
      {"type": "required_status_checks", "ruleset_id": 7, "parameters": {
         "strict_required_status_checks_policy": true,
         "required_status_checks": [{"context": "ci-gate"}]}},
      {"type": "non_fast_forward", "ruleset_id": 7},
      {"type": "deletion", "ruleset_id": 7},
      {"type": "pull_request", "ruleset_id": 7, "parameters": {"required_approving_review_count": 1, "require_code_owner_review": true, "require_last_push_approval": true}}
    ])
}

pub(super) fn non_blocking(facts: &LocalFacts) -> Vec<&Finding> {
    facts
        .findings
        .iter()
        .filter(|f| f.id == "non-blocking")
        .collect()
}
