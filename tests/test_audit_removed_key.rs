//! `discipline audit` across a configuration key a later release removed (#631): the key
//! is set aside by name and the rest of the file is compared. `check` stays strict, and a
//! name that was never a key stays unreadable. Driving the real binary.

mod common;

use common::Repo;
use serde_json::Value;

const KEY: &str = "gates.commit-provenance.allow_author_review";

fn config(exempt: &str, provenance_extra: &str, pii_extra: &str) -> String {
    format!(
        "[meta]\nversion = 1\nname = \"t\"\n[gates.pii]\nexempt_paths = [{exempt}]\n{pii_extra}\
         [gates.commit-provenance]\nenabled = true\n{provenance_extra}"
    )
}

/// `main` after a base that adopts a configuration holding the removed key:
/// #2 grows `exempt_paths` beside the key, #3 drops the key and nothing else, #4 grows
/// `exempt_paths` again with the key gone, #5 adds a name that was never a key, #6 grows
/// `exempt_paths` beside that name.
fn history() -> Repo {
    const WITH_KEY: &str = "allow_author_review = true\n";
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    let steps = [
        ("chore: adopt (#1)", config("\"a/**\"", WITH_KEY, "")),
        (
            "chore: exempt b (#2)",
            config("\"a/**\", \"b/**\"", WITH_KEY, ""),
        ),
        (
            "chore: drop the key (#3)",
            config("\"a/**\", \"b/**\"", "", ""),
        ),
        (
            "chore: exempt c (#4)",
            config("\"a/**\", \"b/**\", \"c/**\"", "", ""),
        ),
        (
            "chore: a typo (#5)",
            config("\"a/**\", \"b/**\", \"c/**\"", "", "never_a_key = true\n"),
        ),
        (
            "chore: exempt d (#6)",
            config(
                "\"a/**\", \"b/**\", \"c/**\", \"d/**\"",
                "",
                "never_a_key = true\n",
            ),
        ),
    ];
    for (subject, text) in steps {
        repo.write("discipline.toml", &text);
        repo.commit(subject);
    }
    repo
}

fn records_of(audit: &Value, pr: u64) -> Vec<Value> {
    audit["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["pr"] == pr)
        .cloned()
        .collect()
}

#[test]
fn a_loosening_beside_a_removed_key_is_compared_and_the_record_names_the_key() {
    let repo = history();
    let run = repo.run(&["audit", "--last", "5", "--ref", "main", "--json"], &[]);
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    let audit: Value = serde_json::from_str(&run.stdout).unwrap();

    // #2: the loosening is a `config` record, and it says what was set aside.
    let r = records_of(&audit, 2);
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(r[0]["kind"], "config", "{r:?}");
    assert_eq!(r[0]["gate"], "pii");
    assert_eq!(r[0]["key"], "exempt_paths");
    let detail = r[0]["detail"].as_str().unwrap();
    assert!(
        detail.contains(&format!("`{KEY}`")) && detail.contains("parent and change"),
        "{detail}"
    );

    // #3: the key itself goes. What it did is not judged, and the record says so.
    let r = records_of(&audit, 3);
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(r[0]["kind"], "config-unreadable", "{r:?}");
    let detail = r[0]["detail"].as_str().unwrap();
    assert!(detail.starts_with("parent: "), "{detail}");
    assert!(
        detail.ends_with("unknown key `allow_author_review`"),
        "{detail}"
    );

    // Control, #4: with no removed key on either side the record carries no note.
    let r = records_of(&audit, 4);
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(r[0]["kind"], "config");
    assert!(r[0].get("detail").is_none(), "{r:?}");

    // #5 and #6: a name that was never a key is not set aside; neither side that holds it
    // is compared.
    for pr in [5, 6] {
        let r = records_of(&audit, pr);
        assert_eq!(r.len(), 1, "#{pr}: {r:?}");
        assert_eq!(r[0]["kind"], "config-unreadable", "#{pr}: {r:?}");
        let detail = r[0]["detail"].as_str().unwrap();
        assert!(detail.ends_with("unknown key `never_a_key`"), "{detail}");
    }

    // The text report carries the note on the loosening's line.
    let text = repo.run(&["audit", "--last", "5", "--ref", "main"], &[]);
    assert_eq!(text.code, 0, "{}\n{}", text.stdout, text.stderr);
    let line = text
        .stdout
        .lines()
        .find(|l| l.contains("#2") && l.contains("exempt_paths"))
        .unwrap_or_else(|| panic!("no line for #2:\n{}", text.stdout));
    assert!(line.contains(KEY), "{line}");
}

#[test]
fn check_still_refuses_a_removed_key() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "discipline.toml",
        &config("\"a/**\"", "allow_author_review = true\n", ""),
    );
    repo.commit("chore: policy");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write("docs/notes.md", "# Notes\n");
    repo.commit("docs: notes");
    let run = repo.check(&[]);
    assert_eq!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    assert!(
        format!("{}{}", run.stdout, run.stderr).contains("allow_author_review"),
        "{}\n{}",
        run.stdout,
        run.stderr
    );
}
