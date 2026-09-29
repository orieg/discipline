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
                // The keys only: a failure never prints a secret entry.
                assert!(keys.contains(&"name"), "{forge}: {list} keys {keys:?}");
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

#[test]
fn gitlab_variables_carry_their_scope_and_never_their_value() {
    let v = fixture("gitlab");
    assert!(v["observed"].as_str().unwrap().starts_with("RUN "));
    let vars = v["variables"].as_array().unwrap();
    assert!(!vars.is_empty());
    for var in vars {
        let o = var.as_object().unwrap();
        for key in ["key", "protected", "masked", "hidden", "environment_scope"] {
            assert!(o.contains_key(key), "{}: {key}", var["key"]);
        }
        assert!(
            !o.contains_key("value"),
            "a recorded value for {}",
            var["key"]
        );
    }
    // One of each shape `secret-scoping` tells apart: readable by every pipeline,
    // protected branches and tags only, one environment only.
    let shape = |k: &str| vars.iter().find(|x| x["key"] == k).unwrap().clone();
    assert_eq!(shape("PLAIN_VAR")["environment_scope"], "*");
    assert_eq!(shape("PLAIN_VAR")["protected"], false);
    assert_eq!(shape("PROTECTED_VAR")["protected"], true);
    assert_eq!(shape("DEPLOY_TOKEN")["environment_scope"], "production");
    assert_eq!(shape("DEPLOY_TOKEN")["masked"], true);
}

#[test]
fn gitlab_exposes_the_job_token_scope_and_the_project_ci_settings() {
    let v = fixture("gitlab");
    assert!(v["job_token_scope"]["inbound_enabled"].is_boolean());
    let ci = v["project_ci_settings"].as_object().unwrap();
    for key in [
        "ci_allow_fork_pipelines_to_run_in_parent_project",
        "ci_push_repository_for_job_token_allowed",
        "restrict_user_defined_variables",
        "ci_pipeline_variables_minimum_override_role",
        "protect_merge_request_pipelines",
        "public_jobs",
    ] {
        assert!(ci.contains_key(key), "{key}");
    }
    assert_eq!(v["protected_tags"][0]["name"], "v*");
    assert_eq!(v["protected_environments"][0]["name"], "production");
    // Each setting behind its own fine-grained token permission, named by the API.
    let perms = v["granular_permissions"].as_object().unwrap();
    assert_eq!(perms["/projects/:id/variables"], "Variable: Read");
    assert_eq!(
        perms["/projects/:id/job_token_scope"],
        "Job Token Scope: Read"
    );
}

#[test]
fn a_gitlab_token_describes_its_own_scopes_expiry_and_granularity() {
    let t = fixture("gitlab")["token_self"].clone();
    for key in ["scopes", "granular", "active", "revoked", "expires_at"] {
        assert!(
            t["keys"].as_array().unwrap().iter().any(|k| k == key),
            "{key}"
        );
    }
    let scopes: Vec<&str> = t["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap())
        .collect();
    // The recorded token could write: the case a token audit exists to report, since
    // `doctor` needs `read_api` and the named read permissions only.
    assert!(scopes.contains(&"read_api"));
    assert!(scopes.contains(&"api"), "{scopes:?}");
}
