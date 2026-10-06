//! Findings that used to share a fingerprint are told apart (`docs/ARCHITECTURE.md`, the
//! fingerprint section).
//!
//! Three cases: a finding on a line whose text repeats in its file, a finding on the pull
//! request body, and two `manifest-sync` rules that name one manifest. For each, the
//! tests hold that the findings have different fingerprints, that a baseline entry for
//! one does not accept the other in a later change, and that the fingerprint a single
//! occurrence had before is the one it has now.

mod common;
use common::{Repo, Run};
use serde_json::Value;

fn of_code(run: &Run, gate: &str, code: &str) -> Vec<Value> {
    assert!(
        serde_json::from_str::<Value>(&run.stdout).is_ok(),
        "no report: {}\n{}",
        run.stdout,
        run.stderr
    );
    run.violations(gate)
        .into_iter()
        .filter(|v| v["code"] == code)
        .collect()
}

fn fingerprints(found: &[Value]) -> Vec<String> {
    found
        .iter()
        .map(|v| v["fingerprint"].as_str().unwrap().to_string())
        .collect()
}

/// A version-2 baseline holding exactly `finding`, as `discipline baseline --write`
/// records one.
fn baseline_of(repo: &Repo, gate: &str, finding: &Value) -> String {
    let entry = format!(
        "version = 2\n\n[[findings]]\ngate = \"{gate}\"\nrule = \"{}\"\npath = \"{}\"\nfingerprint = \"{}\"\n",
        finding["code"].as_str().unwrap(),
        finding["file"].as_str().unwrap_or(""),
        finding["fingerprint"].as_str().unwrap()
    );
    repo.write("discipline-baseline.toml", &entry);
    entry
}

/// Moves `work` onto `main`, so the next change starts from what was just committed.
fn land(repo: &Repo) {
    repo.git(&["checkout", "-q", "main"]);
    repo.git(&["merge", "-q", "--ff-only", "work"]);
    repo.git(&["checkout", "-q", "-B", "work", "main"]);
}

// ---- a line whose text repeats in its file -----------------------------------------------

struct Repeated {
    gate: &'static str,
    code: &'static str,
    path: &'static str,
    /// The file holding the first occurrence.
    one: &'static str,
    /// The file holding the first occurrence and, below it, a second with the same line.
    two: &'static str,
    /// `two` with unrelated lines added above, between and below the occurrences.
    two_moved: &'static str,
}

/// What the runs of one case showed.
struct Seen {
    /// Fingerprint of the first occurrence, reported alone.
    first_alone: String,
    /// Findings of a later change that adds the second occurrence, under a baseline of
    /// the first: `(line, fingerprint)`.
    second_later: Vec<(u64, String)>,
    /// Fingerprints of both occurrences added in one change.
    both: Vec<String>,
    /// Fingerprints of both occurrences once unrelated lines moved them.
    both_moved: Vec<String>,
}

fn probe(case: &Repeated) -> Seen {
    let repo = Repo::new();
    repo.write(case.path, case.one);
    repo.commit("change: first");
    let run = repo.check(&[]);
    let first = of_code(&run, case.gate, case.code);
    assert_eq!(
        first.len(),
        1,
        "the first occurrence is reported once: {}\n{}",
        run.stdout,
        run.stderr
    );
    land(&repo);
    baseline_of(&repo, case.gate, &first[0]);
    repo.write(case.path, case.two);
    repo.commit("change: second");
    let run = repo.check(&[]);
    let second_later = of_code(&run, case.gate, case.code)
        .iter()
        .map(|v| {
            (
                v["line"].as_u64().unwrap(),
                v["fingerprint"].as_str().unwrap().to_string(),
            )
        })
        .collect();

    let both = |content: &str| {
        let repo = Repo::new();
        repo.write(case.path, content);
        repo.commit("change: both");
        let run = repo.check(&[]);
        fingerprints(&of_code(&run, case.gate, case.code))
    };
    Seen {
        first_alone: first[0]["fingerprint"].as_str().unwrap().to_string(),
        second_later,
        both: both(case.two),
        both_moved: both(case.two_moved),
    }
}

