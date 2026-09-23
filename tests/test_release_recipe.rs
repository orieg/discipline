//! The release-gate recipes in `docs/CONFIGURATION.md` ("Release Gate: No Source in
//! the Published Package"): each snippet is parsed, its discipline invocation and
//! configuration are run through the binary against a packed npm tarball, and its
//! publish step is checked to depend on the check.

mod common;

use base64::Engine as _;
use common::{Repo, Run};
use discipline::guards::archive_formats::fixtures;
use serde_yaml::Value;

/// The YAML block between `<!-- name -->` and `<!-- /name -->` in the configuration docs.
fn documented_yaml(name: &str) -> Value {
    let doc = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/CONFIGURATION.md"),
    )
    .unwrap();
    let start = doc
        .find(&format!("<!-- {name} -->"))
        .unwrap_or_else(|| panic!("no {name} block"));
    let end = doc.find(&format!("<!-- /{name} -->")).unwrap();
    let yaml = doc[start..end]
        .split_once("```yaml\n")
        .and_then(|(_, rest)| rest.split_once("```"))
        .map(|(y, _)| y)
        .unwrap();
    serde_yaml::from_str(yaml).unwrap_or_else(|e| panic!("{name} is not YAML: {e}\n{yaml}"))
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key]
        .as_str()
        .unwrap_or_else(|| panic!("`{key}` is not a string in {v:?}"))
}

const LEAKING_MAP: &str = r#"{"version":3,"sources":["../src/cli.ts"],"sourcesContent":["export const x = 1;\n"],"mappings":"AAAA"}"#;

/// A repository at a tagged commit with `release/example-1.0.0.tgz` packed.
fn released(files: &[(&str, &[u8])]) -> Repo {
    let repo = Repo::new();
    repo.commit_base("package.json", r#"{"name":"example"}"#, "release 1.0.0");
    let tgz = repo.path().join("release/example-1.0.0.tgz");
    std::fs::create_dir_all(tgz.parent().unwrap()).unwrap();
    std::fs::write(&tgz, fixtures::gzip(&fixtures::tar(files))).unwrap();
    repo
}

/// A package: what it is, its entries, and the exit code the recipe must give.
type Package = (&'static str, Vec<(&'static str, Vec<u8>)>, i32);

