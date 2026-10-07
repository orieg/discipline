//! Which `if` an early-return detector takes as a candidate is read from the syntax tree
//! (#651).
//!
//! In Rust, Go, Python and JavaScript a test that returns early under a condition on the
//! environment is a conditional skip. An `if` whose condition involves a CI variable is
//! found by the tree's reading of the condition. An `if` on any other environment read is
//! found by what its code spells: an environment read, a CI variable name, or a name the
//! test binds to one. A CI variable named inside a longer string or in a comment, an
//! environment read spelled inside a string, and a bound name that appears only inside a
//! string do not make it one.
//!
//! Each case drives the real binary over a base side whose test runs and a head side
//! that adds the early return. The sources are fixtures built by this file, not tests of
//! this suite.

mod common;
use common::Repo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    /// One `Test Conditionally Skipped` at `error`, exit 1.
    CiSkip,
    /// One `Test Conditionally Skipped` at `note`, exit 0.
    Note,
    /// No finding, exit 0.
    Nothing,
}

struct Lang {
    name: &'static str,
    path: &'static str,
    /// The file with `{PRELUDE}` before the test and `{GUARD}` first in its body.
    template: &'static str,
}

const LANGS: &[Lang] = &[
    Lang {
        name: "rust",
        path: "tests/cart.rs",
        template: "{PRELUDE}#[test]\nfn total() {\n{GUARD}    assert_eq!(cart::total(), 3);\n}\n",
    },
    Lang {
        name: "go",
        path: "cart_test.go",
        template: "package cart\n\nimport (\n\t\"os\"\n\t\"strings\"\n\t\"testing\"\n)\n\nvar _ = os.Getenv\nvar _ = strings.Contains\n\n{PRELUDE}func TestTotal(t *testing.T) {\n{GUARD}\tif Total() != 3 {\n\t\tt.Fatal(\"total\")\n\t}\n}\n",
    },
    Lang {
        name: "python",
        path: "tests/test_cart.py",
        template: "import os\n\n{PRELUDE}\ndef test_total():\n{GUARD}    assert total() == 3\n",
    },
    Lang {
        name: "javascript",
        path: "tests/cart.test.js",
        template: "{PRELUDE}test(\"sums\", () => {\n{GUARD}  expect(total()).toBe(3);\n});\n",
    },
];

fn guard(lang: &str, condition: &str) -> String {
    match lang {
        "rust" => format!("    if {condition} {{\n        return;\n    }}\n"),
        "go" => format!("\tif {condition} {{\n\t\treturn\n\t}}\n"),
        "python" => format!("    if {condition}:\n        return\n"),
        _ => format!("  if ({condition}) {{\n    return;\n  }}\n"),
    }
}

