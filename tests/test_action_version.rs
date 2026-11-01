//! The binary version `action.yml` resolves from its inputs and its own ref: the lines
//! between its `version-resolution` markers, run under bash as the action runs them.

mod common;

fn resolution_block() -> String {
    let action = std::fs::read_to_string("action.yml").unwrap();
    let begin = action.find("# version-resolution:begin").unwrap();
    let end = action.find("# version-resolution:end").unwrap();
    action[begin..end].to_string()
}

fn resolve(input_version: &str, action_ref: &str, action_path: Option<&str>) -> String {
    let script = format!("{}\nprintf '%s' \"${{version}}\"\n", resolution_block());
    let mut cmd = common::script_command("bash");
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

/// One run of the install step's lines from `provenance:begin` through `probe:end`: the
/// provenance decision, the extraction and the `--version` probe of the binary.
struct Install<'a> {
    server: &'a str,
    download_url: &'a str,
    version: &'a str,
    /// Exit status of a stub `gh` that records its arguments; `None` puts no `gh` on PATH.
    gh: Option<i32>,
    /// Whether the release store serves `discipline.intoto.jsonl`.
    bundle: bool,
    require_provenance: &'a str,
    trusted_root: &'a str,
}

const STORE: &str = "https://github.com/orieg/discipline/releases";

impl Default for Install<'_> {
    fn default() -> Self {
        Install {
            server: "https://github.com",
            download_url: STORE,
            version: "v0.14.4",
            gh: Some(0),
            bundle: false,
            require_provenance: "",
            trusted_root: "",
        }
    }
}

struct Ran {
    ok: bool,
    /// The step's stdout followed by the step outputs it wrote.
    out: String,
    /// The arguments `gh` ran with, empty when it did not run.
    gh_args: String,
    /// `GH_TOKEN` as `gh` saw it (`unset` when absent).
    gh_token: String,
    /// What the canary binary wrote when it ran, `None` when it never ran.
    canary: Option<String>,
}

/// A directory holding only the programs the install step's lines call, so whether
/// `gh` is on PATH is the test's choice and not the machine's.
fn tool_dir(dir: &std::path::Path) {
    let path = std::env::var_os("PATH").unwrap();
    for tool in ["bash", "env", "tar", "gzip", "cp", "tail"] {
        let found = std::env::split_paths(&path)
            .map(|d| d.join(tool))
            .find(|p| p.is_file())
            .unwrap_or_else(|| panic!("{tool} is not on PATH"));
        std::os::unix::fs::symlink(found, dir.join(tool)).unwrap();
    }
}

