//! `discipline audit --format html`: a control, bidirectional or invisible character in
//! text an author wrote (a commit subject, a file name) does not
//! reach the page. Each one is shown as U+FFFD, so the reader sees that something was
//! there and the row cannot be reordered or have text hidden in it.

mod common;

use common::Repo;

const CFG: &str = common::CONFIG_HEAD;

/// One character of each class the page replaces: C0 (ESC, and U+0001), DEL, C1 (CSI),
/// the bidirectional overrides, embeddings and isolates, the two directional marks and
/// the Arabic letter mark, the line and paragraph separators, the zero-width characters,
/// the soft hyphen and a tag character.
const HIDDEN: &[char] = &[
    '\u{1}',
    '\u{1b}',
    '\u{7f}',
    '\u{9b}',
    '\u{202a}',
    '\u{202c}',
    '\u{202e}',
    '\u{2066}',
    '\u{2069}',
    '\u{200e}',
    '\u{200f}',
    '\u{61c}',
    '\u{2028}',
    '\u{2029}',
    '\u{200b}',
    '\u{200d}',
    '\u{2060}',
    '\u{feff}',
    '\u{ad}',
    '\u{e0041}',
];

fn audit_page(repo: &Repo) -> String {
    let out = repo.file("audit.html");
    let run = repo.run(
        &[
            "audit",
            "--last",
            "1",
            "--ref",
            "main",
            "--format",
            "html",
            "--output",
            out.to_str().unwrap(),
        ],
        &[("DISCIPLINE_NO_NETWORK", "1")],
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    std::fs::read_to_string(&out).unwrap()
}

fn code_points(page: &str) -> Vec<String> {
    HIDDEN
        .iter()
        .filter(|c| page.contains(**c))
        .map(|c| format!("U+{:04X}", *c as u32))
        .collect()
}

#[test]
fn no_control_or_bidirectional_character_an_author_wrote_reaches_the_page() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.git(&["remote", "add", "origin", "https://github.com/o/r.git"]);
    repo.write(
        "discipline.toml",
        &format!("{CFG}[gates.pii]\nexempt_paths = []\n"),
    );
    repo.commit("chore: adopt (#1)");

    // A file name that reads `src/a<reversed>sr.rs` in a row of the page, with a
    // zero-width space and a C0 control in it.
    let marker = "fn a() {} // discipline:allow(time-estimates) a quoted release plan\n";
    repo.write("src/a\u{202e}sr.\u{200b}r\u{1}s", marker);
    repo.write("src/plain.rs", marker);
    // A configuration value an author wrote: the page counts a list's entries and does
    // not quote them, so nothing of it may appear.
    repo.write(
        "discipline.toml",
        &format!("{CFG}[gates.pii]\nexempt_paths = [\"a\u{202e}b\u{2066}/**\"]\n"),
    );
    // A subject with one character of every class, between ordinary words.
    let hidden: String = HIDDEN.iter().collect();
    let subject = format!("feat: visible start {hidden} visible end");
    repo.git(&["add", "-A"]);
    repo.git(&[
        "commit",
        "-q",
        "--author",
        "Zq\u{202e}Xyzzy\u{1b}[31m\u{200f} <author@example.invalid>",
        "-m",
        &subject,
    ]);
    // The history holds what the test says it does.
    let logged = repo.git_output(&["log", "-1", "--format=%s%n%an"]);
    assert!(logged.contains('\u{202e}'), "{logged:?}");
    assert!(logged.contains('\u{1b}'), "{logged:?}");

    let page = audit_page(&repo);
    assert!(
        code_points(&page).is_empty(),
        "hidden characters in the page: {:?}",
        code_points(&page)
    );
    // The text around them is still there, and each one is shown as U+FFFD.
    assert!(page.contains("feat: visible start "), "the subject");
    assert!(page.contains(" visible end"), "the subject");
    let shown = format!(
        "feat: visible start {} visible end",
        "\u{fffd}".repeat(HIDDEN.len())
    );
    assert!(page.contains(&shown), "one U+FFFD for each character");
    assert!(
        page.contains("src/a\u{fffd}sr.\u{fffd}r\u{fffd}s"),
        "the file name, with the three characters replaced"
    );
    assert!(page.contains("src/plain.rs"), "an ordinary file name");
    // The page names no author, so the author's name is nowhere in it.
    assert!(!page.contains("Xyzzy"), "the author's name");
    // The link to the file still names the file: its characters are percent-encoded.
    assert!(
        page.contains("/src/a%E2%80%AEsr.%E2%80%8Br%01s#L1"),
        "the source link keeps the real path"
    );
}

#[test]
fn ordinary_text_with_a_tab_and_other_scripts_is_written_as_it_is() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", CFG);
    repo.commit("chore: adopt (#1)");
    let marker = "fn a() {} // discipline:allow(time-estimates) a quoted release plan\n";
    repo.write("src/caf\u{e9} \u{65e5}\u{672c}.rs", marker);
    repo.commit("feat: caf\u{e9}\t\u{65e5}\u{672c} \u{5d0}\u{5d1} \u{1f600}");
    let page = audit_page(&repo);
    assert!(
        page.contains("feat: caf\u{e9}\t\u{65e5}\u{672c} \u{5d0}\u{5d1} \u{1f600}"),
        "the subject"
    );
    assert!(page.contains("src/caf\u{e9} \u{65e5}\u{672c}.rs"));
    assert!(!page.contains('\u{fffd}'), "nothing was replaced");
}
