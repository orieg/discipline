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