fn executable(path: &std::path::Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn install(c: &Install) -> Ran {
    let action = std::fs::read_to_string("action.yml").unwrap();
    let begin = action.find("# provenance:begin").unwrap();
    let end = action.find("# probe:end").unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    tool_dir(&bin);
    let gh_args = tmp.path().join("gh.args");
    let gh_token = tmp.path().join("gh.token");
    if let Some(code) = c.gh {
        executable(
            &bin.join("gh"),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\nprintf '%s' \"${{GH_TOKEN-unset}}\" > '{}'\necho 'stub failure' >&2\nexit {code}\n",
                gh_args.display(),
                gh_token.display()
            ),
        );
    }
    // The archive holds a canary `discipline` that records that it ran, and with which token.
    let canary = tmp.path().join("canary");
    let src = tmp.path().join("src");
    std::fs::create_dir(&src).unwrap();
    executable(
        &src.join("discipline"),
        &format!(
            "#!/bin/sh\nprintf 'ran GH_TOKEN=%s' \"${{GH_TOKEN-unset}}\" > '{}'\n",
            canary.display()
        ),
    );
    let dir = tmp.path().join("dir");
    std::fs::create_dir(&dir).unwrap();
    let archive = "discipline-x86_64-unknown-linux-musl.tar.gz";
    assert!(std::process::Command::new("tar")
        .arg("-czf")
        .arg(dir.join(archive))
        .arg("-C")
        .arg(&src)
        .arg("discipline")
        .status()
        .unwrap()
        .success());
    // The release store `fetch` reads from.
    let store = tmp.path().join("store");
    std::fs::create_dir(&store).unwrap();
    if c.bundle {
        std::fs::write(store.join("discipline.intoto.jsonl"), "{}\n").unwrap();
    }
    // `if true; then` stands for the `else` of `binary_path` that the `fi` closes.
    let script = format!(
        "set -euo pipefail\nfail() {{ echo \"FAIL: $*\"; exit 1; }}\nfetch() {{ cp \"$1\" \"$2\"; }}\nif true; then\n{}",
        &action[begin..end]
    );
    let out = common::script_command("bash")
        .arg("-c")
        .arg(script)
        .env("PATH", &bin)
        .env("GITHUB_SERVER_URL", c.server)
        .env("INPUT_DOWNLOAD_URL", c.download_url)
        .env("INPUT_REQUIRE_PROVENANCE", c.require_provenance)
        .env("INPUT_TRUSTED_ROOT", c.trusted_root)
        .env("GH_TOKEN", "token")
        .env("version", c.version)
        .env("base", &store)
        .env("dir", &dir)
        .env("archive", archive)
        .env("GITHUB_OUTPUT", tmp.path().join("outputs"))
        .output()
        .unwrap();
    let outputs = std::fs::read_to_string(tmp.path().join("outputs")).unwrap_or_default();
    Ran {
        ok: out.status.success(),
        out: format!(
            "{}{}{outputs}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        gh_args: std::fs::read_to_string(&gh_args).unwrap_or_default(),
        gh_token: std::fs::read_to_string(&gh_token).unwrap_or_default(),
        canary: std::fs::read_to_string(&canary).ok(),
    }
}

#[test]
fn a_downloaded_archive_must_carry_the_release_workflow_attestation_for_its_tag() {
    let r = install(&Install::default());
    assert!(r.ok, "{}", r.out);
    assert!(
        r.gh_args.starts_with("attestation verify "),
        "{}",
        r.gh_args
    );
    assert!(!r.gh_args.contains("--bundle"), "{}", r.gh_args);
    // A verified attestation says so in the step's outputs (#966).
    assert!(r.out.contains("provenance=verified"), "{}", r.out);
    assert!(
        r.gh_args.contains("--repo orieg/discipline"),
        "{}",
        r.gh_args
    );
    assert!(
        r.gh_args
            .contains("--signer-workflow orieg/discipline/.github/workflows/release.yml"),
        "{}",
        r.gh_args
    );
    assert!(
        r.gh_args.contains("--source-ref refs/tags/v0.14.4"),
        "{}",
        r.gh_args
    );

    // `latest` has no tag to bind to; the signer is still checked.
    let r = install(&Install {
        version: "",
        ..Install::default()
    });
    assert!(r.ok, "{}", r.out);
    assert!(!r.gh_args.contains("--source-ref"), "{}", r.gh_args);

    // An attestation that does not verify stops the install before the binary runs.
    let r = install(&Install {
        gh: Some(1),
        ..Install::default()
    });
    assert!(!r.ok);
    assert!(r.out.contains("did not verify: stub failure"), "{}", r.out);
    assert_eq!(r.canary, None);
}

#[test]
fn provenance_is_skipped_with_a_notice_off_github_or_off_the_default_store() {
    for (server, store, gh, level) in [
        ("https://gitea.example", STORE, Some(1), "::notice"),
        (
            "https://github.com",
            "http://127.0.0.1:8765",
            Some(1),
            "::warning",
        ),
        // On github.com without gh nothing is checked: a warning, not a notice (#987).
        ("https://github.com", STORE, None, "::warning"),
    ] {
        let r = install(&Install {
            server,
            download_url: store,
            gh,
            ..Install::default()
        });
        assert!(r.ok, "{}", r.out);
        assert!(r.gh_args.is_empty(), "gh ran: {}", r.gh_args);
        assert!(
            r.out.contains(&format!(
                "{level} title=discipline install::build provenance not verified"
            )),
            "{}",
            r.out
        );
        assert!(r.out.contains("provenance=unverified"), "{}", r.out);
        // Unless required, an unverified binary still installs as before.
        assert!(r.canary.is_some(), "{}", r.out);
    }
}

#[test]
fn the_version_probe_runs_without_the_job_token() {
    let r = install(&Install::default());
    assert_eq!(r.canary.as_deref(), Some("ran GH_TOKEN=unset"), "{}", r.out);
}

#[test]
fn require_provenance_refuses_an_unverified_binary_before_it_runs() {
    for (server, gh) in [
        ("https://gitea.example", Some(0)),
        ("https://github.com", None),
    ] {
        let r = install(&Install {
            server,
            gh,
            require_provenance: "true",
            ..Install::default()
        });
        assert!(!r.ok, "{}", r.out);
        assert!(r.out.contains("FAIL: require_provenance:"), "{}", r.out);
        assert!(r.out.contains("the binary was not run"), "{}", r.out);
        assert_eq!(r.canary, None, "the binary ran: {}", r.out);
        assert!(r.gh_args.is_empty(), "gh ran: {}", r.gh_args);
    }
    // A verified install passes the same requirement.
    let r = install(&Install {
        require_provenance: "true",
        ..Install::default()
    });
    assert!(r.ok, "{}", r.out);
    assert!(r.canary.is_some());
}

#[test]
fn a_mirror_install_verifies_the_release_sigstore_bundle_without_the_forge_token() {
    let mirror = Install {
        server: "https://gitea.example",
        download_url: "https://mirror.example/discipline",
        bundle: true,
        require_provenance: "true",
        ..Install::default()
    };
    let r = install(&mirror);
    assert!(r.ok, "{}", r.out);
    assert!(r.out.contains("provenance=verified"), "{}", r.out);
    assert!(
        r.gh_args.starts_with("attestation verify "),
        "{}",
        r.gh_args
    );
    assert!(
        r.gh_args.contains("--bundle ") && r.gh_args.contains("/discipline.intoto.jsonl"),
        "{}",
        r.gh_args
    );
    assert!(
        r.gh_args.contains("--repo orieg/discipline"),
        "{}",
        r.gh_args
    );
    assert!(
        r.gh_args
            .contains("--signer-workflow orieg/discipline/.github/workflows/release.yml"),
        "{}",
        r.gh_args
    );
    assert!(
        r.gh_args.contains("--source-ref refs/tags/v0.14.4"),
        "{}",
        r.gh_args
    );
    // No root came with the mirror: gh uses Sigstore's own.
    assert!(
        !r.gh_args.contains("--custom-trusted-root"),
        "{}",
        r.gh_args
    );
    assert_eq!(r.gh_token, "unset");
    assert!(r.canary.is_some());

    // A bundle that does not verify stops the install, required or not.
    for require in ["true", ""] {
        let r = install(&Install {
            gh: Some(1),
            require_provenance: require,
            ..mirror
        });
        assert!(!r.ok, "{}", r.out);
        assert!(r.out.contains("Sigstore bundle"), "{}", r.out);
        assert!(r.out.contains("did not verify: stub failure"), "{}", r.out);
        assert_eq!(r.canary, None);
    }

    // Without a tag, any older signed release would pass: refused before gh runs.
    let r = install(&Install {
        version: "",
        ..mirror
    });
    assert!(!r.ok, "{}", r.out);
    assert!(r.out.contains("set the version input"), "{}", r.out);
    assert!(r.gh_args.is_empty(), "gh ran: {}", r.gh_args);

    // The operator's trusted root, for a runner with no network.
    let root = tempfile::NamedTempFile::new().unwrap();
    let root_path = root.path().to_str().unwrap();
    let r = install(&Install {
        trusted_root: root_path,
        ..mirror
    });
    assert!(r.ok, "{}", r.out);
    assert!(
        r.gh_args
            .contains(&format!("--custom-trusted-root {root_path}")),
        "{}",
        r.gh_args
    );
    let r = install(&Install {
        trusted_root: "/nonexistent/trusted_root.jsonl",
        ..mirror
    });
    assert!(!r.ok, "{}", r.out);
    assert!(r.out.contains("is not a readable file"), "{}", r.out);
}

/// Runs `install.sh --verify-provenance` against a `file://` release `v0.0.0-test` whose
/// store serves a Sigstore bundle, with a stub `gh` exiting `gh_exit` first on PATH.
/// Returns (exit status, stderr, recorded `gh` arguments, whether the binary was installed).
fn install_sh_verified(extra: &[&str], gh_exit: i32) -> (bool, String, String, bool) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store/download/v0.0.0-test");
    std::fs::create_dir_all(&store).unwrap();
    executable(
        &dir.path().join("discipline"),
        "#!/bin/sh\necho 'discipline 0.0.0-test'\n",
    );
    let triple = match (std::env::consts::ARCH, std::env::consts::OS) {
        ("x86_64", "linux") => "x86_64-unknown-linux-musl",
        ("aarch64", "linux") => "aarch64-unknown-linux-musl",
        ("x86_64", "macos") => "x86_64-apple-darwin",
        ("aarch64", "macos") => "aarch64-apple-darwin",
        (a, o) => panic!("unsupported test target: {a}-{o}"),
    };
    let archive = format!("discipline-{triple}.tar.gz");
    let archive_path = store.join(&archive);
    assert!(common::script_command("tar")
        .arg("-czf")
        .arg(&archive_path)
        .arg("-C")
        .arg(dir.path())
        .arg("discipline")
        .status()
        .unwrap()
        .success());
    let sha = common::script_command("sh")
        .arg("-c")
        .arg(format!(
            "(sha256sum '{0}' 2>/dev/null || shasum -a 256 '{0}') | awk '{{print $1}}'",
            archive_path.display()
        ))
        .output()
        .unwrap();
    std::fs::write(
        store.join("SHA256SUMS"),
        format!(
            "{}  {archive}\n",
            String::from_utf8(sha.stdout).unwrap().trim()
        ),
    )
    .unwrap();
    std::fs::write(store.join("discipline.intoto.jsonl"), "{}\n").unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let gh_args = dir.path().join("gh.args");
    executable(
        &bin.join("gh"),
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\nexit {gh_exit}\n",
            gh_args.display()
        ),
    );
    let dest = dir.path().join("dest");
    let out = common::script_command("bash")
        .arg(root.join("install.sh"))
        .arg("--to")
        .arg(&dest)
        .arg("--download-url")
        .arg(format!("file://{}/store", dir.path().display()))
        .arg("--verify-provenance")
        .args(extra)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        std::fs::read_to_string(&gh_args).unwrap_or_default(),
        dest.join("discipline").exists(),
    )
}

