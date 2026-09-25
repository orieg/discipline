//! The binary version `action.yml` resolves from its inputs and its own ref: the lines
//! between its `version-resolution` markers, run under bash as the action runs them.

use std::process::Command;

fn resolution_block() -> String {
    let action = std::fs::read_to_string("action.yml").unwrap();
    let begin = action.find("# version-resolution:begin").unwrap();
    let end = action.find("# version-resolution:end").unwrap();
    action[begin..end].to_string()
}

fn resolve(input_version: &str, action_ref: &str, action_path: Option<&str>) -> String {
    let script = format!("{}\nprintf '%s' \"${{version}}\"\n", resolution_block());
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(script)
        .env("INPUT_VERSION", input_version)
        .env("ACTION_REF", action_ref)
        .env_remove("GITHUB_ACTION_PATH");
    if let Some(p) = action_path {
        cmd.env("GITHUB_ACTION_PATH", p);
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn a_pinned_action_runs_the_binary_of_its_own_release() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"discipline\"\nversion = \"0.12.3\"\n",
    )
    .unwrap();
    let path = dir.path().to_str().unwrap();
    let sha = "5fa2b71ad3e32176e31d6a3c672c9e5e0c2ed43c";
    // An explicit `version:` wins; an exact tag names itself.
    assert_eq!(resolve("v0.11.0", sha, Some(path)), "v0.11.0");
    assert_eq!(resolve("", "v0.12.2", Some(path)), "v0.12.2");
    // A major tag, a commit SHA and a branch read the action's own Cargo.toml.
    assert_eq!(resolve("", "v0", Some(path)), "v0.12.3");
    assert_eq!(resolve("", sha, Some(path)), "v0.12.3");
    assert_eq!(resolve("", "main", Some(path)), "v0.12.3");
    // With no Cargo.toml beside the action (a vendored copy), the release is `latest`.
    assert_eq!(resolve("", sha, None), "");
}
