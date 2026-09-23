//! Editor and bot integrations documented in `docs/CONFIGURATION.md`, checked against the
//! binary's real output so a format change cannot silently break them.

mod common;

use common::Repo;

/// The JSON block between `<!-- name -->` and `<!-- /name -->` in the configuration docs.
fn documented_json(name: &str) -> serde_json::Value {
    let doc = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/CONFIGURATION.md"),
    )
    .unwrap();
    let start = doc
        .find(&format!("<!-- {name} -->"))
        .unwrap_or_else(|| panic!("no {name} block"));
    let end = doc.find(&format!("<!-- /{name} -->")).unwrap();
    let block = &doc[start..end];
    let json = block
        .split_once("```json\n")
        .and_then(|(_, rest)| rest.split_once("```"))
        .map(|(j, _)| j)
        .unwrap();
    serde_json::from_str(json).unwrap_or_else(|e| panic!("{name} is not JSON: {e}\n{json}"))
}

#[test]
fn the_documented_vscode_problem_matcher_reads_real_findings() {
    let tasks = documented_json("vscode-problem-matcher");
    let task = &tasks["tasks"][0];
    assert_eq!(task["options"]["env"]["NO_COLOR"], "1");
    let pattern = &task["problemMatcher"]["pattern"];
    let head = regex::Regex::new(pattern[0]["regexp"].as_str().unwrap()).unwrap();
    let message = regex::Regex::new(pattern[1]["regexp"].as_str().unwrap()).unwrap();

    let repo = Repo::new();
    repo.write(
        "tests/a.rs",
        "#[test]\nfn adds() {\n    let x = 1;\n    let _ = x + 1;\n}\n\n#[test]\nfn orders() {\n    let x = 1;\n    assert!(x < 2);\n}\n",
    );
    let run = repo.run(&["diff"], &[("NO_COLOR", "1")]);
    assert_eq!(run.code, 1, "{}", run.stdout);
    let lines: Vec<&str> = run.stdout.lines().collect();
    let mut found = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if let Some(c) = head.captures(l) {
            let next = lines.get(i + 1).copied().unwrap_or("");
            let msg = message
                .captures(next)
                .unwrap_or_else(|| panic!("no message line after {l:?}: {next:?}"));
            found.push((
                c[1].to_string(),
                c[2].to_string(),
                c[3].to_string(),
                c.get(4).map(|m| m.as_str().to_string()),
                msg[1].to_string(),
            ));
        }
    }
    let ar = found
        .iter()
        .find(|f| f.1 == "assertion-reduction")
        .unwrap_or_else(|| panic!("{found:?}\n{}", run.stdout));
    assert_eq!(ar.0, "error");
    assert_eq!(ar.2, "tests/a.rs");
    assert_eq!(ar.3.as_deref(), Some("2"));
    assert!(ar.4.contains("adds"), "{ar:?}");
    assert!(
        !run.stdout.contains('\u{1b}'),
        "NO_COLOR must keep the output plain for the pattern"
    );
}

#[test]
fn the_renovate_preset_groups_every_pin_and_reads_the_documented_gitlab_include() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let preset: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("renovate/discipline.json")).unwrap(),
    )
    .unwrap();
    assert!(preset["extends"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e == ":enablePreCommit"));

    // Every place discipline is pinned is in the one group, and nothing else is.
    let rule = &preset["packageRules"][0];
    assert_eq!(rule["groupName"], "discipline");
    assert_eq!(rule["pinDigests"], true);
    let names = rule["matchPackageNames"].as_array().unwrap();
    let matches = |dep: &str| {
        names.iter().any(|n| {
            let n = n.as_str().unwrap();
            match n.strip_prefix('/').and_then(|r| r.strip_suffix('/')) {
                Some(re) => regex::Regex::new(re).unwrap().is_match(dep),
                None => n == dep,
            }
        })
    };
    for dep in [
        "orieg/discipline",
        "https://github.com/orieg/discipline",
        "https://github.com/orieg/discipline.git",
        "ghcr.io/orieg/discipline",
    ] {
        assert!(matches(dep), "{dep} is not in the discipline group");
    }
    for other in [
        "orieg/other",
        "actions/checkout",
        "ghcr.io/orieg/disciplinex",
    ] {
        assert!(!matches(other), "{other} is in the discipline group");
    }

    // The GitLab include pattern finds the current release in the documented snippet.
    let manager = &preset["customManagers"][0];
    assert_eq!(manager["datasourceTemplate"], "github-tags");
    let pattern = regex::Regex::new(manager["matchStrings"][0].as_str().unwrap()).unwrap();
    let docs = std::fs::read_to_string(root.join("docs/CONFIGURATION.md")).unwrap();
    let found: Vec<String> = pattern
        .captures_iter(&docs)
        .map(|c| c["currentValue"].to_string())
        .collect();
    assert!(!found.is_empty(), "no GitLab include in the docs");
    let current = format!("v{}", env!("CARGO_PKG_VERSION"));
    assert!(
        found.iter().all(|v| *v == current),
        "{found:?} vs {current}"
    );
    let files: Vec<&str> = manager["managerFilePatterns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().trim_matches('/'))
        .collect();
    let file_matches = |f: &str| {
        files
            .iter()
            .any(|p| regex::Regex::new(p).unwrap().is_match(f))
    };
    assert!(file_matches(".gitlab-ci.yml") && file_matches("ci/.gitlab/discipline.yaml"));
    assert!(!file_matches("README.md"));
}