#[test]
fn install_sh_verify_provenance_checks_the_sigstore_bundle_before_installing() {
    let (ok, err, args, installed) = install_sh_verified(&["--version", "v0.0.0-test"], 0);
    assert!(ok, "{err}");
    assert!(installed);
    assert!(args.starts_with("attestation verify "), "{args}");
    assert!(args.contains("/discipline.intoto.jsonl"), "{args}");
    assert!(args.contains("--bundle "), "{args}");
    assert!(args.contains("--repo orieg/discipline"), "{args}");
    assert!(
        args.contains("--signer-workflow orieg/discipline/.github/workflows/release.yml"),
        "{args}"
    );
    assert!(
        args.contains("--source-ref refs/tags/v0.0.0-test"),
        "{args}"
    );

    let (ok, err, _, installed) = install_sh_verified(&["--version", "v0.0.0-test"], 1);
    assert!(!ok);
    assert!(err.contains("did not verify"), "{err}");
    assert!(!installed, "an unverified binary was installed");

    // Without a release tag any older signed release would pass.
    let (ok, err, args, installed) = install_sh_verified(&[], 0);
    assert!(!ok);
    assert!(err.contains("--verify-provenance needs --version"), "{err}");
    assert!(args.is_empty() && !installed);

    // An offline trusted root is passed through.
    let root = tempfile::NamedTempFile::new().unwrap();
    let root_path = root.path().to_str().unwrap();
    let (ok, err, args, _) = install_sh_verified(
        &["--version", "v0.0.0-test", "--trusted-root", root_path],
        0,
    );
    assert!(ok, "{err}");
    assert!(
        args.contains(&format!("--custom-trusted-root {root_path}")),
        "{args}"
    );

    // The removed GPG path, which could never succeed, says what replaced it.
    let (ok, err, _, _) = install_sh_verified(&["--gpg-key", "key.asc"], 0);
    assert!(!ok);
    assert!(err.contains("use --verify-provenance"), "{err}");
}

