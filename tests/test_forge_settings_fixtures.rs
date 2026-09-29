//! Phase 14 Step 0: what Gitea's and Forgejo's APIs expose for the Actions settings
//! `doctor` reads on GitHub, recorded against running instances (docs/ROADMAP.md,
//! Phase 14). These tests pin the facts the per-forge table in
//! docs/guides/ci-platforms.md states, so a re-recorded fixture that changes one fails
//! here first.

use serde_json::Value;

fn fixture(forge: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/forge_settings/{forge}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")))
        .unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn gitea_and_forgejo_list_secrets_by_name_with_no_scope() {
    for forge in ["gitea", "forgejo"] {
        let v = fixture(forge);
        assert!(
            v["observed"].as_str().unwrap().starts_with("RUN "),
            "{forge}"
        );
        let endpoints = v["endpoints"].as_object().unwrap();
        for path in [
            "/repos/{owner}/{repo}/actions/secrets",
            "/orgs/{org}/actions/secrets",
            "/repos/{owner}/{repo}/actions/variables",
            "/repos/{owner}/{repo}/tag_protections",
        ] {
            assert!(endpoints.contains_key(path), "{forge}: {path}");
        }
        for list in ["repo_secrets", "org_secrets"] {
            let secrets = v[list].as_array().unwrap();
            assert!(!secrets.is_empty(), "{forge}: {list}");
            for s in secrets {
                let keys: Vec<&str> = s.as_object().unwrap().keys().map(String::as_str).collect();
                // A name and when it was created, never a value, and no environment or
                // other scope to check.
                assert!(keys.contains(&"name"), "{forge}: {s}");
                for absent in ["data", "value", "environment", "environment_scope", "scope"] {
                    assert!(!keys.contains(&absent), "{forge}: {list} has {absent}");
                }
            }
        }
    }
}

#[test]
fn gitea_and_forgejo_have_no_api_for_token_permissions_policy_or_immutability() {
    for forge in ["gitea", "forgejo"] {
        let v = fixture(forge);
        for path in v["endpoints"].as_object().unwrap().keys() {
            for word in [
                "permissions",
                "immutable",
                "allowed",
                "workflow_permissions",
            ] {
                assert!(!path.contains(word), "{forge}: {path}");
            }
        }
        // Tag protection is the one setting both have; the API refuses one with no
        // user or team allowed to push.
        assert_eq!(v["tag_protections"][0]["name_pattern"], "v*", "{forge}");
        assert_eq!(
            v["tag_protection_create_without_whitelist"]["status"], 400,
            "{forge}"
        );
    }
}

#[test]
fn forgejo_returns_variables_without_their_value_and_gitea_with_it() {
    assert_eq!(fixture("gitea")["repo_variables"][0]["data"], "eu");
    assert_eq!(fixture("forgejo")["repo_variables"][0]["data"], "");
}