/// Every property that does not hold, named.
fn told_apart(case: &Repeated, seen: &Seen) -> Result<(), Vec<String>> {
    let mut failed = Vec::new();
    if seen.second_later.len() != 1 {
        failed.push(format!(
            "a baseline entry for the first `{}` accepts a second on an identical line added later: reported {:?}",
            case.code, seen.second_later
        ));
    } else if seen.second_later[0].1 == seen.first_alone {
        failed.push(format!(
            "the second `{}` has the fingerprint of the first",
            case.code
        ));
    }
    if seen.both.len() != 2 || seen.both[0] == seen.both[1] {
        failed.push(format!(
            "two `{}` on identical lines do not have two fingerprints: {:?}",
            case.code, seen.both
        ));
    }
    if !seen.both.contains(&seen.first_alone) {
        failed.push(format!(
            "the first `{}` changes fingerprint when a second is present",
            case.code
        ));
    }
    if seen.both != seen.both_moved {
        failed.push(format!(
            "the fingerprints of `{}` change when unrelated lines move: {:?} then {:?}",
            case.code, seen.both, seen.both_moved
        ));
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed)
    }
}

#[test]
fn two_handlers_on_identical_lines_are_told_apart() {
    let case = Repeated {
        gate: "error-swallowing",
        code: "error-swallowing/empty-error-handler-added",
        path: "pkg/io.py",
        one: "def load(p):\n    try:\n        return open(p).read()\n    except Exception:\n        pass\n",
        two: "def load(p):\n    try:\n        return open(p).read()\n    except Exception:\n        pass\n\n\ndef save(p, text):\n    try:\n        open(p, 'w').write(text)\n    except Exception:\n        pass\n",
        two_moved: "import os\n\n\ndef load(p):\n    try:\n        return open(p).read()\n    except Exception:\n        pass\n\n\ndef size(p):\n    return os.path.getsize(p)\n\n\ndef save(p, text):\n    try:\n        open(p, 'w').write(text)\n    except Exception:\n        pass\n\n\ndef name(p):\n    return os.path.basename(p)\n",
    };
    let seen = probe(&case);
    assert_eq!(told_apart(&case, &seen), Ok(()));
}

#[test]
fn two_unsafe_blocks_on_identical_lines_are_told_apart() {
    let case = Repeated {
        gate: "unsafe-safety-comment",
        code: "unsafe-safety-comment/safety-comment-missing",
        path: "src/raw.rs",
        one: "pub fn first(p: *const u8) -> u8 {\n    unsafe { *p }\n}\n",
        two: "pub fn first(p: *const u8) -> u8 {\n    unsafe { *p }\n}\n\npub fn second(p: *const u8) -> u8 {\n    unsafe { *p }\n}\n",
        two_moved: "pub const WIDTH: usize = 8;\n\npub fn first(p: *const u8) -> u8 {\n    unsafe { *p }\n}\n\npub fn width() -> usize {\n    WIDTH\n}\n\npub fn second(p: *const u8) -> u8 {\n    unsafe { *p }\n}\n\npub fn twice() -> usize {\n    WIDTH * 2\n}\n",
    };
    let seen = probe(&case);
    assert_eq!(told_apart(&case, &seen), Ok(()));
}

#[test]
fn two_estimates_on_identical_lines_of_a_document_are_told_apart() {
    let case = Repeated {
        gate: "time-estimates",
        code: "time-estimates/time-estimate",
        path: "docs/rollout.md",
        one: "# Rollout\n\n## Reader\n\nThe migration takes 2 weeks.\n",
        two: "# Rollout\n\n## Reader\n\nThe migration takes 2 weeks.\n\n## Writer\n\nThe migration takes 2 weeks.\n",
        two_moved: "# Rollout\n\nOrdered by dependency.\n\n## Reader\n\nThe migration takes 2 weeks.\n\nBlocked on the schema change.\n\n## Writer\n\nThe migration takes 2 weeks.\n\n## Index\n\nRuns after the writer.\n",
    };
    let seen = probe(&case);
    assert_eq!(told_apart(&case, &seen), Ok(()));
}