/// Runs the lines between the `download-url` markers of `action.yml` for `url`, with
/// `extra` set in the environment. Returns (exit status, stdout).
fn download_url_check(url: &str, extra: &[(&str, &str)]) -> (bool, String) {
    let action = std::fs::read_to_string("action.yml").unwrap();
    let begin = action.find("# download-url:begin").unwrap();
    let end = action.find("# download-url:end").unwrap();
    let script = format!(
        "set -euo pipefail\nfail() {{ echo \"FAIL: $*\"; exit 1; }}\n{}",
        &action[begin..end]
    );
    let mut cmd = common::script_command("bash");
    cmd.arg("-c")
        .arg(script)
        .env("INPUT_DOWNLOAD_URL", url)
        .env_remove("DISCIPLINE_INSTALL_ALLOW_INSECURE_HTTP");
    for (k, v) in extra {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn the_action_refuses_a_cleartext_download_url_as_install_sh_does() {
    for ok in [
        STORE,
        "https://mirror.example.com/discipline",
        "file:///srv/mirror",
        "http://127.0.0.1:8765",
        "http://localhost/releases",
        "http://[::1]:9000",
    ] {
        let (pass, out) = download_url_check(ok, &[]);
        assert!(pass, "{ok}: {out}");
    }
    for bad in [
        "http://mirror.example.com/discipline",
        "http://127.0.0.1.example.com/x",
        "http://user@127.0.0.1/x",
        "ftp://mirror.example.com/x",
    ] {
        let (pass, out) = download_url_check(bad, &[]);
        assert!(!pass, "{bad} was accepted: {out}");
        assert!(out.contains("FAIL:") && out.contains(bad), "{bad}: {out}");
    }
    // The same opt-in as install.sh accepts a cleartext mirror on purpose.
    let (pass, out) = download_url_check(
        "http://mirror.example.com/discipline",
        &[("DISCIPLINE_INSTALL_ALLOW_INSECURE_HTTP", "1")],
    );
    assert!(pass, "{out}");
}

#[test]
fn install_tool_script_invokes_correct_version_command() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let stub_bin = dir.path().join("stubs");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&stub_bin).unwrap();
    let log = dir.path().join("invocations.log");

    std::fs::write(
        stub_bin.join("curl"),
        "#!/bin/sh\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in\n    --output) touch \"$2\"; shift 2 ;;\n    *) shift ;;\n  esac\ndone\nexit 0\n",
    )
    .unwrap();
    // Read the checksum line as the real tool does: a stub that exits first leaves
    // `echo ... | sha256sum --check` to die of SIGPIPE under `pipefail`.
    std::fs::write(
        stub_bin.join("sha256sum"),
        "#!/bin/sh\ncat >/dev/null\nexit 0\n",
    )
    .unwrap();
    std::fs::write(
        stub_bin.join("tar"),
        format!(
            "#!/bin/sh\noutdir=\"\"\ntarget=\"\"\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in\n    -C) outdir=\"$2\"; shift 2 ;;\n    *) target=\"$1\"; shift ;;\n  esac\ndone\ntool=\"$(basename \"$target\")\"\nmkdir -p \"$outdir\"\nprintf '#!/bin/sh\\necho \"ARGS: $*\" >> \"%s\"\\n' \"{}\" > \"$outdir/$tool\"\nchmod +x \"$outdir/$tool\"\nexit 0\n",
            log.display()
        ),
    )
    .unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for stub in ["curl", "sha256sum", "tar"] {
            std::fs::set_permissions(stub_bin.join(stub), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
    }

    let path = format!("{}:{}", stub_bin.display(), std::env::var("PATH").unwrap());

    for (tool, expected_args) in [
        ("actionlint", "--version"),
        ("act", "--version"),
        ("scorecard", "version"),
        ("cargo-cyclonedx", "cyclonedx --version"),
    ] {
        let out = common::script_command("bash")
            .arg("tests/action/install-tool.sh")
            .arg(tool)
            .env("PATH", &path)
            .env("HOME", &home)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "failed for {tool}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let recorded = std::fs::read_to_string(&log).unwrap();
        let last_line = recorded.lines().last().unwrap();
        assert_eq!(
            last_line,
            &format!("ARGS: {expected_args}"),
            "wrong invocation for tool {tool}"
        );
    }
}

/// The Forgejo self-test workflow is a rename of the Gitea one: comments and
/// `name:` fields name the forge, and everything else (the triggers under `on:`,
/// `permissions:`, and each job's steps, `uses`, `run`, `env`, `with`) must stay
/// identical so both forges prove the same action has no host-only dependency.
/// `ci.yml` runs the Gitea copy under act.
/// Killed mutant: any step edited in only one copy. The controls below show a
/// differing trigger or permission is a difference and a differing name is not.
#[test]
fn forgejo_and_gitea_selftest_workflows_run_the_same_jobs() {
    fn modulo_names(what: &str, text: &str) -> serde_yaml::Value {
        let mut doc: serde_yaml::Value =
            serde_yaml::from_str(text).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert!(doc.get("jobs").is_some(), "{what} has no jobs");
        strip_names(&mut doc);
        doc
    }

    fn workflow(path: &str) -> serde_yaml::Value {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let text =
            std::fs::read_to_string(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        modulo_names(path, &text)
    }

    fn strip_names(value: &mut serde_yaml::Value) {
        match value {
            serde_yaml::Value::Mapping(mapping) => {
                mapping.remove(serde_yaml::Value::String("name".to_string()));
                for (_, v) in mapping.iter_mut() {
                    strip_names(v);
                }
            }
            serde_yaml::Value::Sequence(items) => {
                for v in items {
                    strip_names(v);
                }
            }
            _ => {}
        }
    }

    let sample = |name: &str, trigger: &str, permission: &str| {
        modulo_names(
            "sample",
            &format!(
                "name: {name}\non:\n  {trigger}:\npermissions:\n  contents: {permission}\njobs:\n  a:\n    steps:\n      - name: {name}\n        run: echo\n"
            ),
        )
    };
    let reference = sample("one", "push", "read");
    assert_eq!(sample("two", "push", "read"), reference);
    assert_ne!(sample("one", "pull_request", "read"), reference);
    assert_ne!(sample("one", "push", "write"), reference);

    assert_eq!(
        workflow(".forgejo/workflows/action-selftest.yml"),
        workflow(".gitea/workflows/action-selftest.yml"),
        "Forgejo and Gitea self-test workflows drifted apart: mirror the change to both"
    );
}

#[test]
fn step10_action_and_installers_use_hardened_tempdirs_fetch_tar_and_container_home() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let action = std::fs::read_to_string(root.join("action.yml")).unwrap();
    assert!(
        action.contains("umask 077 && mktemp -d"),
        "action.yml must create temp directories with umask 077 && mktemp -d"
    );
    assert!(
        action.contains("--proto '=https' --tlsv1.2 --max-redirs 3"),
        "action.yml curl fetch must enforce HTTPS, TLSv1.2+, and bounded redirects"
    );
    assert!(
        action.contains("--no-same-owner --no-same-permissions"),
        "action.yml tar extraction must pass --no-same-owner --no-same-permissions"
    );
    assert!(
        action.contains("protocol.file.allow=never")
            && action.contains("core.hooksPath=/dev/null")
            && action.contains("core.fsmonitor=false"),
        "action.yml git fetch must pass hardened git -c overrides"
    );

    let install_sh = std::fs::read_to_string(root.join("install.sh")).unwrap();
    assert!(
        install_sh.contains("--no-same-owner --no-same-permissions"),
        "install.sh must pass --no-same-owner --no-same-permissions to tar"
    );
    assert!(
        install_sh.contains("--proto '=https' --tlsv1.2 --max-redirs 3"),
        "install.sh must pass --proto '=https' --tlsv1.2 --max-redirs 3 to curl"
    );
    assert!(
        install_sh.contains("install -m 0755") && install_sh.contains("mv -f"),
        "install.sh must install atomically via install + mv -f"
    );

    // Run install.sh against a fake release with a non-default DOWNLOAD_URL and verify warning + atomic install
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store/download/v0.0.0-test");
    std::fs::create_dir_all(&store).unwrap();
    let bin_src = dir.path().join("discipline");
    std::fs::write(&bin_src, "#!/bin/sh\necho 'discipline 0.0.0-test'\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin_src, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let triple = match (std::env::consts::ARCH, std::env::consts::OS) {
        ("x86_64", "linux") => "x86_64-unknown-linux-musl",
        ("aarch64", "linux") => "aarch64-unknown-linux-musl",
        ("x86_64", "macos") => "x86_64-apple-darwin",
        ("aarch64", "macos") => "aarch64-apple-darwin",
        (a, o) => panic!("unsupported test target: {a}-{o}"),
    };
    let archive_name = format!("discipline-{triple}.tar.gz");
    let archive_path = store.join(&archive_name);
    let tar_status = common::script_command("tar")
        .arg("-czf")
        .arg(&archive_path)
        .arg("-C")
        .arg(dir.path())
        .arg("discipline")
        .status()
        .unwrap();
    assert!(tar_status.success());
    let sha_out = common::script_command("sh")
        .arg("-c")
        .arg(format!(
            "(sha256sum '{}' 2>/dev/null || shasum -a 256 '{}') | awk '{{print $1}}'",
            archive_path.display(),
            archive_path.display()
        ))
        .output()
        .unwrap();
    let digest = String::from_utf8(sha_out.stdout).unwrap();
    std::fs::write(
        store.join("SHA256SUMS"),
        format!("{}  {}\n", digest.trim(), archive_name),
    )
    .unwrap();
    let dest = dir.path().join("dest");
    let out = common::script_command("bash")
        .arg(root.join("install.sh"))
        .arg("--to")
        .arg(&dest)
        .arg("--version")
        .arg("v0.0.0-test")
        .arg("--download-url")
        .arg(format!("file://{}/store", dir.path().display()))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "install.sh failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("warning: using non-default download URL"),
        "expected non-default download URL warning on stderr, got: {stderr}"
    );

    for dockerfile in ["Dockerfile", "Dockerfile.release"] {
        let df = std::fs::read_to_string(root.join(dockerfile)).unwrap();
        assert!(
            df.contains("-h /var/lib/discipline"),
            "{dockerfile} must set discipline user home to /var/lib/discipline, not /workspace"
        );
        assert!(
            !df.contains("VOLUME [\"/workspace\"]"),
            "{dockerfile} must not declare VOLUME [\"/workspace\"]"
        );
    }
}