fn seen(lang: &Lang, prelude: &str, guard: &str) -> (i32, Vec<(String, String)>) {
    let repo = Repo::new();
    let base = lang
        .template
        .replace("{PRELUDE}", prelude)
        .replace("{GUARD}", "");
    let head = lang
        .template
        .replace("{PRELUDE}", prelude)
        .replace("{GUARD}", guard);
    repo.commit_base_files(&[(lang.path, &base)], "test: base suite");
    repo.write(lang.path, &head);
    repo.commit("test: add a guard");
    let run = repo.check(&[]);
    let findings = run
        .violations("ignored-tests")
        .iter()
        .map(|v| {
            (
                v["title"].as_str().unwrap().to_string(),
                v["severity"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    (run.code, findings)
}

fn differing(cases: &[(&str, &str, String, Want)]) -> Vec<String> {
    let mut wrong = Vec::new();
    for (lang, prelude, guard, want) in cases {
        let lang = LANGS.iter().find(|l| l.name == *lang).unwrap();
        let (code, findings) = seen(lang, prelude, guard);
        let one = |severity: &str| {
            findings.len() == 1
                && findings[0].0 == "Test Conditionally Skipped"
                && findings[0].1 == severity
        };
        let ok = match want {
            Want::CiSkip => one("error") && code == 1,
            Want::Note => one("note") && code == 0,
            Want::Nothing => findings.is_empty() && code == 0,
        };
        if !ok {
            wrong.push(format!(
                "{} `{}`: wanted {want:?}, got exit {code} with {findings:?}",
                lang.name,
                guard.trim().replace('\n', " ")
            ));
        }
    }
    wrong
}

fn case(
    lang: &'static str,
    condition: &str,
    want: Want,
) -> (&'static str, &'static str, String, Want) {
    (lang, "", guard(lang, condition), want)
}

#[test]
fn a_ci_name_inside_a_longer_string_is_not_a_candidate() {
    let wrong = differing(&[
        case("rust", "mode() == \"runs on CI too\"", Want::Nothing),
        case("go", "mode() == \"runs on CI too\"", Want::Nothing),
        case("python", "mode() == \"runs on CI too\"", Want::Nothing),
        case("javascript", "mode() === \"runs on CI too\"", Want::Nothing),
        case("javascript", "mode() === `runs on CI too`", Want::Nothing),
    ]);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn a_ci_name_in_a_comment_is_not_a_candidate() {
    let wrong = differing(&[
        case("rust", "slow() /* not in CI */", Want::Nothing),
        case("go", "slow() /* not in CI */", Want::Nothing),
        (
            "python",
            "",
            "    if (slow()  # not in CI\n        ):\n        return\n".to_string(),
            Want::Nothing,
        ),
        case("javascript", "slow() /* not in CI */", Want::Nothing),
    ]);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn an_environment_read_spelled_in_a_string_is_not_a_candidate() {
    let wrong = differing(&[
        case(
            "rust",
            "source().contains(\"std::env::var\")",
            Want::Nothing,
        ),
        case(
            "go",
            "strings.Contains(source(), \"os.Getenv\")",
            Want::Nothing,
        ),
        case("python", "\"os.environ\" in source()", Want::Nothing),
        case(
            "javascript",
            "source().includes(\"process.env\")",
            Want::Nothing,
        ),
    ]);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn a_bound_name_inside_a_string_is_not_a_candidate() {
    let wrong = differing(&[
        (
            "rust",
            "",
            "    let flag = std::env::var(\"CI\").is_ok();\n    let _ = flag;\n    if label() == \"flag\" {\n        return;\n    }\n".to_string(),
            Want::Nothing,
        ),
        (
            "go",
            "",
            "\tflag := os.Getenv(\"CI\") != \"\"\n\t_ = flag\n\tif label() == \"flag\" {\n\t\treturn\n\t}\n".to_string(),
            Want::Nothing,
        ),
        (
            "python",
            "",
            "    flag = os.environ.get(\"CI\")\n    if label() == \"flag\":\n        return\n".to_string(),
            Want::Nothing,
        ),
        (
            "javascript",
            "",
            "  const flag = process.env.CI;\n  if (label() === \"flag\") {\n    return;\n  }\n".to_string(),
            Want::Nothing,
        ),
    ]);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn a_real_environment_read_is_still_a_ci_early_return() {
    let wrong = differing(&[
        case("rust", "std::env::var(\"CI\").is_ok()", Want::CiSkip),
        case("rust", "option_env!(\"CI\").is_some()", Want::CiSkip),
        case("go", "os.Getenv(\"CI\") != \"\"", Want::CiSkip),
        case("python", "os.environ.get(\"CI\")", Want::CiSkip),
        case("python", "os.getenv('GITHUB_ACTIONS')", Want::CiSkip),
        case("javascript", "process.env.CI", Want::CiSkip),
        case("javascript", "process.env[\"CI\"]", Want::CiSkip),
        (
            "rust",
            "",
            "    let ci = std::env::var(\"CI\").is_ok();\n    if ci {\n        return;\n    }\n"
                .to_string(),
            Want::CiSkip,
        ),
        (
            "go",
            "",
            "\tci := os.Getenv(\"CI\") != \"\"\n\tif ci {\n\t\treturn\n\t}\n".to_string(),
            Want::CiSkip,
        ),
        (
            "python",
            "",
            "    ci = os.environ.get(\"CI\")\n    if ci:\n        return\n".to_string(),
            Want::CiSkip,
        ),
        (
            "javascript",
            "",
            "  const ci = process.env.CI;\n  if (ci) {\n    return;\n  }\n".to_string(),
            Want::CiSkip,
        ),
    ]);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A string literal that is the name of a CI variable is read by the tree's own reading
/// of the condition, whatever call or comparison holds it: these verdicts do not change.
#[test]
fn a_whole_string_naming_a_ci_variable_keeps_its_verdict() {
    let wrong = differing(&[
        case("rust", "lookup(\"CI\")", Want::CiSkip),
        case("go", "lookup(\"CI\")", Want::CiSkip),
        case("python", "lookup(\"CI\")", Want::CiSkip),
        case("javascript", "lookup(\"CI\")", Want::CiSkip),
        case("rust", "runner() == \"CI\"", Want::CiSkip),
        case("go", "runner() == \"CI\"", Want::CiSkip),
        case("python", "runner() == \"CI\"", Want::CiSkip),
        case("javascript", "runner() === \"CI\"", Want::CiSkip),
    ]);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// An early return under a read of a variable that is not a CI variable is found by the
/// environment read its code spells, and stays a note.
#[test]
fn an_environment_read_of_another_variable_is_still_a_note() {
    let wrong = differing(&[
        case("rust", "std::env::var(\"SLOW\").is_ok()", Want::Note),
        case("go", "os.Getenv(\"SLOW\") != \"\"", Want::Note),
        case("python", "os.environ.get(\"SLOW\")", Want::Note),
        case("javascript", "process.env.SLOW", Want::Note),
        (
            "go",
            "",
            "\tslow := os.Getenv(\"SLOW\") != \"\"\n\tif slow {\n\t\treturn\n\t}\n".to_string(),
            Want::Note,
        ),
    ]);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