/// The three packages every recipe is run against.
fn packages() -> Vec<Package> {
    let manifest = ("package/package.json", br#"{"name":"example"}"#.to_vec());
    let inline = format!(
        "run();\n//# sourceMappingURL=data:application/json;base64,{}\n",
        base64::engine::general_purpose::STANDARD.encode(LEAKING_MAP)
    );
    vec![
        (
            "clean",
            vec![
                manifest.clone(),
                ("package/dist/cli.js", b"run();\n".to_vec()),
                ("package/dist/cli.d.ts", b"export {};\n".to_vec()),
            ],
            0,
        ),
        (
            "inline source map",
            vec![
                manifest.clone(),
                ("package/dist/cli.js", inline.into_bytes()),
            ],
            1,
        ),
        (
            "TypeScript source",
            vec![
                manifest,
                ("package/dist/cli.js", b"run();\n".to_vec()),
                ("package/src/cli.ts", b"run();\n".to_vec()),
            ],
            1,
        ),
    ]
}

/// Runs `args` for each package and checks the verdict, then lifts the inline
/// leak with a directive passed as PR_BODY.
fn assert_recipe_gates(args: &[&str], env: &[(&str, &str)]) {
    for (what, files, code) in packages() {
        let files: Vec<(&str, &[u8])> = files.iter().map(|(n, b)| (*n, b.as_slice())).collect();
        let repo = released(&files);
        let run: Run = repo.run(args, env);
        assert_eq!(run.code, code, "{what}: {}{}", run.stdout, run.stderr);
        let outcome = run.outcome("archive-contents");
        assert!(outcome["enabled"].as_bool().unwrap(), "{what}");
        assert!(outcome["examined"].as_u64().unwrap() >= 2, "{what}");
        if what == "inline source map" {
            assert_eq!(
                run.titles("archive-contents"),
                vec!["Source Leaked In Archive"]
            );
            let mut lifted_env = env.to_vec();
            lifted_env.push((
                "PR_BODY",
                "allow-archive-leak: package/dist/cli.js the inline map is intended for this release",
            ));
            let lifted = repo.run(args, &lifted_env);
            assert_eq!(lifted.code, 0, "{}{}", lifted.stdout, lifted.stderr);
        }
    }
}

#[test]
fn the_documented_github_release_job_checks_the_packed_tarball_before_publishing() {
    let workflow = documented_yaml("release-recipe-github");
    let tags = &workflow["on"]["push"]["tags"];
    assert!(
        tags.as_sequence().is_some_and(|t| !t.is_empty()),
        "runs on tag push"
    );
    let steps = workflow["jobs"]["publish"]["steps"].as_sequence().unwrap();
    let index = |pred: &dyn Fn(&Value) -> bool| steps.iter().position(pred).unwrap();
    let pack = index(&|s| {
        s["run"]
            .as_str()
            .is_some_and(|r| r.contains("npm pack --pack-destination release"))
    });
    let check = index(&|s| {
        s["uses"]
            .as_str()
            .is_some_and(|u| u.starts_with("orieg/discipline@"))
    });
    let publish = index(&|s| {
        s["run"]
            .as_str()
            .is_some_and(|r| r.contains("npm publish release/"))
    });
    assert!(pack < check && check < publish, "pack, check, then publish");
    for step in &steps[check..=publish] {
        assert!(
            step.get("if").is_none(),
            "no step between check and publish runs on failure"
        );
        assert!(step.get("continue-on-error").is_none());
    }
    // Third-party actions are pinned by commit SHA.
    for step in steps {
        if let Some(uses) = step["uses"].as_str() {
            if !uses.starts_with("orieg/discipline@") {
                let sha = uses.split_once('@').unwrap().1;
                assert!(
                    sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()),
                    "{uses}"
                );
            }
        }
    }

    // The action passes these inputs as `--suite`, `--base`, DISCIPLINE_ENABLE
    // and DISCIPLINE_CONFIG_OVERRIDE (action.yml).
    let with = &steps[check]["with"];
    let suite = text(with, "suite");
    let base = text(with, "base_ref");
    let override_toml = text(with, "config_override");
    let parsed: toml::Value = toml::from_str(override_toml).unwrap();
    assert_eq!(
        parsed["gates"]["archive-contents"]["archive_path"].as_str(),
        Some("release/*.tgz")
    );
    assert_recipe_gates(
        &[
            "check", "--format", "json", "--suite", suite, "--base", base,
        ],
        &[
            ("DISCIPLINE_ENABLE", text(with, "enable")),
            ("DISCIPLINE_CONFIG_OVERRIDE", override_toml),
        ],
    );
}

#[test]
fn the_documented_gitlab_release_jobs_check_the_packed_tarball_before_publishing() {
    let pipeline = documented_yaml("release-recipe-gitlab");
    let package = &pipeline["package"];
    let check = &pipeline["check-package"];
    let publish = &pipeline["publish"];
    for job in [package, check, publish] {
        assert_eq!(job["rules"][0]["if"].as_str(), Some("$CI_COMMIT_TAG"));
    }
    assert!(package["script"].as_sequence().unwrap().iter().any(|l| l
        .as_str()
        .unwrap()
        .contains("npm pack --pack-destination release")));
    let needs: Vec<&str> = publish["needs"]
        .as_sequence()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect();
    assert!(needs.contains(&"check-package"), "publish needs the check");
    assert!(check.get("allow_failure").is_none());

    let script = check["script"].as_sequence().unwrap();
    assert_eq!(script.len(), 1);
    let command: Vec<&str> = script[0].as_str().unwrap().split_whitespace().collect();
    assert_eq!(&command[..2], &["discipline", "check"]);
    let mut args = vec!["check", "--format", "json"];
    args.extend(&command[2..]);
    let vars = &check["variables"];
    assert_recipe_gates(
        &args,
        &[
            ("DISCIPLINE_ENABLE", text(vars, "DISCIPLINE_ENABLE")),
            (
                "DISCIPLINE_CONFIG_OVERRIDE",
                text(vars, "DISCIPLINE_CONFIG_OVERRIDE"),
            ),
        ],
    );
}