#[test]
fn step11_build_repo_rejects_symlinks_purges_existing_outputs_and_homebrew_pins_host_key() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = tempfile::tempdir().unwrap();
    let artifacts = dir.path().join("artifacts");
    let docs = dir.path().join("docs");
    std::fs::create_dir_all(&artifacts).unwrap();
    std::fs::create_dir_all(docs.join("apt")).unwrap();

    // 1. Pre-existing poisoned file in docs/apt must be purged by build_repo.py
    let poisoned = docs.join("apt/poisoned.txt");
    std::fs::write(&poisoned, "stale artifact").unwrap();

    let out = common::script_command("python3")
        .arg(root.join("scripts/build_repo.py"))
        .arg("--artifacts-dir")
        .arg(&artifacts)
        .arg("--docs-dir")
        .arg(&docs)
        .arg("--allow-empty")
        .arg("--unsigned")
        .arg("--version")
        .arg("0.22.0")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "build_repo.py failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !poisoned.exists(),
        "build_repo.py must purge pre-existing docs/apt contents before building"
    );

    // 2. A symlink inside docs_dir must be rejected fail-closed by build_repo.py
    #[cfg(unix)]
    {
        let symlink_path = docs.join("evil-link");
        std::os::unix::fs::symlink("/etc/passwd", &symlink_path).unwrap();
        let bad_out = common::script_command("python3")
            .arg(root.join("scripts/build_repo.py"))
            .arg("--artifacts-dir")
            .arg(&artifacts)
            .arg("--docs-dir")
            .arg(&docs)
            .arg("--allow-empty")
            .arg("--unsigned")
            .arg("--version")
            .arg("0.22.0")
            .output()
            .unwrap();
        assert!(
            !bad_out.status.success(),
            "build_repo.py must reject symlinks inside --docs-dir"
        );
    }

    // 3. update_homebrew_formula.py must pin GitHub's SSH host key and use StrictHostKeyChecking=yes
    let hb = std::fs::read_to_string(root.join("scripts/update_homebrew_formula.py")).unwrap();
    assert!(
        !hb.contains("StrictHostKeyChecking=accept-new"),
        "update_homebrew_formula.py must not use StrictHostKeyChecking=accept-new"
    );
    assert!(
        hb.contains("StrictHostKeyChecking=yes") && hb.contains("UserKnownHostsFile="),
        "update_homebrew_formula.py must enforce StrictHostKeyChecking=yes with a pinned UserKnownHostsFile"
    );

    // 4. pages.yml site job must run step-security/harden-runner
    let pages = std::fs::read_to_string(root.join(".github/workflows/pages.yml")).unwrap();
    let harden_count = pages.matches("step-security/harden-runner@").count();
    assert!(
        harden_count >= 3,
        "pages.yml must run step-security/harden-runner in all three jobs (site, sign, deploy), found {harden_count}"
    );
}

