//! `suppression-delta` with its default `quote_line = false` reports where a suppression
//! was added and which rule it names, and never the free text beside the rule: a
//! description after the rules (`-- reason`, `// reason`, `# reason`) or a word that is
//! not a rule identifier can hold anything, a token included (AGENTS.md §3.4, item 6).
//!
//! One case per suppression family, each with a fake secret in its free text, through
//! the real binary and every report format.

mod common;
use common::{Repo, CONFIG_HEAD};

const SECRET: &str = "FREETEXTSECRET42";

/// The families, each as a file and its added suppression line(s) with free text.
fn families() -> Vec<(&'static str, String)> {
    let s = SECRET;
    vec![
        (
            "src/eslint.js",
            format!("// eslint-disable-next-line no-console -- token={s}\nconsole.log(1);\n"),
        ),
        (
            "src/eslint_block.js",
            format!("/* eslint-disable no-alert, no-eval -- {s} */\nalert(1);\n"),
        ),
        (
            "src/ts_ignore.ts",
            format!("// @ts-ignore because {s}\nconst a: number = \"x\";\n"),
        ),
        (
            "src/ts_expect.ts",
            format!("// @ts-expect-error {s}\nconst b: number = \"x\";\n"),
        ),
        ("src/ruff.py", format!("import os  # noqa: F401 {s}\n")),
        ("src/flake8.py", format!("import sys  # noqa: F401, E501 # {s}\n")),
        (
            "src/pylint.py",
            format!("x = 1  # pylint: disable=invalid-name {s}\n"),
        ),
        (
            "src/mypy.py",
            format!("y: int = \"x\"  # type: ignore[assignment] {s}\n"),
        ),
        (
            "src/clippy.rs",
            format!("#[allow(clippy::needless_return, reason = \"{s}\")]\npub fn f() -> u8 {{ return 1; }}\n"),
        ),
        (
            "src/Warn.java",
            format!("public class Warn {{\n    @SuppressWarnings(\"unchecked {s}\")\n    void f() {{}}\n}}\n"),
        ),
        (
            "src/nolint.go",
            format!("package p\n\nfunc f() {{\n\tg() //nolint:errcheck // {s}\n}}\n"),
        ),
        (
            "src/rubocop.rb",
            format!("def f # rubocop:disable Metrics/AbcSize -- {s}\nend\n"),
        ),
        (
            "src/nolint.cc",
            format!("int f() {{ return 0; }} // NOLINT(readability-x) {s}\n"),
        ),
        (
            "src/Pragma.cs",
            format!("class P {{\n#pragma warning disable CS0168 // {s}\n}}\n"),
        ),
        (
            "src/Sup.kt",
            format!("@Suppress(\"UNCHECKED_CAST {s}\")\nfun f() {{}}\n"),
        ),
        (
            "src/Message.cs",
            format!("class M {{\n    [SuppressMessage(\"Usage\", \"CA1801\", Justification = \"{s}\")]\n    void F() {{}}\n}}\n"),
        ),
        (
            "src/Warn.scala",
            format!("object W {{\n  @nowarn(\"msg={s}\")\n  def f(): Unit = ()\n}}\n"),
        ),
        (
            "src/pragma.c",
            format!("#pragma GCC diagnostic ignored \"-Wunused\" // {s}\nint g(void) {{ return 0; }}\n"),
        ),
        (
            "src/staticcheck.go",
            format!("package p\n\n//lint:ignore SA1019 {s}\nvar x = old.F\n"),
        ),
        (
            "src/lint.swift",
            format!("// swiftlint:disable:next force_cast {s}\nlet a = b as! C\n"),
        ),
    ]
}

#[test]
fn no_family_echoes_its_free_text_in_any_report_format() {
    let repo = Repo::new();
    repo.write("discipline.toml", CONFIG_HEAD);
    repo.commit("chore: configuration");
    repo.git(&["branch", "-f", "main"]);
    let families = families();
    for (path, src) in &families {
        repo.write(path, src);
    }
    repo.commit("feat: add suppressions");

    let json = repo.check(&[]);
    let report: serde_json::Value = serde_json::from_str(&json.stdout).unwrap_or_else(|e| {
        panic!("json: {e}\n{}\n{}", json.stdout, json.stderr);
    });
    let outcome = report["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["gate"] == "suppression-delta")
        .expect("suppression-delta ran");
    let violations = outcome["violations"].as_array().unwrap();
    // Every family is reported: the absence of the secret is not an absence of findings.
    for (path, _) in &families {
        assert!(
            violations.iter().any(|v| v["file"].as_str() == Some(path)),
            "{path} is not reported: {violations:#?}"
        );
    }
    assert!(
        !json.stdout.contains(SECRET) && !json.stderr.contains(SECRET),
        "the json report echoes free text:\n{}",
        json.stdout
    );
    // The rule identifiers are still named.
    let messages: String = violations
        .iter()
        .map(|v| format!("{}\n", v["message"].as_str().unwrap_or("")))
        .collect();
    for rule in [
        "no-console",
        "no-alert, no-eval",
        "F401",
        "F401, E501",
        "invalid-name",
        "assignment",
        "clippy::needless_return",
        "unchecked",
        "errcheck",
        "Metrics/AbcSize",
        "readability-x",
        "CS0168",
        "UNCHECKED_CAST",
        "force_cast",
        "CA1801",
        "-Wunused",
        "SA1019",
    ] {
        assert!(messages.contains(rule), "{rule} is not named:\n{messages}");
    }

    for format in [
        "terminal",
        "github-summary",
        "junit",
        "sarif",
        "gitlab",
        "agent-prompt",
    ] {
        let run = repo.run(&["check", "--format", format, "--base", "main"], &[]);
        assert!(
            !run.stdout.contains(SECRET) && !run.stderr.contains(SECRET),
            "the {format} report echoes free text:\n{}\n{}",
            run.stdout,
            run.stderr
        );
    }
}

#[test]
fn quote_line_still_quotes_the_line_when_asked() {
    let repo = Repo::new();
    repo.write("discipline.toml", CONFIG_HEAD);
    repo.commit("chore: configuration");
    repo.git(&["branch", "-f", "main"]);
    repo.write(
        "src/eslint.js",
        &format!("// eslint-disable-next-line no-console -- note {SECRET}\nconsole.log(1);\n"),
    );
    repo.commit("feat: add suppression");
    let run = repo.run(
        &[
            "check",
            "--format",
            "json",
            "--base",
            "main",
            "--config-override",
            "[gates.suppression-delta]\nquote_line = true",
        ],
        &[],
    );
    assert!(run.stdout.contains(SECRET), "{}", run.stdout);
}
