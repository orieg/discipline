//! `discipline audit --format html`: the page for a fixed history is pinned byte for byte,
//! loads nothing from the network, escapes what authors wrote, and links to the forge the
//! `origin` remote names.
//!
//! `DISCIPLINE_BLESS_AUDIT_HTML=1 cargo test --test test_audit_html` rewrites the golden
//! file after an intended change to the page.

mod common;

use common::Repo;
use discipline::audit::{
    baseline_records, config_changes, directive_records, ChangeInfoRef, Links, Record, Summary,
};
use discipline::forge::{Forge, ForgeKind};

const GOLDEN: &str = "tests/fixtures/audit/report.html";

fn change(ord: usize, pr: Option<u64>, subject: &str) -> ChangeInfoRef {
    ChangeInfoRef {
        sha: format!("{:0>40}", format!("{ord}abc")),
        pr,
        // One change a day, newest first, from 2026-09-20.
        time: 1_789_862_400 - ord as i64 * 86_400,
        ord,
        subject: subject.to_string(),
    }
}

const CFG: &str = common::CONFIG_HEAD;

/// Newest first: a guard-gate loosening waived in the same change (#6), a hidden waiver
/// with an author-written subject that needs escaping (#5), a pii exemption pushed with no
/// pull request (4), its tightening (#3), baseline growth (#2), a skipped issue link (#1).
fn summary() -> Summary {
    let mut records: Vec<Record> = Vec::new();
    let mut tightenings: Vec<Record> = Vec::new();
    let c6 = change(0, Some(6), "chore: widen ratifiers (#6)");
    records.extend(directive_records(
        "s\n\nallow-gate-weakening: ratified-paths second ratifier",
        &c6,
        false,
    ));
    records.extend(
        config_changes(
            Some(&format!(
                "{CFG}[gates.ratified-paths]\nratifiers = [\"a\"]\n"
            )),
            Some(&format!(
                "{CFG}[gates.ratified-paths]\nratifiers = [\"a\", \"b\"]\n"
            )),
            &c6,
        )
        .0,
    );
    records.extend(directive_records(
        "s\n\n<!-- allow-stub: fn_a later -->",
        &change(1, Some(5), "feat: <script>alert(1)</script> & stubs (#5)"),
        false,
    ));
    let c4 = change(2, None, "chore: exempt a");
    records.extend(
        config_changes(
            Some(&format!("{CFG}[gates.pii]\nexempt_paths = []\n")),
            Some(&format!("{CFG}[gates.pii]\nexempt_paths = [\"a/**\"]\n")),
            &c4,
        )
        .0,
    );
    let c3 = change(3, Some(3), "chore: drop the exemption (#3)");
    tightenings.extend(
        config_changes(
            Some(&format!("{CFG}[gates.pii]\nexempt_paths = [\"a/**\"]\n")),
            Some(&format!("{CFG}[gates.pii]\nexempt_paths = []\n")),
            &c3,
        )
        .1,
    );
    records.extend(baseline_records(
        None,
        Some("version = 2\n[[findings]]\ngate = \"stub-bodies\"\nrule = \"r\"\npath = \"src/a.rs\"\nfingerprint = \"\"\n"),
        &change(4, Some(2), "chore: grandfather (#2)"),
    ));
    records.extend(directive_records(
        "s\n\nno-issue: release bookkeeping",
        &change(5, Some(1), "chore: release (#1)"),
        false,
    ));
    let mut s = Summary::from_records("main".into(), 8, records, tightenings);
    s.version = "test".into();
    s.tip = "f".repeat(40);
    s.links = Some(Links::for_forge(&Forge {
        kind: ForgeKind::GitHub,
        url: "https://github.com".into(),
        repo: "o/r".into(),
    }));
    s
}

#[test]
fn the_page_for_a_fixed_history_matches_the_golden_file() {
    let page = discipline::audit_html::render(&summary());
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(GOLDEN);
    if std::env::var("DISCIPLINE_BLESS_AUDIT_HTML").is_ok_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &page).unwrap();
    }
    let golden = std::fs::read_to_string(&path).expect("run with DISCIPLINE_BLESS_AUDIT_HTML=1");
    assert!(
        page == golden,
        "the audit page changed; if intended, rerun with DISCIPLINE_BLESS_AUDIT_HTML=1 and review the diff of {GOLDEN}"
    );
}

