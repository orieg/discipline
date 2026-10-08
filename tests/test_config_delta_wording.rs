//! `toolchain-config` and `sandbox-config` run one gate loop (#493). What each hands it
//! shows in the text of its findings and notes: the message of every finding kind, the
//! directive its remedy names, the side an unreadable file is named by, and the note when
//! the change touches no file of the gate. Each is pinned here through the real binary.

mod common;
use common::{Repo, Run};

/// `(code, file, message, remediation)` of the findings `gate` reports in `run`, sorted.
fn wording(run: &Run, gate: &str) -> Vec<(String, String, String, String)> {
    let text = |v: &serde_json::Value, key: &str| v[key].as_str().unwrap().to_string();
    let mut out: Vec<_> = run
        .violations(gate)
        .iter()
        .map(|v| {
            (
                text(v, "code"),
                text(v, "file"),
                text(v, "message"),
                text(v, "remediation"),
            )
        })
        .collect();
    out.sort();
    out
}

fn row(
    code: &str,
    file: &str,
    message: &str,
    remediation: &str,
) -> (String, String, String, String) {
    (
        code.to_string(),
        file.to_string(),
        message.to_string(),
        remediation.to_string(),
    )
}

/// A repository whose base holds `base` and whose change holds `head`; `None` on the head
/// side deletes the file.
fn changed(files: &[(&str, &str, Option<&str>)]) -> Repo {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    for (path, base, _) in files {
        repo.write(path, base);
    }
    repo.commit("chore: base");
    repo.git(&["checkout", "-q", "-B", "work"]);
    for (path, _, head) in files {
        match head {
            Some(head) => repo.write(path, head),
            None => repo.remove(path),
        }
    }
    repo.commit("chore: change");
    repo
}

#[test]
fn toolchain_config_words_each_finding_and_names_its_own_directive() {
    let repo = changed(&[
        (
            "eslint.config.js",
            "export default [];\n",
            Some("export default [{}];\n"),
        ),
        (
            "tsconfig.json",
            "{\"compilerOptions\": {\"strict\": true}}\n",
            Some("{\"compilerOptions\": {\"strict\": false}}\n"),
        ),
        ("mypy.ini", "[mypy]\nstrict = True\n", None),
        ("ruff.toml", "line-length = 100\n", Some("= broken\n")),
        (
            ".eslintrc.json",
            "{\"extends\": [\"eslint:recommended\"]}\n",
            Some("{\"extends\": [\"eslint:recommended\", \"plugin:x/lax\"]}\n"),
        ),
    ]);
    let run = repo.check(&[]);
    assert_eq!(
        wording(&run, "toolchain-config"),
        [
            row(
                "toolchain-config/toolchain-config-change-not-analysed",
                ".eslintrc.json",
                "`extends` in `.eslintrc.json` now inherits `plugin:x/lax`; what an inherited configuration loosens cannot be read from this diff.",
                "Review the inherited configuration; record it with `allow-toolchain-weakening: extends <reason>` if it is intended.",
            ),
            row(
                "toolchain-config/toolchain-config-change-not-analysed",
                "eslint.config.js",
                "`eslint.config.js` is configuration written as code; whether the change loosens it cannot be read from a diff.",
                "Review the change; record it with `allow-toolchain-weakening: <path> <reason>` if it is intended.",
            ),
            row(
                "toolchain-config/toolchain-config-deleted",
                "mypy.ini",
                "`mypy.ini` was deleted; the settings it carried no longer apply.",
                "Restore it, or record the deletion with `allow-toolchain-weakening: <path> <reason>`.",
            ),
            row(
                "toolchain-config/toolchain-config-unreadable",
                "ruff.toml",
                "`ruff.toml` could not be parsed on one side, so its weakening could not be checked.",
                "Fix the file so it parses.",
            ),
            row(
                "toolchain-config/toolchain-config-weakened",
                "tsconfig.json",
                "`compilerOptions.strict` switched off in `tsconfig.json`.",
                "Revert it, or justify it on its own line in the PR body or a commit message: `allow-toolchain-weakening: compilerOptions.strict <reason>`.",
            ),
        ],
        "{}{}",
        run.stdout,
        run.stderr
    );
}

#[test]
fn sandbox_config_words_each_finding_names_its_own_directive_and_the_unreadable_side() {
    let repo = changed(&[
        (
            ".devcontainer/init-firewall.sh",
            "iptables -P OUTPUT DROP\n",
            Some("iptables -P OUTPUT ACCEPT\n"),
        ),
        (
            ".claude/settings.json",
            "{\"permissions\": {\"allow\": []}}\n",
            Some("{\"permissions\": {\"allow\": [\"Bash\"]}}\n"),
        ),
        (
            "compose.yaml",
            "services: [\n",
            Some("services:\n  a:\n    image: x\n"),
        ),
        (".mcp.json", "{\"mcpServers\": {}}\n", Some("{\n")),
    ]);
    let run = repo.check(&[]);
    assert_eq!(
        wording(&run, "sandbox-config"),
        [
            row(
                "sandbox-config/sandbox-change-not-analysed",
                ".devcontainer/init-firewall.sh",
                "`.devcontainer/init-firewall.sh` sets a container's outbound rules in code; whether the change widens them cannot be read from a diff.",
                "Review the change; record it with `allow-sandbox-widening: <path> <reason>` if it is intended.",
            ),
            row(
                "sandbox-config/sandbox-config-unreadable",
                ".mcp.json",
                "`.mcp.json` could not be parsed on the head side, so whether it widens the sandbox could not be checked.",
                "Fix the file so it parses.",
            ),
            row(
                "sandbox-config/sandbox-config-unreadable",
                "compose.yaml",
                "`compose.yaml` could not be parsed on the base side, so whether it widens the sandbox could not be checked.",
                "Fix the file so it parses.",
            ),
            row(
                "sandbox-config/sandbox-config-widened",
                ".claude/settings.json",
                "`permissions.allow` gained 1 entr(y/ies): `Bash` in `.claude/settings.json`.",
                "Revert it, or justify it on its own line in the PR body or a commit message: `allow-sandbox-widening: permissions.allow <reason>`.",
            ),
        ],
        "{}{}",
        run.stdout,
        run.stderr
    );
}

#[test]
fn each_gate_says_in_its_own_words_that_it_examined_nothing() {
    let repo = changed(&[("docs/more.md", "# More\n", Some("# More\n\nText.\n"))]);
    let run = repo.check(&[]);
    for (gate, note) in [
        (
            "toolchain-config",
            "no toolchain configuration files modified in this diff",
        ),
        (
            "sandbox-config",
            "no agent settings or container definitions modified in this diff",
        ),
    ] {
        let outcome = run.outcome(gate);
        assert_eq!(outcome["examined"], 0, "{gate}: {outcome}");
        assert_eq!(outcome["notes"], serde_json::json!([note]), "{gate}");
    }
}