#[test]
fn step21_container_images_scope_safe_directory_and_defer_workspace_trust_to_entrypoint() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for dockerfile in ["Dockerfile", "Dockerfile.release"] {
        let df = std::fs::read_to_string(root.join(dockerfile)).unwrap();
        assert!(
            !df.contains("safe.directory '*'"),
            "{dockerfile} must scope system safe.directory to /workspace instead of '*'"
        );
        assert!(
            df.contains("git config --system --add safe.directory /workspace"),
            "{dockerfile} must set git config --system --add safe.directory /workspace"
        );
        assert!(
            !df.contains("ENV DISCIPLINE_TRUST_WORKSPACE=1"),
            "{dockerfile} must not set global ENV DISCIPLINE_TRUST_WORKSPACE=1"
        );
        assert!(
            df.contains("USER 10001:10001"),
            "{dockerfile} must run as non-root USER 10001:10001"
        );
    }

    let entrypoint = root.join("packaging/docker/docker-entrypoint.sh");
    let ep_text = std::fs::read_to_string(&entrypoint).unwrap();
    assert!(
        ep_text.contains("DISCIPLINE_TRUST_WORKSPACE=1"),
        "docker-entrypoint.sh must conditionally export DISCIPLINE_TRUST_WORKSPACE=1 for container workspaces"
    );

    // Execute docker-entrypoint.sh in an arbitrary non-CI directory and verify DISCIPLINE_TRUST_WORKSPACE is not forced on.
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let stub = bin.join("discipline");
    std::fs::write(
        &stub,
        "#!/bin/sh\nprintf 'TRUST=%s\\n' \"${DISCIPLINE_TRUST_WORKSPACE:-unset}\"\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let out_non_ci = common::script_command("sh")
        .arg(&entrypoint)
        .arg("check")
        .current_dir(dir.path())
        .env("PATH", &path)
        .env("HOME", dir.path())
        .env_remove("DISCIPLINE_TRUST_WORKSPACE")
        .env_remove("CI")
        .env_remove("GITHUB_ACTIONS")
        .env_remove("GITLAB_CI")
        .env_remove("GITEA_ACTIONS")
        .env_remove("FORGEJO_ACTIONS")
        .output()
        .unwrap();
    assert!(out_non_ci.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out_non_ci.stdout).trim(),
        "TRUST=unset",
        "entrypoint outside /workspace without CI env must leave DISCIPLINE_TRUST_WORKSPACE unset"
    );

    // Execute docker-entrypoint.sh with CI=true and verify DISCIPLINE_TRUST_WORKSPACE=1 is exported.
    let out_ci = common::script_command("sh")
        .arg(&entrypoint)
        .arg("check")
        .current_dir(dir.path())
        .env("PATH", &path)
        .env("HOME", dir.path())
        .env_remove("DISCIPLINE_TRUST_WORKSPACE")
        .env("CI", "true")
        .output()
        .unwrap();
    assert!(out_ci.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out_ci.stdout).trim(),
        "TRUST=1",
        "entrypoint with CI=true must export DISCIPLINE_TRUST_WORKSPACE=1"
    );
}

