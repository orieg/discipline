//! `.github/workflows/release.yml` publishes the crate through crates.io trusted
//! publishing. Cargo does not exchange a job's OIDC token for a registry token, and
//! the crate accepts no API token, so a `cargo publish` step can only authenticate
//! with the token an earlier `rust-lang/crates-io-auth-action` step of its job returned.

use serde_yaml::Value;

const EXCHANGE_ACTION: &str = "rust-lang/crates-io-auth-action@";

fn release_workflow() -> Value {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".github/workflows/release.yml");
    let text = std::fs::read_to_string(&path).unwrap();
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("release.yml is not YAML: {e}"))
}

/// The step id in `${{ steps.<id>.outputs.token }}`, if `expr` is exactly that.
fn token_output_step(expr: &str) -> Option<&str> {
    expr.trim()
        .strip_prefix("${{")?
        .strip_suffix("}}")?
        .trim()
        .strip_prefix("steps.")?
        .strip_suffix(".outputs.token")
}

/// What is wrong with each `cargo publish` step of `workflow`, and how many there are.
fn publish_step_defects(workflow: &Value) -> (usize, Vec<String>) {
    let mut publishes = 0;
    let mut defects = Vec::new();
    let jobs = workflow["jobs"].as_mapping().expect("release.yml has jobs");
    for (job_name, job) in jobs {
        let job_name = job_name.as_str().unwrap();
        let steps = job["steps"].as_sequence().expect("a job has steps");
        for (index, step) in steps.iter().enumerate() {
            if !step["run"]
                .as_str()
                .is_some_and(|run| run.contains("cargo publish"))
            {
                continue;
            }
            publishes += 1;
            let at = format!("{job_name} step {index}");
            if job["permissions"]["id-token"].as_str() != Some("write") {
                defects.push(format!("{at}: the job lacks `id-token: write`"));
            }
            if job["environment"].as_str() != Some("release") {
                defects.push(format!("{at}: the job lacks `environment: release`"));
            }
            let Some(token) = step["env"]["CARGO_REGISTRY_TOKEN"].as_str() else {
                defects.push(format!("{at}: no CARGO_REGISTRY_TOKEN in the step's env"));
                continue;
            };
            let Some(id) = token_output_step(token) else {
                defects.push(format!(
                    "{at}: CARGO_REGISTRY_TOKEN is `{token}`, not a step's token output"
                ));
                continue;
            };
            let Some(exchange) = steps[..index].iter().find(|s| s["id"].as_str() == Some(id))
            else {
                defects.push(format!("{at}: no earlier step has id `{id}`"));
                continue;
            };
            let uses = exchange["uses"].as_str().unwrap_or_default();
            match uses.strip_prefix(EXCHANGE_ACTION) {
                None => defects.push(format!(
                    "{at}: step `{id}` runs `{uses}`, not the exchange action"
                )),
                Some(pin) if pin.len() != 40 || !pin.bytes().all(|b| b.is_ascii_hexdigit()) => {
                    defects.push(format!(
                        "{at}: the exchange action is pinned to `{pin}`, not a commit"
                    ))
                }
                Some(_) => {}
            }
        }
    }
    (publishes, defects)
}

#[test]
fn every_cargo_publish_step_authenticates_with_an_exchanged_token() {
    let (publishes, defects) = publish_step_defects(&release_workflow());
    assert_ne!(
        publishes, 0,
        "release.yml has no `cargo publish` step for this test to check"
    );
    assert!(defects.is_empty(), "{}", defects.join("\n"));
}

/// The form v0.17.0 shipped, and each way of supplying a token that the crate refuses
/// or that names no exchange, is reported; the exchanged form is not.
#[test]
fn a_publish_step_without_an_exchanged_token_is_reported() {
    let job = |steps: &str| -> Value {
        serde_yaml::from_str(&format!(
            "jobs:\n  promote:\n    environment: release\n    permissions:\n      id-token: write\n    steps:\n{steps}"
        ))
        .unwrap()
    };
    let pin = "c6f97d42243bad5fab37ca0427f495c86d5b1a18";
    let cases = [
        ("      - run: cargo publish --locked\n".to_string(), "no CARGO_REGISTRY_TOKEN"),
        (
            "      - run: cargo publish --locked\n        env:\n          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}\n".to_string(),
            "not a step's token output",
        ),
        (
            "      - run: cargo publish --locked\n        env:\n          CARGO_REGISTRY_TOKEN: ${{ steps.auth.outputs.token }}\n".to_string(),
            "no earlier step has id `auth`",
        ),
        (
            format!("      - run: cargo publish --locked\n        env:\n          CARGO_REGISTRY_TOKEN: ${{{{ steps.auth.outputs.token }}}}\n      - id: auth\n        uses: {EXCHANGE_ACTION}{pin}\n"),
            "no earlier step has id `auth`",
        ),
        (
            format!("      - id: auth\n        uses: {EXCHANGE_ACTION}v1\n      - run: cargo publish --locked\n        env:\n          CARGO_REGISTRY_TOKEN: ${{{{ steps.auth.outputs.token }}}}\n"),
            "not a commit",
        ),
        (
            "      - id: auth\n        uses: actions/checkout@v7\n      - run: cargo publish --locked\n        env:\n          CARGO_REGISTRY_TOKEN: ${{ steps.auth.outputs.token }}\n".to_string(),
            "not the exchange action",
        ),
    ];
    for (steps, want) in &cases {
        let (publishes, defects) = publish_step_defects(&job(steps));
        assert_eq!(publishes, 1, "{steps}");
        assert!(
            defects.len() == 1 && defects[0].contains(want),
            "expected one defect holding `{want}`, got {defects:?} for\n{steps}"
        );
    }

    let good = format!("      - id: auth\n        uses: {EXCHANGE_ACTION}{pin}\n      - run: cargo publish --locked\n        env:\n          CARGO_REGISTRY_TOKEN: ${{{{ steps.auth.outputs.token }}}}\n");
    assert_eq!(publish_step_defects(&job(&good)), (1, Vec::new()));

    let no_permission: Value = serde_yaml::from_str(&format!(
        "jobs:\n  promote:\n    environment: release\n    steps:\n{good}"
    ))
    .unwrap();
    let (_, defects) = publish_step_defects(&no_permission);
    assert!(
        defects.len() == 1 && defects[0].contains("lacks `id-token: write`"),
        "{defects:?}"
    );

    let no_env: Value = serde_yaml::from_str(&format!(
        "jobs:\n  promote:\n    permissions:\n      id-token: write\n    steps:\n{good}"
    ))
    .unwrap();
    let (_, defects) = publish_step_defects(&no_env);
    assert!(
        defects.len() == 1 && defects[0].contains("lacks `environment: release`"),
        "{defects:?}"
    );
}
