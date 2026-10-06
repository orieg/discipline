//! `commit-provenance` on the commit a forge writes when it squash-merges a pull request
//! (#566), driving the real binary.
//!
//! The squash messages are written by hand in GitHub's layout for a squash of several
//! commits: the pull request title, then for each commit a `* subject` paragraph followed
//! by that commit's body and trailers, then, when the commits have co-authors, a dashed
//! rule and the `Signed-off-by:` / `Co-authored-by:` lines GitHub gathers:
//!
//! ```text
//! feat: parser (#7)
//!
//! * feat: parser
//!
//! First body.
//!
//! Reviewed-by: Rev Iewer <rev@example.com>
//! Signed-off-by: Dev Eloper <dev@example.com>
//!
//! * test: cover the parser
//!
//! Second body.
//!
//! Signed-off-by: Dev Eloper <dev@example.com>
//!
//! ---------
//!
//! Signed-off-by: Dev Eloper <dev@example.com>
//! Co-authored-by: Dev Eloper <dev@example.com>
//! ```

mod common;
use common::{Repo, Run, CONFIG_HEAD};

const AUTHOR: &str = "Dev Eloper <dev@example.com>";

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

/// GitHub's message for a squash of two commits.
fn squash(first_trailers: &str, second_trailers: &str, gathered: &str) -> String {
    let mut m = format!(
        "feat: parser (#7)\n\n* feat: parser\n\nFirst body, wrapped\nover two lines.\n\n\
         {first_trailers}\n\n* test: cover the parser\n\nSecond body.\n\n{second_trailers}\n"
    );
    if !gathered.is_empty() {
        m.push_str(&format!("\n---------\n\n{gathered}\n"));
    }
    m
}

fn check_one(gate_config: &str, message: &str) -> Run {
    let repo = repo_with(gate_config);
    commit_verbatim(&repo, "a.txt", message);
    repo.check(&[])
}

fn titles(run: &Run) -> Vec<String> {
    let mut t = run.titles("commit-provenance");
    t.sort();
    t
}

#[test]
fn a_review_in_an_earlier_entry_of_a_squash_is_read() {
    // The agent marker is in the block GitHub gathers; the review is in the first entry.
    let run = check_one(
        "",
        &squash(
            "Reviewed-by: Rev Iewer <rev@example.com>\nSigned-off-by: Dev Eloper <dev@example.com>",
            "Signed-off-by: Dev Eloper <dev@example.com>",
            "Signed-off-by: Dev Eloper <dev@example.com>\nCo-authored-by: Claude <agent@example.com>",
        ),
    );
    assert!(
        titles(&run).is_empty(),
        "{:?}",
        run.violations("commit-provenance")
    );
    assert_eq!(run.outcome("commit-provenance")["examined"], 1);

    // The review is in the last entry, directly before the dashed rule.
    let run = check_one(
        "",
        &squash(
            "Signed-off-by: Dev Eloper <dev@example.com>",
            "Reviewed-by: Rev Iewer <rev@example.com>",
            "Signed-off-by: Dev Eloper <dev@example.com>\nCo-authored-by: Claude <agent@example.com>",
        ),
    );
    assert!(
        titles(&run).is_empty(),
        "{:?}",
        run.violations("commit-provenance")
    );

    // Control: the same squash with no review in any entry is still reported.
    let run = check_one(
        "",
        &squash(
            "Signed-off-by: Dev Eloper <dev@example.com>",
            "Signed-off-by: Dev Eloper <dev@example.com>",
            "Signed-off-by: Dev Eloper <dev@example.com>\nCo-authored-by: Claude <agent@example.com>",
        ),
    );
    assert_eq!(run.code, 1);
    assert_eq!(titles(&run), ["Agent Commit Without Review"]);
}