#[test]
fn step22_fuzz_workflow_and_manifest_register_forge_and_smuggling_target() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let fuzz_toml = std::fs::read_to_string(root.join("fuzz/Cargo.toml")).unwrap();
    assert!(
        fuzz_toml.contains("name = \"forge_and_smuggling\""),
        "fuzz/Cargo.toml must register forge_and_smuggling bin target"
    );
    let fuzz_wf = std::fs::read_to_string(root.join(".github/workflows/fuzz.yml")).unwrap();
    assert!(
        fuzz_wf.contains("- forge_and_smuggling"),
        ".github/workflows/fuzz.yml matrix must include forge_and_smuggling"
    );
    let target_src = std::fs::read_to_string(root.join("fuzz/fuzz_targets/forge_and_smuggling.rs"))
        .expect("fuzz/fuzz_targets/forge_and_smuggling.rs must exist");
    assert!(
        target_src.contains("parse_remote")
            && target_src.contains("ssh_hostname")
            && target_src.contains("text_classes"),
        "forge_and_smuggling.rs must fuzz parse_remote, ssh_hostname, and instruction_smuggling::text_classes"
    );
}

#[test]
fn step26_container_and_infra_hardening_enforces_cap_drop_seccomp_safe_dir_and_gpg_install() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

    // 1. ci.yml docker-smoke must drop capabilities and disable privilege escalation
    let ci_yml = std::fs::read_to_string(root.join(".github/workflows/ci.yml")).unwrap();
    assert!(
        ci_yml.contains("--cap-drop=ALL")
            && ci_yml.contains("--security-opt=no-new-privileges:true"),
        "ci.yml docker-smoke must pass --cap-drop=ALL --security-opt=no-new-privileges:true"
    );

    // 2. templates/argo-workflow-template.yaml must define securityContext (cap-drop ALL, seccomp RuntimeDefault, non-root) and pin alpine digest
    let argo = std::fs::read_to_string(root.join("templates/argo-workflow-template.yaml")).unwrap();
    assert!(
        argo.contains("securityContext:")
            && argo.contains("allowPrivilegeEscalation: false")
            && argo.contains("seccompProfile:")
            && argo.contains("type: RuntimeDefault")
            && argo.contains("drop:")
            && argo.contains("alpine:3.21@sha256:"),
        "argo-workflow-template.yaml must enforce non-root securityContext with cap-drop ALL, seccomp RuntimeDefault, and pinned alpine digest"
    );

    // 3. packaging/docker/docker-entrypoint.sh must only add safe.directory when DISCIPLINE_TRUST_WORKSPACE=1
    let entrypoint = root.join("packaging/docker/docker-entrypoint.sh");
    let ep_dir = tempfile::tempdir().unwrap();
    let bin = ep_dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("discipline"), "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            bin.join("discipline"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let out_untrusted = common::script_command("sh")
        .arg(&entrypoint)
        .arg("check")
        .current_dir(ep_dir.path())
        .env("PATH", &path)
        .env("HOME", ep_dir.path())
        .env_remove("DISCIPLINE_TRUST_WORKSPACE")
        .env_remove("CI")
        .env_remove("GITHUB_ACTIONS")
        .env_remove("GITLAB_CI")
        .env_remove("GITEA_ACTIONS")
        .env_remove("FORGEJO_ACTIONS")
        .output()
        .unwrap();
    assert!(out_untrusted.status.success());
    let gitconfig = std::fs::read_to_string(ep_dir.path().join(".gitconfig")).unwrap_or_default();
    assert!(
        !gitconfig.contains("directory ="),
        "docker-entrypoint.sh must not add safe.directory for untrusted non-CI host directories, got: {gitconfig}"
    );

    // 4. install.sh and docs/install.sh must stay identical and reject cleartext non-loopback HTTP DOWNLOAD_URL
    let install_sh = std::fs::read_to_string(root.join("install.sh")).unwrap();
    let docs_install_sh = std::fs::read_to_string(root.join("docs/install.sh")).unwrap();
    assert_eq!(
        install_sh, docs_install_sh,
        "install.sh and docs/install.sh must stay byte-for-byte identical"
    );

    let dest = ep_dir.path().join("dest");
    for bad_url in [
        "http://mirror.example.com/releases",
        "http://localhost:@mirror.example.com/releases",
        "http://127.0.0.1:@mirror.example.com/releases",
    ] {
        let bad_http = common::script_command("bash")
            .arg(root.join("install.sh"))
            .arg("--to")
            .arg(&dest)
            .arg("--download-url")
            .arg(bad_url)
            .output()
            .unwrap();
        assert!(
            !bad_http.status.success(),
            "install.sh must refuse cleartext non-loopback HTTP DOWNLOAD_URL `{bad_url}` by default"
        );
        let bad_http_err = String::from_utf8_lossy(&bad_http.stderr);
        assert!(
            bad_http_err.contains("refusing cleartext HTTP download URL"),
            "expected cleartext HTTP refusal diagnostic for `{bad_url}`, got: {bad_http_err}"
        );
    }

    // 5. docs/CONFIGURATION.md must document scoped safe.directory '/workspace' and --cap-drop=ALL
    let config_doc = std::fs::read_to_string(root.join("docs/CONFIGURATION.md")).unwrap();
    assert!(
        !config_doc.contains("safe.directory '*'"),
        "docs/CONFIGURATION.md must not claim system-wide safe.directory '*'"
    );
    assert!(
        config_doc.contains("--cap-drop=ALL") && config_doc.contains("no-new-privileges:true"),
        "docs/CONFIGURATION.md must document --cap-drop=ALL and no-new-privileges:true for docker run"
    );
}