#[test]
fn the_page_loads_nothing_and_escapes_what_authors_wrote() {
    let page = discipline::audit_html::render(&summary());
    assert!(page.starts_with("<!doctype html>"));
    for external in [
        "<script src",
        "<link rel=\"stylesheet\"",
        "url(",
        "@import",
        "<iframe",
        "<img",
    ] {
        assert!(!page.contains(external), "{external}");
    }
    // The only script is the one the page carries.
    assert_eq!(page.matches("<script").count(), 1);
    assert!(!page.contains("<script>alert"));
    assert!(page.contains("&lt;script&gt;alert(1)&lt;/script&gt; &amp; stubs"));
    // Signals lead, with their next action, and what was not checked says so.
    let decisions = &page[page.find(r#"<ol class="decisions">"#).unwrap()..];
    assert!(decisions.find("Look first").unwrap() < decisions.find("Look soon").unwrap());
    assert!(page.contains("Not checked"));
    // Every source link points at the forge the links name, at the change's commit.
    assert!(page.contains(r#"href="https://github.com/o/r/blob/"#));
    assert!(
        page.contains("/discipline.toml#L5"),
        "the loosened option's line"
    );
}

#[test]
fn the_command_writes_the_page_and_links_to_the_origin_forge() {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://gitlab.example.com/group/sub/proj.git",
    ]);
    repo.write(
        "discipline.toml",
        &format!("{CFG}[gates.pii]\nexempt_paths = []\n"),
    );
    repo.commit("chore: adopt (#1)");
    repo.write(
        "discipline.toml",
        &format!("{CFG}[gates.pii]\nexempt_paths = [\"a/**\"]\n"),
    );
    repo.commit("chore: exempt a (#2)");
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
    assert!(run.stdout.is_empty(), "{}", run.stdout);
    let page = std::fs::read_to_string(&out).unwrap();
    assert!(
        page.contains("loosened-not-restored") || page.contains("never tightened back"),
        "{page}"
    );
    assert!(
        page.contains("https://gitlab.example.com/group/sub/proj/-/merge_requests/2"),
        "GitLab pull request link"
    );
    assert!(
        page.contains("https://gitlab.example.com/group/sub/proj/-/blob/"),
        "GitLab file link"
    );
    assert!(
        !page.contains("github.com"),
        "no GitHub link for a GitLab remote"
    );
}

#[test]
fn a_source_link_stays_in_the_repository_whatever_the_path() {
    // A browser reads a `%2e%2e` segment as `..`: unencoded, a directory with that name
    // walks the link out of the repository to another one on the same forge, and `#` or
    // `?` in a file name cuts the link short.
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.git(&["remote", "add", "origin", "https://github.com/o/r.git"]);
    repo.write("discipline.toml", CFG);
    repo.commit("chore: adopt (#1)");
    let marker = "fn a() {} // discipline:allow(time-estimates) a quoted release plan\n";
    for path in [
        "%2e%2e/%2e%2e/%2e%2e/%2e%2e/evil/r/blob/main/x.rs",
        "src/a #?.rs",
        "src/plain.rs",
    ] {
        repo.write(path, marker);
    }
    repo.commit("feat: markers (#2)");
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
    let page = std::fs::read_to_string(&out).unwrap();
    let open = r#"class="src-link" href=""#;
    let hrefs: Vec<&str> = page
        .match_indices(open)
        .map(|(i, _)| {
            let rest = &page[i + open.len()..];
            &rest[..rest.find('"').unwrap()]
        })
        .collect();
    let ends = |tail: &str| hrefs.iter().any(|h| h.ends_with(tail));
    assert!(ends("/src/plain.rs#L1"), "{hrefs:?}");
    assert!(
        ends("/%252e%252e/%252e%252e/%252e%252e/%252e%252e/evil/r/blob/main/x.rs#L1"),
        "{hrefs:?}"
    );
    assert!(ends("/src/a%20%23%3F.rs#L1"), "{hrefs:?}");
    for h in &hrefs {
        assert!(h.starts_with("https://github.com/o/r/blob/"), "{h}");
        assert!(
            !h.to_ascii_lowercase().contains("%2e"),
            "a dot segment: {h}"
        );
        assert_eq!(h.matches('#').count(), 1, "{h}");
    }
}
