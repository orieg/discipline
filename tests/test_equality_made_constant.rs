//! Equality method made constant (#481, D2), through the real binary:
//! `stub-bodies/equality-made-constant` reported when an added or changed equality method
//! returns constant `true`, with AST-level carve-outs for test doubles, mock frameworks,
//! and generated equality.

mod common;
use common::{Repo, Run, CONFIG_HEAD};

const GATE: &str = "stub-bodies";
const CODE: &str = "stub-bodies/equality-made-constant";

fn repo() -> Repo {
    let repo = Repo::new();
    repo.write("discipline.toml", CONFIG_HEAD);
    repo.commit("chore: configuration");
    repo.git(&["branch", "-f", "main"]);
    repo
}

fn reported(run: &Run) -> Vec<(String, String, u64, String)> {
    run.violations(GATE)
        .iter()
        .map(|v| {
            (
                v["code"].as_str().unwrap().to_string(),
                v["severity"].as_str().unwrap().to_string(),
                v["line"].as_u64().unwrap_or(0),
                v["message"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Python Acceptance Criteria & Precision
// ---------------------------------------------------------------------------

#[test]
fn positive_control_python_eq_returns_true_in_production_code() {
    let r = repo();
    r.write(
        "src/user.py",
        "class User:\n    def __eq__(self, other):\n        return True\n",
    );
    r.commit("feat: add user");
    let run = r.check(&[]);
    let got = reported(&run);
    assert_eq!(
        run.code, 0,
        "severity warning does not fail by default: {}",
        run.stdout
    );
    assert_eq!(got.len(), 1, "expected 1 finding, got: {got:?}");
    assert_eq!(got[0].0, CODE);
    assert_eq!(got[0].1, "warning");
    assert_eq!(got[0].2, 2);
    assert!(got[0].3.contains("__eq__"));
}

#[test]
fn positive_control_python_eq_with_docstring_and_logging() {
    let r = repo();
    r.write(
        "src/auth.py",
        "import logging\n\nlogger = logging.getLogger(__name__)\n\nclass Token:\n    def __eq__(self, other):\n        \"\"\"Compare tokens for equality.\"\"\"\n        logger.debug(\"comparing tokens\")\n        return True\n",
    );
    r.commit("feat: add token");
    let run = r.check(&[]);
    let got = reported(&run);
    assert_eq!(run.code, 0);
    assert_eq!(got.len(), 1, "expected 1 finding, got: {got:?}");
    assert_eq!(got[0].0, CODE);
    assert_eq!(got[0].1, "warning");
    assert_eq!(got[0].2, 6);
}

#[test]
fn positive_control_changed_equality_made_constant() {
    let r = repo();
    r.commit_base(
        "src/account.py",
        "class Account:\n    def __eq__(self, other):\n        return self.id == other.id\n",
        "feat: base account",
    );
    r.write(
        "src/account.py",
        "class Account:\n    def __eq__(self, other):\n        return True\n",
    );
    r.commit("fix: make equality return true");
    let run = r.check(&[]);
    let got = reported(&run);
    assert_eq!(run.code, 0);
    assert_eq!(got.len(), 1, "expected 1 finding, got: {got:?}");
    assert_eq!(got[0].0, CODE);
    assert_eq!(got[0].1, "warning");
    assert_eq!(got[0].2, 2);
}

#[test]
fn positive_control_real_class_in_test_path() {
    let r = repo();
    r.write(
        "tests/test_user.py",
        "class RealUser:\n    def __eq__(self, other):\n        return True\n\ndef test_user():\n    assert len(str(RealUser())) > 0\n",
    );
    r.commit("test: add real user in test path");
    let run = r.check(&[]);
    let got = reported(&run);
    assert_eq!(run.code, 0);
    assert_eq!(
        got.len(),
        1,
        "real class in test path must not be carved out: {got:?}"
    );
    assert_eq!(got[0].0, CODE);
    assert_eq!(got[0].1, "warning");
    assert_eq!(got[0].2, 2);
}

// ---------------------------------------------------------------------------
// Negative Controls & Carve-outs
// ---------------------------------------------------------------------------

#[test]
fn negative_control_field_comparison() {
    let r = repo();
    r.write(
        "src/user.py",
        "class User:\n    def __eq__(self, other):\n        return self.id == other.id\n",
    );
    r.commit("feat: add user with real equality");
    let run = r.check(&[]);
    let got = reported(&run);
    assert!(got.is_empty(), "field comparison must be silent: {got:?}");
}

#[test]
fn negative_control_stub_in_test_path() {
    let r = repo();
    r.write(
        "tests/test_auth.py",
        "class StubToken:\n    def __eq__(self, other):\n        return True\n\ndef test_stub():\n    assert len(str(StubToken())) > 0\n",
    );
    r.commit("test: add stub in test path");
    let run = r.check(&[]);
    let got = reported(&run);
    assert!(
        got.is_empty(),
        "stub in test path must be carved out: {got:?}"
    );
}

#[test]
fn negative_control_fake_in_test_path() {
    let r = repo();
    r.write(
        "tests/test_client.py",
        "class FakeClient:\n    def __eq__(self, other):\n        return True\n\ndef test_fake():\n    assert len(str(FakeClient())) > 0\n",
    );
    r.commit("test: add fake in test path");
    let run = r.check(&[]);
    let got = reported(&run);
    assert!(
        got.is_empty(),
        "fake in test path must be carved out: {got:?}"
    );
}

#[test]
fn negative_control_mock_framework_stub() {
    let r = repo();
    r.write(
        "tests/test_mock.py",
        "from unittest.mock import MagicMock\n\nclass MockSession(MagicMock):\n    def __eq__(self, other):\n        return True\n\ndef test_mock():\n    assert len(str(MockSession())) > 0\n",
    );
    r.commit("test: add mock framework stub");
    let run = r.check(&[]);
    let got = reported(&run);
    assert!(
        got.is_empty(),
        "mock framework stub must be carved out: {got:?}"
    );
}

#[test]
fn negative_control_generated_dataclass() {
    let r = repo();
    r.write(
        "src/point.py",
        "from dataclasses import dataclass\n\n@dataclass\nclass Point:\n    def __eq__(self, other):\n        return True\n",
    );
    r.commit("feat: add dataclass");
    let run = r.check(&[]);
    let got = reported(&run);
    assert!(got.is_empty(), "@dataclass must be carved out: {got:?}");
}

#[test]
fn negative_control_generated_pydantic_basemodel() {
    let r = repo();
    r.write(
        "src/model.py",
        "from pydantic import BaseModel\n\nclass Item(BaseModel):\n    def __eq__(self, other):\n        return True\n",
    );
    r.commit("feat: add pydantic model");
    let run = r.check(&[]);
    let got = reported(&run);
    assert!(got.is_empty(), "BaseModel must be carved out: {got:?}");
}

#[test]
fn negative_control_delegating_inequality() {
    let r = repo();
    r.write(
        "src/user.py",
        "class User:\n    def __ne__(self, other):\n        return not (self == other)\n",
    );
    r.commit("feat: add delegating inequality");
    let run = r.check(&[]);
    let got = reported(&run);
    assert!(
        got.is_empty(),
        "__ne__ delegating to == must be silent: {got:?}"
    );
}

#[test]
fn negative_control_marker_type_lifted_by_directive() {
    let r = repo();
    r.write(
        "src/wildcard.py",
        "class WildcardMatcher:\n    def __eq__(self, other):\n        return True\n",
    );
    r.commit("feat: add wildcard matcher\n\nallow-stub: WildcardMatcher represents universal equality by design\n");
    let run = r.check(&[]);
    let got = reported(&run);
    assert!(
        got.is_empty(),
        "allow-stub on type name must lift finding: {got:?}"
    );
    let outcome = run.outcome(GATE);
    assert_eq!(
        outcome["overrides"].as_array().map(|a| a.len()),
        Some(1),
        "override must be recorded"
    );
}

#[test]
fn negative_control_near_constant_bodies_out_of_scope() {
    let r = repo();
    r.write(
        "src/residuals.py",
        "class A:\n    def __eq__(self, other):\n        x = True\n        return x\n\nclass B:\n    def __eq__(self, other):\n        return (self.id == other.id) or True\n",
    );
    r.commit("feat: add residual forms");
    let run = r.check(&[]);
    let got = reported(&run);
    assert!(
        got.is_empty(),
        "near-constant residuals are out of scope: {got:?}"
    );
}

#[test]
fn negative_control_already_constant_on_base_side() {
    let r = repo();
    r.commit_base(
        "src/legacy.py",
        "class Legacy:\n    def __eq__(self, other):\n        return True\n",
        "feat: base legacy",
    );
    r.write(
        "src/legacy.py",
        "class Legacy:\n    # untouched method\n    def __eq__(self, other):\n        return True\n",
    );
    r.commit("docs: comment on legacy");
    let run = r.check(&[]);
    let got = reported(&run);
    assert!(
        got.is_empty(),
        "already constant on base must not be reported: {got:?}"
    );
}

// ---------------------------------------------------------------------------
// Multi-Language Controls
// ---------------------------------------------------------------------------

#[test]
fn positive_control_rust_eq_made_constant() {
    let r = repo();
    r.write(
        "src/user.rs",
        "pub struct User {\n    pub id: u64,\n}\n\nimpl PartialEq for User {\n    fn eq(&self, _other: &Self) -> bool {\n        true\n    }\n}\n",
    );
    r.commit("feat: add rust user");
    let run = r.check(&[]);
    let got = reported(&run);
    assert_eq!(got.len(), 1, "expected 1 finding, got: {got:?}");
    assert_eq!(got[0].0, CODE);
    assert_eq!(got[0].1, "warning");
    assert_eq!(got[0].2, 6);
}

#[test]
fn negative_control_rust_derived_partial_eq() {
    let r = repo();
    r.write(
        "src/user.rs",
        "#[derive(PartialEq)]\npub struct User {\n    pub id: u64,\n}\n",
    );
    r.commit("feat: add derived partial eq");
    let run = r.check(&[]);
    let got = reported(&run);
    assert!(got.is_empty(), "derived PartialEq must be silent: {got:?}");
}

#[test]
fn positive_control_java_equals_made_constant() {
    let r = repo();
    r.write(
        "src/User.java",
        "public class User {\n    public boolean equals(Object o) {\n        return true;\n    }\n}\n",
    );
    r.commit("feat: add java user");
    let run = r.check(&[]);
    let got = reported(&run);
    assert_eq!(got.len(), 1, "expected 1 finding, got: {got:?}");
    assert_eq!(got[0].0, CODE);
    assert_eq!(got[0].1, "warning");
    assert_eq!(got[0].2, 2);
}

#[test]
fn negative_control_java_record_and_lombok() {
    let r = repo();
    r.write("src/Point.java", "public record Point(int x, int y) {}\n");
    r.write(
        "src/Account.java",
        "import lombok.EqualsAndHashCode;\n\n@EqualsAndHashCode\npublic class Account {\n    int id;\n}\n",
    );
    r.commit("feat: add record and lombok");
    let run = r.check(&[]);
    let got = reported(&run);
    assert!(got.is_empty(), "record and lombok must be silent: {got:?}");
}

#[test]
fn positive_control_cpp_operator_equality_constant_true() {
    let r = repo();
    r.write(
        "src/user.cc",
        "struct User {\n    bool operator==(const User& other) const {\n        return true;\n    }\n};\n",
    );
    r.commit("feat: add cpp user");
    let run = r.check(&[]);
    let got = reported(&run);
    assert_eq!(got.len(), 1, "expected 1 finding, got: {got:?}");
    assert_eq!(got[0].0, CODE);
    assert_eq!(got[0].1, "warning");
    assert_eq!(got[0].2, 2);
}

#[test]
fn negative_control_cpp_operator_equality() {
    let r = repo();
    r.write(
        "src/user.cc",
        "struct User {\n    int id;\n    bool operator==(const User& other) const {\n        return id == other.id;\n    }\n};\n",
    );
    r.commit("feat: add cpp user with real equality");
    let run = r.check(&[]);
    let got = reported(&run);
    assert!(
        got.is_empty(),
        "real cpp operator== must be silent: {got:?}"
    );
}