/// A baseline written before occurrences were told apart holds one entry per occurrence,
/// all with the fingerprint of the first: each still accepts its occurrence, and a third
/// occurrence added later is reported.
#[test]
fn entries_recorded_with_one_fingerprint_for_two_identical_lines_still_match() {
    let repo = Repo::new();
    let path = "docs/rollout.md";
    let two = "# Rollout\n\n## Reader\n\nThe migration takes 2 weeks.\n\n## Writer\n\nThe migration takes 2 weeks.\n";
    repo.write(
        path,
        "# Rollout\n\n## Reader\n\nThe migration takes 2 weeks.\n",
    );
    repo.commit("change: first");
    let run = repo.check(&[]);
    let first = of_code(&run, "time-estimates", "time-estimates/time-estimate");
    assert_eq!(first.len(), 1, "{}", run.stdout);
    let entry = baseline_of(&repo, "time-estimates", &first[0]);
    let one_entry = entry.split_once("\n\n").unwrap().1.to_string();
    repo.write("discipline-baseline.toml", &format!("{entry}\n{one_entry}"));
    repo.write(path, two);
    repo.commit("change: second");
    let run = repo.check(&[]);
    assert_eq!(
        of_code(&run, "time-estimates", "time-estimates/time-estimate").len(),
        0,
        "two entries accept two occurrences: {}",
        run.stdout
    );
    assert_eq!(run.json()["baselined"], 2, "{}", run.stdout);

    repo.write(
        path,
        &format!("{two}\n## Index\n\nThe migration takes 2 weeks.\n"),
    );
    repo.commit("change: third");
    let run = repo.check(&[]);
    let left = of_code(&run, "time-estimates", "time-estimates/time-estimate");
    assert_eq!(left.len(), 1, "{}", run.stdout);
    assert_eq!(left[0]["line"], 13, "{}", run.stdout);
    assert_eq!(run.json()["baselined"], 2, "{}", run.stdout);
}

// ---- a finding on the pull request body ----------------------------------------------------

struct OnBody {
    gate: &'static str,
    code: &'static str,
    /// `discipline.toml` on the base, when the gate needs one.
    config: Option<&'static str>,
    /// The body of one pull request, and of another whose finding is a different one.
    first: String,
    second: String,
    /// `first` with a paragraph above its finding.
    first_moved: String,
}

struct BodySeen {
    first: Vec<String>,
    second: Vec<String>,
    first_moved: Vec<String>,
    /// Findings the second body gives under a baseline of the first body's finding.
    second_under_baseline: usize,
    /// Findings the first body gives under that baseline.
    first_under_baseline: usize,
    /// The baseline file written from the first body's finding.
    baseline: String,
}

fn probe_body(case: &OnBody) -> BodySeen {
    let repo = Repo::new();
    if let Some(config) = case.config {
        repo.commit_base("discipline.toml", config, "chore: configure");
    }
    repo.write("docs/notes.txt", "ordering only\n");
    repo.commit("docs: notes");
    let with = |body: &str| {
        let run = repo.check_with_pr(&[], body);
        of_code(&run, case.gate, case.code)
    };
    let first = with(&case.first);
    assert_eq!(first.len(), 1, "the first body gives one finding");
    let second = fingerprints(&with(&case.second));
    let first_moved = fingerprints(&with(&case.first_moved));
    let baseline = baseline_of(&repo, case.gate, &first[0]);
    repo.commit("chore: baseline");
    BodySeen {
        first: fingerprints(&first),
        second,
        first_moved,
        second_under_baseline: with(&case.second).len(),
        first_under_baseline: with(&case.first).len(),
        baseline,
    }
}

