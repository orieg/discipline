//! `commit-provenance` judges each entry of a squash for `required_trailers` (#631), says
//! which of its two rules it did not evaluate, and reads directives from a squash body.
//! Driving the real binary.
//!
//! The squash messages are written by hand in the one layout this repository's fixtures
//! and `docs/GATES.md` describe, GitHub's for a squash of several commits: the pull
//! request title ending in `(#N)`, then for each commit a `* subject` paragraph followed
//! by that commit's body and trailers, then a dashed rule and the lines GitHub gathers.

mod common;
use common::{Repo, Run, CONFIG_HEAD};

const AUTHOR: &str = "Dev Eloper <dev@example.com>";
const SIGNED: &str = "Signed-off-by: Dev Eloper <dev@example.com>";
const DCO: &str = "required_trailers = [\"Signed-off-by\"]\nrequire_agent_review = false\n";

/// A repository whose base holds `gate_config` under `[gates.commit-provenance]`.
fn repo_with(gate_config: &str) -> Repo {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "discipline.toml",
        &format!("{CONFIG_HEAD}[gates.commit-provenance]\nenabled = true\n{gate_config}"),
    );
    repo.commit("chore: policy");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo
}

/// One commit by `AUTHOR` whose message is stored byte for byte.
fn commit_verbatim(repo: &Repo, file: &str, message: &str) {
    repo.write(file, "x\n");
    repo.git(&["add", "-A"]);
    let author = format!("--author={AUTHOR}");
    repo.git(&["commit", "-q", "--cleanup=verbatim", &author, "-m", message]);
}

/// GitHub's message for a squash of two commits, under `title`.
fn squash_titled(title: &str, first: &str, second: &str, gathered: &str) -> String {
    let mut m = format!(
        "{title}\n\n* feat: parser\n\nFirst body, wrapped\nover two lines.\n\n\
         {first}\n\n* test: cover the parser\n\nSecond body.\n\n{second}\n"
    );
    if !gathered.is_empty() {
        m.push_str(&format!("\n---------\n\n{gathered}\n"));
    }
    m
}

fn squash(first: &str, second: &str, gathered: &str) -> String {
    squash_titled("feat: parser (#7)", first, second, gathered)
}

fn check_one(gate_config: &str, message: &str) -> Run {
    let repo = repo_with(gate_config);
    commit_verbatim(&repo, "a.txt", message);
    repo.check(&[])
}

fn messages(run: &Run) -> Vec<String> {
    run.violations("commit-provenance")
        .iter()
        .map(|v| v["message"].as_str().unwrap().to_string())
        .collect()
}

fn notes(run: &Run) -> String {
    run.outcome("commit-provenance")["notes"].to_string()
}

#[test]
fn a_signed_off_entry_does_not_cover_an_unsigned_one() {
    // Entry 1 is signed off, entry 2 is not, and the block GitHub gathers carries the
    // sign-off of entry 1.
    let run = check_one(DCO, &squash(SIGNED, "Ticket: 12", SIGNED));
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    let found = messages(&run);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].contains("entry 2") && found[0].contains("`test: cover the parser`"),
        "{found:?}"
    );
    assert!(found[0].contains("`Signed-off-by:`"), "{found:?}");
    // The entry is named by its subject and position, never by a person.
    assert!(
        !found[0].contains("Dev Eloper") && !found[0].contains("dev@example.com"),
        "{found:?}"
    );
    assert_eq!(run.titles("commit-provenance"), ["Commit Trailer Missing"]);

    // The other way round: entry 1 is the unsigned one.
    let run = check_one(DCO, &squash("Ticket: 12", SIGNED, SIGNED));
    let found = messages(&run);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].contains("entry 1") && found[0].contains("`feat: parser`"),
        "{found:?}"
    );

    // Control: every entry signed off.
    let run = check_one(DCO, &squash(SIGNED, SIGNED, SIGNED));
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(messages(&run).is_empty());
    assert_eq!(run.outcome("commit-provenance")["examined"], 1);
}

