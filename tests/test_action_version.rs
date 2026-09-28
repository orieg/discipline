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

/// Runs the lines between the `provenance` markers with a stub `gh` that records its
/// arguments and exits with `gh_exit`. Returns (exit status, stdout, recorded args).
fn provenance(
    server: &str,
    download_url: &str,
    version: &str,
    gh_exit: i32,
) -> (bool, String, String) {
    let action = std::fs::read_to_string("action.yml").unwrap();
    let begin = action.find("# provenance:begin").unwrap();
    let end = action.find("# provenance:end").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let log = dir.path().join("gh.args");
    let stub = bin.join("gh");
    std::fs::write(
        &stub,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\necho 'stub failure' >&2\nexit {gh_exit}\n",
            log.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let script = format!(
        "set -euo pipefail\nfail() {{ echo \"FAIL: $*\"; exit 1; }}\n{}",
        &action[begin..end]
    );
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let out = Command::new("bash")
        .arg("-c")
        .arg(script)
        .env("PATH", path)
        .env("GITHUB_SERVER_URL", server)
        .env("INPUT_DOWNLOAD_URL", download_url)
        .env("GH_TOKEN", "token")
        .env("version", version)
        .env("dir", dir.path())
        .env("archive", "discipline-x86_64-unknown-linux-musl.tar.gz")
        .output()
        .unwrap();
    let args = std::fs::read_to_string(&log).unwrap_or_default();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        args,
    )
}

const STORE: &str = "https://github.com/orieg/discipline/releases";

#[test]
fn a_downloaded_archive_must_carry_the_release_workflow_attestation_for_its_tag() {
    let (ok, _, args) = provenance("https://github.com", STORE, "v0.14.4", 0);
    assert!(ok);
    assert!(args.starts_with("attestation verify "), "{args}");
    assert!(args.contains("--repo orieg/discipline"), "{args}");
    assert!(
        args.contains("--signer-workflow orieg/discipline/.github/workflows/release.yml"),
        "{args}"
    );
    assert!(args.contains("--source-ref refs/tags/v0.14.4"), "{args}");

    // `latest` has no tag to bind to; the signer is still checked.
    let (ok, _, args) = provenance("https://github.com", STORE, "", 0);
    assert!(ok);
    assert!(!args.contains("--source-ref"), "{args}");

    // An attestation that does not verify stops the install.
    let (ok, out, _) = provenance("https://github.com", STORE, "v0.14.4", 1);
    assert!(!ok);
    assert!(out.contains("did not verify: stub failure"), "{out}");
}

#[test]
fn provenance_is_skipped_with_a_notice_off_github_or_off_the_default_store() {
    for (server, store) in [
        ("https://gitea.example", STORE),
        ("https://github.com", "http://127.0.0.1:8765"),
    ] {
        let (ok, out, args) = provenance(server, store, "v0.14.4", 1);
        assert!(ok, "{out}");
        assert!(args.is_empty(), "gh ran: {args}");
        assert!(out.contains("build provenance not verified"), "{out}");
    }
}
