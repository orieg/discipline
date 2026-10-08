//! Suppressions a tool honours that `suppression-delta` did not read (#685): qualified
//! and file-level annotations, comment spellings the tools accept, and a comment that
//! carries two tools' suppressions. Each is driven through the real binary: a change
//! that adds the suppression is reported at its line and lifted by `allow-suppression`,
//! and a change that adds a spelling the tool does not honour is not reported.

mod common;
use common::Repo;

const BLOCKING: &[&str] = &[
    "--config-override",
    "[gates.suppression-delta]\nseverity = \"error\"",
];
const GATE: &str = "suppression-delta";

/// One added suppression: the file, its base side, the line put in after `after_line`
/// lines of the base side, and a subject that lifts it.
struct Added<'a> {
    path: &'a str,
    base: &'a str,
    after_line: usize,
    line: &'a str,
    /// The snippets reported, one per site, in order.
    snippets: &'a [&'a str],
    subject: &'a str,
}

/// `base` with `line` put in after its first `after` lines.
fn with_line(base: &str, after: usize, line: &str) -> String {
    let mut lines: Vec<&str> = base.lines().collect();
    lines.insert(after, line);
    lines.join("\n") + "\n"
}

/// The suppression is reported once per snippet at its line, and the directive naming
/// `subject` lifts every one.
fn reported_then_lifted(case: &Added) {
    let repo = Repo::new();
    repo.commit_base(case.path, case.base, "chore: base file");
    repo.write(case.path, &with_line(case.base, case.after_line, case.line));
    repo.commit("chore: change");
    let run = repo.check(BLOCKING);
    let got: Vec<(String, u64, String)> = run
        .violations(GATE)
        .iter()
        .map(|v| {
            (
                v["file"].as_str().unwrap().to_string(),
                v["line"].as_u64().unwrap(),
                v["remediation"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(got.len(), case.snippets.len(), "{}: {got:?}", case.line);
    for ((file, line, detail), snippet) in got.iter().zip(case.snippets) {
        assert_eq!(file, case.path, "{}", case.line);
        assert_eq!(*line, case.after_line as u64 + 1, "{}", case.line);
        assert!(
            detail.contains(&format!("`{snippet}`")),
            "{}: {detail}",
            case.line
        );
    }
    assert_eq!(run.code, 1, "{}", case.line);

    repo.commit(&format!(
        "chore: explain\n\nallow-suppression: {} the tool is wrong about this line",
        case.subject
    ));
    let lifted = repo.check(BLOCKING);
    assert!(
        lifted.violations(GATE).is_empty(),
        "{}: {:?}",
        case.line,
        lifted.violations(GATE)
    );
    assert_eq!(
        lifted.outcome(GATE)["overrides"].as_array().unwrap().len(),
        case.snippets.len(),
        "{}",
        case.line
    );
}

/// Adding `line` to the file adds no site.
fn not_reported(path: &str, base: &str, after_line: usize, line: &str) {
    let repo = Repo::new();
    repo.commit_base(path, base, "chore: base file");
    repo.write(path, &with_line(base, after_line, line));
    repo.commit("chore: change");
    let run = repo.check(BLOCKING);
    assert!(
        run.violations(GATE).is_empty(),
        "{line}: {:?}",
        run.violations(GATE)
    );
    assert_eq!(run.outcome(GATE)["examined"], 0, "{line}");
}

const JAVA: &str =
    "package p;\nclass A {\n  int f(int p) {\n    int x = p;\n    return x;\n  }\n}\n";
const KOTLIN: &str =
    "package p\nclass A {\n  fun f(p: Int): Int {\n    val x = p\n    return x\n  }\n}\n";
const SCALA: &str = "class A {\n  def f(p: Int): Int = {\n    val x = p\n    x\n  }\n}\n";
const RUBY: &str = "class A\n  def f(p)\n    x = p\n    x\n  end\nend\n";
const PYTHON: &str = "import os\n\n\ndef f(p):\n    x = p\n    return x\n";

#[test]
fn java_qualified_annotation_added_is_reported_and_lifted() {
    reported_then_lifted(&Added {
        path: "src/main/java/A.java",
        base: JAVA,
        after_line: 1,
        line: "@java.lang.SuppressWarnings(\"unchecked\")",
        snippets: &["@java.lang.SuppressWarnings(\"unchecked\")"],
        subject: "unchecked",
    });
    reported_then_lifted(&Added {
        path: "src/main/java/A.java",
        base: JAVA,
        after_line: 2,
        line: "  @java.lang.SuppressWarnings",
        snippets: &["@java.lang.SuppressWarnings"],
        subject: "src/main/java/A.java",
    });
    not_reported(
        "src/main/java/A.java",
        JAVA,
        1,
        "@my.NotSuppressWarnings(\"unchecked\")",
    );
}

#[test]
fn java_comment_suppression_added_is_reported_and_lifted() {
    reported_then_lifted(&Added {
        path: "src/main/java/A.java",
        base: JAVA,
        after_line: 3,
        line: "    // NOSONAR",
        snippets: &["// NOSONAR"],
        subject: "NOSONAR",
    });
    reported_then_lifted(&Added {
        path: "src/main/java/A.java",
        base: JAVA,
        after_line: 3,
        line: "    //noinspection UnnecessaryLocalVariable",
        snippets: &["//noinspection UnnecessaryLocalVariable"],
        subject: "UnnecessaryLocalVariable",
    });
    not_reported("src/main/java/A.java", JAVA, 3, "    // nosonar");
    not_reported("src/main/java/A.java", JAVA, 3, "    //noinspection");
    not_reported(
        "src/main/java/A.java",
        JAVA,
        3,
        "    /* noinspection UnnecessaryLocalVariable */",
    );
}

#[test]
fn kotlin_qualified_and_file_annotation_added_is_reported_and_lifted() {
    reported_then_lifted(&Added {
        path: "src/main/kotlin/A.kt",
        base: KOTLIN,
        after_line: 0,
        line: "@file:Suppress(\"UNUSED_PARAMETER\")",
        snippets: &["@file:Suppress(\"UNUSED_PARAMETER\")"],
        subject: "UNUSED_PARAMETER",
    });
    reported_then_lifted(&Added {
        path: "src/main/kotlin/A.kt",
        base: KOTLIN,
        after_line: 1,
        line: "@kotlin.Suppress(\"UNCHECKED_CAST\")",
        snippets: &["@kotlin.Suppress(\"UNCHECKED_CAST\")"],
        subject: "UNCHECKED_CAST",
    });
    not_reported(
        "src/main/kotlin/A.kt",
        KOTLIN,
        0,
        "@file:JvmName(\"Suppress\")",
    );
    not_reported("src/main/kotlin/A.kt", KOTLIN, 1, "@my.Suppressed(\"x\")");
}

#[test]
fn kotlin_comment_suppression_added_is_reported_and_lifted() {
    reported_then_lifted(&Added {
        path: "src/main/kotlin/A.kt",
        base: KOTLIN,
        after_line: 3,
        line: "    // NOSONAR",
        snippets: &["// NOSONAR"],
        subject: "NOSONAR",
    });
    not_reported("src/main/kotlin/A.kt", KOTLIN, 3, "    // see NOSONAR");
}

#[test]
fn scala_qualified_annotation_added_is_reported_and_lifted() {
    reported_then_lifted(&Added {
        path: "src/main/scala/A.scala",
        base: SCALA,
        after_line: 0,
        line: "@scala.annotation.nowarn",
        snippets: &["@scala.annotation.nowarn"],
        subject: "src/main/scala/A.scala",
    });
    reported_then_lifted(&Added {
        path: "src/main/scala/A.scala",
        base: SCALA,
        after_line: 1,
        line: "  @scala.annotation.nowarn(\"cat=deprecation\")",
        snippets: &["@scala.annotation.nowarn(\"cat=deprecation\")"],
        subject: "cat=deprecation",
    });
    not_reported("src/main/scala/A.scala", SCALA, 0, "@my.notnowarn");
}

#[test]
fn ruby_unspaced_rubocop_comment_added_is_reported_and_lifted() {
    reported_then_lifted(&Added {
        path: "lib/a.rb",
        base: RUBY,
        after_line: 2,
        line: "    #rubocop:disable Lint/UselessAssignment",
        snippets: &["#rubocop:disable Lint/UselessAssignment"],
        subject: "Lint/UselessAssignment",
    });
    reported_then_lifted(&Added {
        path: "lib/a.rb",
        base: RUBY,
        after_line: 0,
        line: "#  rubocop : todo Style/Documentation",
        snippets: &["#  rubocop : todo Style/Documentation"],
        subject: "Style/Documentation",
    });
    not_reported(
        "lib/a.rb",
        RUBY,
        2,
        "    #rubocop:enable Lint/UselessAssignment",
    );
    not_reported(
        "lib/a.rb",
        RUBY,
        2,
        "    # rubo cop:disable Lint/UselessAssignment",
    );
}

#[test]
fn python_spelling_added_is_reported_and_lifted() {
    let cases = [
        ("# NOQA", "pkg/a.py"),
        ("# NoQA: F401", "F401"),
        ("# type:ignore", "mypy"),
        ("# pylint: disable-next=unused-variable", "unused-variable"),
    ];
    for (comment, subject) in cases {
        reported_then_lifted(&Added {
            path: "pkg/a.py",
            base: PYTHON,
            after_line: 4,
            line: &format!("    {comment}"),
            snippets: &[comment],
            subject,
        });
    }
    not_reported("pkg/a.py", PYTHON, 4, "    # typ:ignore");
    not_reported(
        "pkg/a.py",
        PYTHON,
        4,
        "    # pylint: disable-nxt=unused-variable",
    );
    not_reported("pkg/a.py", PYTHON, 4, "    y = \"# NOQA\"");
}

/// A `noqa` after other comment text is a site, and a comment that suppresses mypy and
/// the linter is one site for each tool: lifting one leaves the other reported.
#[test]
fn python_noqa_after_other_comment_text_is_its_own_site() {
    reported_then_lifted(&Added {
        path: "pkg/a.py",
        base: PYTHON,
        after_line: 4,
        line: "    y = p  # the vendor stub is wrong # noqa: E501",
        snippets: &["# noqa: E501"],
        subject: "E501",
    });
    reported_then_lifted(&Added {
        path: "pkg/a.py",
        base: PYTHON,
        after_line: 4,
        line: "    y = p  # type: ignore # noqa: E501",
        snippets: &["# type: ignore # noqa: E501", "# noqa: E501"],
        subject: "pkg/a.py",
    });
    not_reported(
        "pkg/a.py",
        PYTHON,
        4,
        "    y = p  # the vendor stub is wrong, noqa",
    );

    let repo = Repo::new();
    repo.commit_base("pkg/a.py", PYTHON, "chore: base file");
    repo.write(
        "pkg/a.py",
        &with_line(PYTHON, 4, "    y = p  # type: ignore # noqa: E501"),
    );
    repo.commit("chore: change\n\nallow-suppression: mypy the vendor stub is wrong");
    let run = repo.check(BLOCKING);
    let kinds: Vec<String> = run
        .violations(GATE)
        .iter()
        .map(|v| v["message"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(kinds.len(), 1, "{kinds:?}");
    assert!(kinds[0].contains("linter-disable"), "{kinds:?}");
}