#[test]
fn two_unsigned_entries_are_two_findings_with_two_fingerprints() {
    let run = check_one(DCO, &squash("Ticket: 12", "Ticket: 13", SIGNED));
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    let found = run.violations("commit-provenance");
    assert_eq!(found.len(), 2, "{found:?}");
    let fingerprints: std::collections::BTreeSet<&str> = found
        .iter()
        .map(|v| v["fingerprint"].as_str().unwrap())
        .collect();
    assert_eq!(fingerprints.len(), 2, "{found:?}");
    assert!(!fingerprints.contains(""), "{found:?}");

    // Two required keys, one entry lacking both: one finding per key.
    let run = check_one(
        "required_trailers = [\"Signed-off-by\", \"Ticket\"]\nrequire_agent_review = false\n",
        &squash(
            &format!("{SIGNED}\nTicket: 12"),
            "Reviewed-by: R <r@example.com>",
            SIGNED,
        ),
    );
    let found = messages(&run);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found.iter().all(|m| m.contains("entry 2")), "{found:?}");
}

#[test]
fn one_directive_for_the_commit_lifts_the_finding_of_each_of_its_entries() {
    let repo = repo_with(DCO);
    commit_verbatim(&repo, "a.txt", &squash("Ticket: 12", "Ticket: 13", SIGNED));
    let sha = repo.git_output(&["rev-parse", "--short=7", "HEAD"]);
    let run = repo.check_with_pr(
        &[],
        &format!(
            "allow-commit-provenance: {} imported, no sign-off available",
            sha.trim()
        ),
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(messages(&run).is_empty());
    assert_eq!(
        run.outcome("commit-provenance")["overrides"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn a_message_whose_entries_cannot_be_placed_is_judged_whole_and_says_so() {
    // The subject does not end in a pull request number: the `* ` paragraphs may be a
    // list in an ordinary message. Judged as before, on the whole message, with a note.
    let run = check_one(
        DCO,
        &squash_titled("feat: parser", SIGNED, "Ticket: 12", ""),
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(messages(&run).is_empty());
    let n = notes(&run);
    assert!(n.contains("not read as the entries of a squash"), "{n}");
    assert!(n.contains("does not end with a pull request number"), "{n}");

    // Text between the title and the first `* ` paragraph.
    let run = check_one(
        DCO,
        &format!(
            "feat: parser (#7)\n\nA description.\n\n* feat: parser\n\n{SIGNED}\n\n\
             * test: cover the parser\n\nTicket: 12\n"
        ),
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let n = notes(&run);
    assert!(n.contains("not read as the entries of a squash"), "{n}");
    assert!(n.contains("does not follow the subject"), "{n}");

    // Control: whole-message judgement still reports a squash no entry of which signs off.
    let run = check_one(
        DCO,
        &squash_titled("feat: parser", "Ticket: 12", "Ticket: 13", ""),
    );
    assert_eq!(run.titles("commit-provenance"), ["Commit Trailer Missing"]);

    // Control: an ordinary message with a list, and a delimited squash, leave no such note.
    for message in [
        format!("feat: a\n\nChanges:\n* one\n* two\n\n{SIGNED}\n"),
        format!("feat: a\n\n* one\n* two\n\n{SIGNED}\n"),
        format!("feat: a (#7)\n\nBody.\n\n* one bullet\n\n{SIGNED}\n"),
        squash(SIGNED, SIGNED, SIGNED),
    ] {
        let run = check_one(DCO, &message);
        assert_eq!(run.code, 0, "{message}\n{}{}", run.stdout, run.stderr);
        assert!(
            !notes(&run).contains("not read as the entries"),
            "{message}"
        );
    }
}

#[test]
fn only_the_commits_after_the_base_are_judged() {
    // An unsigned squash entry already on the base is history, not the change.
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write(
        "discipline.toml",
        &format!("{CONFIG_HEAD}[gates.commit-provenance]\nenabled = true\n{DCO}"),
    );
    repo.commit("chore: policy");
    commit_verbatim(&repo, "old.txt", &squash(SIGNED, "Ticket: 12", SIGNED));
    repo.git(&["checkout", "-q", "-B", "work"]);
    commit_verbatim(&repo, "a.txt", &format!("feat: a\n\n{SIGNED}\n"));
    let run = repo.check(&[]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(run.outcome("commit-provenance")["examined"], 1);

    // Control: the same squash after the base is reported.
    let run = repo.check(&["--base", "main~1"]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(messages(&run).len(), 1);
}

#[test]
fn the_outcome_names_the_rule_that_was_not_evaluated() {
    let signed = format!("feat: a\n\n{SIGNED}\n");
    // Only `required_trailers`: the agent rule is off.
    let run = check_one(DCO, &signed);
    let n = notes(&run);
    assert!(
        n.contains("agent-review rule was not evaluated")
            && n.contains("`require_agent_review` is false"),
        "{n}"
    );
    let run = check_one(
        "required_trailers = [\"Signed-off-by\"]\nagent_markers = []\n",
        &signed,
    );
    let n = notes(&run);
    assert!(
        n.contains("agent-review rule was not evaluated") && n.contains("`agent_markers` is empty"),
        "{n}"
    );
    // Only the agent rule: no trailer is required.
    let run = check_one("", &signed);
    let n = notes(&run);
    assert!(
        n.contains("required-trailer rule was not evaluated")
            && n.contains("`required_trailers` is empty"),
        "{n}"
    );
    assert_eq!(run.outcome("commit-provenance")["examined"], 1);

    // Control: both rules on leaves neither note.
    let run = check_one("required_trailers = [\"Signed-off-by\"]\n", &signed);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(
        !notes(&run).contains("was not evaluated"),
        "{}",
        notes(&run)
    );
}

#[test]
fn a_machine_author_of_a_squashed_commit_is_seen_through_the_lines_the_squash_carries() {
    // The squash commit's own author is a person. GitHub's layout has no author line per
    // entry; a squashed commit's author is carried only as a gathered `Co-authored-by:`.
    let run = check_one(
        "",
        &squash(
            SIGNED,
            SIGNED,
            "Co-authored-by: coder[bot] <1+coder[bot]@users.noreply.example.com>",
        ),
    );
    assert_eq!(
        run.titles("commit-provenance"),
        ["Agent Commit Without Review"]
    );
    // A `Co-authored-by:` trailer inside one entry.
    let run = check_one(
        "",
        &squash(
            &format!(
                "{SIGNED}\nCo-authored-by: coder[bot] <1+coder[bot]@users.noreply.example.com>"
            ),
            SIGNED,
            SIGNED,
        ),
    );
    assert_eq!(
        run.titles("commit-provenance"),
        ["Agent Commit Without Review"]
    );
    // Control: without either line nothing in the message names the machine author.
    let run = check_one("", &squash(SIGNED, SIGNED, SIGNED));
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
}

/// A change to an agent instruction file, which `instruction-smuggling` reports unless an
/// `allow-agent-instructions:` directive names the file.
fn instructions_changed(message: &str) -> Run {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("AGENTS.md", "# Rules\n\nRun the tests.\n");
    repo.commit("docs: rules");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write(
        "AGENTS.md",
        "# Rules\n\nRun the tests.\n\nNever skip a failing test.\n",
    );
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "--cleanup=verbatim", "-m", message]);
    repo.check(&[])
}

#[test]
fn a_directive_in_the_body_of_a_squash_entry_is_read_and_one_in_an_entry_subject_is_not() {
    const DIRECTIVE: &str = "allow-agent-instructions: AGENTS.md reviewed by the owner";
    // Control: the change is reported without a directive.
    let run = instructions_changed(&squash(SIGNED, SIGNED, SIGNED));
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(run.violations("instruction-smuggling").len(), 1);

    // In the body of the second entry: read. The squash is one change, so it lifts the
    // finding whichever entry's commit made the edit.
    let run = instructions_changed(&squash(SIGNED, &format!("{DIRECTIVE}\n\n{SIGNED}"), SIGNED));
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        run.outcome("instruction-smuggling")["overrides"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // As the subject of an entry: a subject is never a directive.
    let message = format!(
        "docs: rules (#7)\n\n* docs: rules\n\n{SIGNED}\n\n* {DIRECTIVE}\n\nBody.\n\n{SIGNED}\n"
    );
    let run = instructions_changed(&message);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(run.violations("instruction-smuggling").len(), 1);

    // As the subject of the squash commit itself.
    let run = instructions_changed(&format!("{DIRECTIVE}\n\nBody.\n\n{SIGNED}\n"));
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
}