#[test]
fn an_agent_marker_in_an_earlier_entry_of_a_squash_is_read() {
    // Nothing in the last entry or in the gathered block says an agent wrote any of it.
    let run = check_one(
        "",
        &squash(
            "Agent-Tool: coder 1.2\nSigned-off-by: Dev Eloper <dev@example.com>",
            "Signed-off-by: Dev Eloper <dev@example.com>",
            "Signed-off-by: Dev Eloper <dev@example.com>",
        ),
    );
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(titles(&run), ["Agent Commit Without Review"]);

    // The same without the gathered block: the last entry's trailers end the message.
    let run = check_one(
        "",
        &squash(
            "Agent-Tool: coder 1.2",
            "Signed-off-by: Dev Eloper <dev@example.com>",
            "",
        ),
    );
    assert_eq!(titles(&run), ["Agent Commit Without Review"]);

    // Control: a squash with no marker anywhere is not an agent commit.
    let run = check_one(
        "",
        &squash(
            "Signed-off-by: Dev Eloper <dev@example.com>",
            "Signed-off-by: Dev Eloper <dev@example.com>",
            "Signed-off-by: Dev Eloper <dev@example.com>",
        ),
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(titles(&run).is_empty());
}

#[test]
fn a_person_reviewing_their_own_agent_assisted_commit_is_reported_in_a_squash() {
    // The layout the removed `allow_author_review` tests covered: the review trailer
    // names the commit's author. Several commits squashed.
    let run = check_one(
        "",
        &squash(
            "Reviewed-by: Dev Eloper <dev@example.com>\nAgent-Tool: coder 1.2",
            "Signed-off-by: Dev Eloper <dev@example.com>",
            "Signed-off-by: Dev Eloper <dev@example.com>\nCo-authored-by: Claude <agent@example.com>",
        ),
    );
    assert_eq!(run.code, 1);
    assert_eq!(titles(&run), ["Agent Commit Reviewed By Its Author"]);

    // Control: one commit squashed, where GitHub splits the block into paragraphs.
    let run = check_one(
        "",
        "feat: parser (#7)\n\nBody.\n\nReviewed-by: Dev Eloper <dev@example.com>\n\n\
         Session: https://example.com/s\n\nCo-authored-by: Claude <agent@example.com>\n",
    );
    assert_eq!(titles(&run), ["Agent Commit Reviewed By Its Author"]);
}

#[test]
fn a_required_trailer_in_any_entry_of_a_squash_counts() {
    const CFG: &str = "required_trailers = [\"Ticket\"]\nrequire_agent_review = false\n";
    let run = check_one(
        CFG,
        &squash(
            "Ticket: 12\nSigned-off-by: Dev Eloper <dev@example.com>",
            "Signed-off-by: Dev Eloper <dev@example.com>",
            "Signed-off-by: Dev Eloper <dev@example.com>",
        ),
    );
    assert!(
        titles(&run).is_empty(),
        "{:?}",
        run.violations("commit-provenance")
    );

    // Control: no entry carries it.
    let run = check_one(
        CFG,
        &squash(
            "Signed-off-by: Dev Eloper <dev@example.com>",
            "Signed-off-by: Dev Eloper <dev@example.com>",
            "Signed-off-by: Dev Eloper <dev@example.com>",
        ),
    );
    assert_eq!(titles(&run), ["Commit Trailer Missing"]);
}

#[test]
fn an_ordinary_commit_is_read_as_before() {
    const CFG: &str = "required_trailers = [\"Signed-off-by\"]\n";
    // Control: the last paragraph is the trailer block.
    let run = check_one(
        CFG,
        "feat: a\n\nBody: prose with a colon.\n\nSigned-off-by: Dev Eloper <dev@example.com>\n",
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(titles(&run).is_empty());

    // Control: `Key: value` text that is not at the end of the message, or of a squash
    // entry, is body text. Here it is followed by prose, quoted, and inside a code fence.
    let run = check_one(
        CFG,
        "feat: a\n\nSigned-off-by: Dev Eloper <dev@example.com>\nAgent-Tool: coder 1.2\n\n\
         Prose after it.\n\n> Agent-Tool: coder 1.2\n\n```\nAgent-Tool: coder 1.2\n```\n",
    );
    assert_eq!(titles(&run), ["Commit Trailer Missing"]);

    // Control: a bulleted list in a body is not a squash unless a bullet starts a
    // paragraph, and prose between a `Key: value` paragraph and the bullet keeps it body.
    let run = check_one(
        CFG,
        "feat: a\n\nAgent-Tool: coder 1.2\n\nChanges:\n* one\n* two\n\n\
         Signed-off-by: Dev Eloper <dev@example.com>\n",
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(titles(&run).is_empty());
}

#[test]
fn an_indented_quote_of_trailers_does_not_satisfy_a_required_trailer() {
    const CFG: &str = "required_trailers = [\"Signed-off-by\"]\n";
    let run = check_one(
        CFG,
        "revert: a\n\nThe reverted commit ended with:\n\n    \
         Signed-off-by: Dev Eloper <dev@example.com>\n    Ticket: 12\n",
    );
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(titles(&run), ["Commit Trailer Missing"]);

    // Control: an indented line under a trailer continues it, as in git.
    let run = check_one(
        CFG,
        "feat: a\n\nSigned-off-by: Dev Eloper\n  <dev@example.com>\n",
    );
    assert!(titles(&run).is_empty());
}

#[test]
fn a_cherry_pick_line_does_not_void_the_trailer_block() {
    const CFG: &str = "required_trailers = [\"Signed-off-by\"]\n";
    // What `git cherry-pick -x` writes: its line joins an existing trailer block.
    let run = check_one(
        CFG,
        "fix: a\n\nBody.\n\nSigned-off-by: Dev Eloper <dev@example.com>\n\
         (cherry picked from commit 0123456789abcdef0123456789abcdef01234567)\n",
    );
    assert!(
        titles(&run).is_empty(),
        "{:?}",
        run.violations("commit-provenance")
    );

    // Control: a cherry-picked commit that never had the trailer.
    let run = check_one(
        CFG,
        "fix: a\n\nBody.\n\n(cherry picked from commit 0123456789abcdef0123456789abcdef01234567)\n",
    );
    assert_eq!(titles(&run), ["Commit Trailer Missing"]);
}

#[test]
fn a_crlf_message_has_its_trailers_read() {
    const CFG: &str = "required_trailers = [\"Signed-off-by\"]\n";
    let run = check_one(
        CFG,
        "fix: a\r\n\r\nBody.\r\n\r\nSigned-off-by: Dev Eloper <dev@example.com>\r\nAgent-Tool: coder 1.2\r\n",
    );
    // The required trailer is found and the marker is read too.
    assert_eq!(titles(&run), ["Agent Commit Without Review"]);

    // Control: a CRLF message without the trailer.
    let run = check_one(CFG, "fix: a\r\n\r\nBody.\r\n");
    assert_eq!(titles(&run), ["Commit Trailer Missing"]);
}

#[test]
fn a_gate_with_no_rule_on_says_so() {
    let nothing_checked = |cfg: &str| {
        let repo = repo_with(cfg);
        commit_verbatim(&repo, "a.txt", "feat: a\n\nAgent-Tool: coder 1.2\n");
        let run = repo.check(&[]);
        assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
        let outcome = run.outcome("commit-provenance");
        assert_eq!(outcome["examined"], 0, "{outcome}");
        let notes = outcome["notes"].to_string();
        assert!(notes.contains("not evaluated: no rule is on"), "{notes}");
        notes
    };
    let notes = nothing_checked("require_agent_review = false\n");
    assert!(notes.contains("`require_agent_review` is false"), "{notes}");
    let notes = nothing_checked("agent_markers = []\n");
    assert!(notes.contains("`agent_markers` is empty"), "{notes}");

    // Control: with either rule on the commits are examined and no such note is left.
    for cfg in [
        "",
        "require_agent_review = false\nrequired_trailers = [\"Agent-Tool\"]\n",
    ] {
        let repo = repo_with(cfg);
        commit_verbatim(
            &repo,
            "a.txt",
            "feat: a\n\nAgent-Tool: coder 1.2\nReviewed-by: Rev Iewer <rev@example.com>\n",
        );
        let run = repo.check(&[]);
        let outcome = run.outcome("commit-provenance");
        assert_eq!(outcome["examined"], 1, "{cfg}: {outcome}");
        assert!(
            !outcome["notes"].to_string().contains("no rule is on"),
            "{cfg}: {outcome}"
        );
    }
}