fn bodies_told_apart(case: &OnBody, seen: &BodySeen) -> Result<(), Vec<String>> {
    let mut failed = Vec::new();
    if seen.second.len() != 1 || seen.second == seen.first {
        failed.push(format!(
            "two different `{}` on two pull request bodies share a fingerprint",
            case.code
        ));
    }
    if seen.first_moved != seen.first {
        failed.push(format!(
            "`{}` changes fingerprint when its line moves in the body",
            case.code
        ));
    }
    if seen.second_under_baseline != 1 {
        failed.push(format!(
            "a baseline entry for `{}` on one body accepts another finding on a later body",
            case.code
        ));
    }
    if seen.first_under_baseline != 0 {
        failed.push(format!(
            "a baseline entry for `{}` does not accept the finding it was written for",
            case.code
        ));
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed)
    }
}

#[test]
fn estimates_on_two_pull_request_bodies_are_told_apart() {
    let case = OnBody {
        gate: "time-estimates",
        code: "time-estimates/time-estimate",
        config: None,
        first: "Refs #101.\n\nThe migration takes 2 weeks.\n".into(),
        second: "Refs #101.\n\nThe cutover takes 3 weeks.\n".into(),
        first_moved: "Refs #101.\n\nOrdered by dependency.\n\nThe migration takes 2 weeks.\n"
            .into(),
    };
    let seen = probe_body(&case);
    assert_eq!(bodies_told_apart(&case, &seen), Ok(()));
}

/// The fingerprint is a hash of the line: neither the report's fingerprint nor the
/// baseline entry holds the text that was matched.
#[test]
fn leaks_on_two_pull_request_bodies_are_told_apart_without_their_text() {
    let home = |user: &str| format!("/{}/{user}/notes", "Users");
    let case = OnBody {
        gate: "pii",
        code: "pii/host-or-pii-leak",
        config: None,
        first: format!("Refs #101.\n\nMeasured under {}.\n", home("alice")),
        second: format!("Refs #101.\n\nMeasured under {}.\n", home("bob")),
        first_moved: format!(
            "Refs #101.\n\nOrdered by dependency.\n\nMeasured under {}.\n",
            home("alice")
        ),
    };
    let seen = probe_body(&case);
    assert_eq!(bodies_told_apart(&case, &seen), Ok(()));
    assert!(
        !seen.baseline.contains("alice") && !seen.baseline.contains("Users"),
        "{}",
        seen.baseline
    );
    assert!(seen.first[0].len() == 64 && seen.first[0].chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn checklist_claims_on_two_pull_request_bodies_are_told_apart() {
    let case = OnBody {
        gate: "pr-checklist",
        code: "pr-checklist/checklist-claims-tests",
        config: Some("[meta]\nversion = 1\nname = \"t\"\n\n[gates.pr-checklist]\nenabled = true\n"),
        first: "Refs #101.\n\n- [x] Added tests for the reader\n".into(),
        second: "Refs #101.\n\n- [x] Added tests for the writer\n".into(),
        first_moved: "Refs #101.\n\nOrdered by dependency.\n\n- [x] Added tests for the reader\n"
            .into(),
    };
    let seen = probe_body(&case);
    assert_eq!(bodies_told_apart(&case, &seen), Ok(()));
}

/// The title and the body are fingerprinted by what they say: the finding on one pull
/// request's body is not the finding on another's.
#[test]
fn invisible_characters_in_two_pull_request_bodies_are_told_apart() {
    let case = OnBody {
        gate: "instruction-smuggling",
        code: "instruction-smuggling/invisible-characters-in-description",
        config: None,
        first: "Tidies\u{200b} the plan. Refs #101.\n".into(),
        second: "Reorders\u{200b} the plan. Refs #101.\n".into(),
        // One finding for the whole body: there is no line to move.
        first_moved: "Tidies\u{200b} the plan. Refs #101.\n".into(),
    };
    let seen = probe_body(&case);
    assert_eq!(bodies_told_apart(&case, &seen), Ok(()));
}

// ---- two `manifest-sync` rules on one manifest -----------------------------------------------

const TWO_RULES: &str = r#"[meta]
version = 1
name = "t"

[gates.manifest-sync]
enabled = true

[[gates.manifest-sync.rules]]
manifest = "MANIFEST"
extract_regex = '(?m)^(\S+)$'
watched_paths = ["src/**"]

[[gates.manifest-sync.rules]]
manifest = "MANIFEST"
extract_regex = '(?m)^(\S+)$'
watched_paths = ["docs/**"]
"#;

/// A repository whose `MANIFEST` lists every file two rules watch.
fn two_rules_in_sync() -> Repo {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("discipline.toml", TWO_RULES),
            ("MANIFEST", "src/lib.rs\ndocs/plan.md\n"),
        ],
        "chore: manifest",
    );
    repo
}

fn drift(run: &Run) -> Vec<Value> {
    of_code(run, "manifest-sync", "manifest-sync/manifest-drift")
}

#[test]
fn two_rules_on_one_manifest_are_told_apart() {
    // The first rule alone.
    let repo = two_rules_in_sync();
    repo.write("src/extra.rs", "pub fn extra() {}\n");
    repo.commit("feat: extra source");
    let run = repo.check(&[]);
    let first = drift(&run);
    assert_eq!(first.len(), 1, "{}\n{}", run.stdout, run.stderr);

    // Both rules in one run: two findings, two fingerprints, and the documented exit.
    repo.write("docs/extra.md", "# Extra\n");
    repo.commit("docs: extra");
    let run = repo.check(&[]);
    let both = drift(&run);
    assert_eq!(run.code, 1, "{}\n{}", run.stdout, run.stderr);
    assert_eq!(both.len(), 2, "{}", run.stdout);
    let prints = fingerprints(&both);
    assert_ne!(prints[0], prints[1]);
    assert!(
        prints.contains(&first[0]["fingerprint"].as_str().unwrap().to_string()),
        "the first rule keeps its fingerprint when the second reports too"
    );

    // A baseline entry for the first rule's finding leaves the second rule's.
    baseline_of(&repo, "manifest-sync", &first[0]);
    repo.commit("chore: baseline");
    let run = repo.check(&[]);
    let left = drift(&run);
    assert_eq!(left.len(), 1, "{}", run.stdout);
    assert!(
        left[0]["message"]
            .as_str()
            .unwrap()
            .contains("docs/extra.md"),
        "{}",
        run.stdout
    );

    // The second rule alone has the fingerprint it has beside the first.
    let repo = two_rules_in_sync();
    repo.write("docs/extra.md", "# Extra\n");
    repo.commit("docs: extra");
    let run = repo.check(&[]);
    let second = drift(&run);
    assert_eq!(second.len(), 1, "{}", run.stdout);
    assert_eq!(second[0]["fingerprint"], left[0]["fingerprint"]);
}

/// A manifest one rule names keeps the fingerprint it had: its code and its path, with
/// nothing else hashed.
#[test]
fn one_rule_on_a_manifest_keeps_its_fingerprint() {
    let hex = |text: &str| discipline::report::gitlab::sha256_hex(text.as_bytes());
    let repo = two_rules_in_sync();
    repo.write("src/extra.rs", "pub fn extra() {}\n");
    repo.commit("feat: extra source");
    let run = repo.check(&[]);
    let first = drift(&run);
    assert_eq!(first.len(), 1, "{}", run.stdout);
    assert_eq!(
        first[0]["fingerprint"].as_str().unwrap(),
        hex(&format!(
            "v2:manifest-sync/manifest-drift:MANIFEST:{}",
            hex("")
        ))
    );
}
