//! `hook install --upgrade` and a hook file somebody edited (#653). A generated script,
//! workflow or plugin carries a digest of its own content: one that still matches is
//! rewritten as before, any other is left byte for byte, with the difference printed and
//! exit 1, until `--force` is given. A JSON hook file cannot carry a digest: one with
//! content of its own gets this release's entries merged in under the same rule, and
//! everything else in it is kept. Each layout is driven through the real binary.

mod common;
use common::{Repo, Run};
use discipline::hook::{Agent, GENERATED_HEADER};
use discipline::hookfile::{stamp, stamp_state, Stamp, STAMP};

/// One generated file with the header: the arguments that write it and where it is.
struct Layout {
    args: &'static [&'static str],
    file: &'static str,
    /// A line a person adds, in the file's comment syntax.
    comment: &'static str,
}

const LAYOUTS: &[Layout] = &[
    Layout {
        args: &["--agent", "claude-code"],
        file: ".claude/hooks/discipline-bootstrap.sh",
        comment: "# kept on purpose: reviewed by the platform team",
    },
    Layout {
        args: &["--agent", "copilot", "--cloud-agent"],
        file: ".github/workflows/copilot-setup-steps.yml",
        comment: "# kept on purpose: reviewed by the platform team",
    },
    Layout {
        args: &["--agent", "opencode"],
        file: ".opencode/plugins/discipline.js",
        comment: "// kept on purpose: reviewed by the platform team",
    },
    Layout {
        args: &["--agent", "opencode", "--observe"],
        file: ".opencode/plugins/discipline.js",
        comment: "// kept on purpose: reviewed by the platform team",
    },
];

fn install(repo: &Repo, layout: &[&str], extra: &[&str]) -> Run {
    let mut args = vec!["hook", "install"];
    args.extend_from_slice(layout);
    args.extend_from_slice(extra);
    repo.run(&args, &[])
}

fn read(repo: &Repo, file: &str) -> String {
    std::fs::read_to_string(repo.file(file)).unwrap()
}

/// What this release writes for `layout`, from a fresh repository.
fn current(layout: &Layout) -> String {
    let fresh = Repo::new();
    let run = install(&fresh, layout.args, &[]);
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    read(&fresh, layout.file)
}

/// `text` without its digest line.
fn unstamped(text: &str) -> String {
    text.split_inclusive('\n')
        .filter(|l| !l.contains(STAMP))
        .collect()
}

/// What an earlier release wrote where `now` is: another body, and that release's digest.
fn earlier(now: &str) -> String {
    let leader = if now.starts_with("//") { "//" } else { "#" };
    let body = format!(
        "{}{leader} A line an earlier release wrote and this one does not.\n",
        unstamped(now)
    );
    stamp(&body, GENERATED_HEADER, &["mode=enforcing"])
}

#[test]
fn a_generated_file_carries_a_digest_of_its_own_content() {
    for layout in LAYOUTS {
        let now = current(layout);
        assert_eq!(
            now.lines().filter(|l| l.contains(STAMP)).count(),
            1,
            "{}: {now}",
            layout.file
        );
        assert_eq!(stamp_state(&now), Stamp::Unedited, "{}", layout.file);
        // Control: the same file with one line added no longer matches.
        assert_eq!(
            stamp_state(&format!("{now}{}\n", layout.comment)),
            Stamp::Edited,
            "{}",
            layout.file
        );
    }
}

#[test]
fn upgrade_rewrites_an_unedited_file_of_an_earlier_release() {
    for layout in LAYOUTS {
        let now = current(layout);
        let old = earlier(&now);
        assert_eq!(stamp_state(&old), Stamp::Unedited);
        let repo = Repo::new();
        repo.write(layout.file, &old);

        let plain = install(&repo, layout.args, &[]);
        assert_eq!(plain.code, 0, "{}", plain.stdout);
        assert!(
            plain
                .stdout
                .contains("was written by an earlier discipline release"),
            "{}: {}",
            layout.file,
            plain.stdout
        );
        assert_eq!(read(&repo, layout.file), old);

        let up = install(&repo, layout.args, &["--upgrade"]);
        assert_eq!(up.code, 0, "{}: {}", layout.file, up.stdout);
        assert!(up.stdout.contains("upgraded"), "{}", up.stdout);
        assert!(!up.stdout.contains("@@"), "no diff: {}", up.stdout);
        assert_eq!(read(&repo, layout.file), now, "{}", layout.file);
    }
}

#[test]
fn upgrade_refuses_an_edited_file_and_force_overwrites_it() {
    for layout in LAYOUTS {
        let now = current(layout);
        // Edited after an earlier release wrote it, and edited after this one did.
        for base in [earlier(&now), now.clone()] {
            let edited = format!("{base}{}\n", layout.comment);
            let repo = Repo::new();
            repo.write(layout.file, &edited);

            let plain = install(&repo, layout.args, &[]);
            assert_eq!(plain.code, 0, "{}", plain.stdout);
            assert!(
                plain.stdout.contains("was changed after") && plain.stdout.contains("--upgrade"),
                "{}: {}",
                layout.file,
                plain.stdout
            );
            assert_eq!(read(&repo, layout.file), edited);

            let refused = install(&repo, layout.args, &["--upgrade"]);
            assert_eq!(refused.code, 1, "{}: {}", layout.file, refused.stdout);
            assert!(
                refused.stdout.contains(layout.file)
                    && refused.stdout.contains("was changed after")
                    && refused.stdout.contains("--force")
                    && refused.stdout.contains(&format!("-{}", layout.comment)),
                "{}: {}",
                layout.file,
                refused.stdout
            );
            assert_eq!(
                read(&repo, layout.file),
                edited,
                "{}: left byte for byte",
                layout.file
            );

            let forced = install(&repo, layout.args, &["--upgrade", "--force"]);
            assert_eq!(forced.code, 0, "{}: {}", layout.file, forced.stdout);
            assert!(
                forced.stdout.contains("overwrote")
                    && forced.stdout.contains(&format!("-{}", layout.comment)),
                "{}: {}",
                layout.file,
                forced.stdout
            );
            assert_eq!(read(&repo, layout.file), now, "{}", layout.file);
        }
    }
}

/// The edits of the report: a digest pinned by hand with a comment saying why, and a
/// guard added before the download.
#[test]
fn upgrade_keeps_a_bootstrap_pinned_by_hand() {
    let layout = &LAYOUTS[0];
    let now = current(layout);
    let edited = now
        .replace(
            "set -u\n",
            "set -u\n# The digests below were checked against the release's provenance.\n[ -n \"${DISCIPLINE_SKIP_BOOTSTRAP:-}\" ] && exit 0\n",
        )
        .replace("want=\"$(awk", "want=\"0123abcd\" # $(awk");
    assert!(edited.contains("0123abcd") && edited.contains("DISCIPLINE_SKIP_BOOTSTRAP"));
    let repo = Repo::new();
    repo.write(layout.file, &edited);
    let refused = install(&repo, layout.args, &["--upgrade"]);
    assert_eq!(refused.code, 1, "{}", refused.stdout);
    assert!(
        refused
            .stdout
            .contains("-# The digests below were checked against the release's provenance.")
            && refused.stdout.contains("-want=\"0123abcd\""),
        "{}",
        refused.stdout
    );
    assert_eq!(read(&repo, layout.file), edited);
}

#[test]
fn upgrade_cannot_tell_a_file_without_a_digest_and_refuses_it() {
    for layout in LAYOUTS {
        let now = current(layout);
        // What a release before the digest wrote: the header, no digest line.
        let old = format!("{}{}\n", unstamped(&now), layout.comment);
        assert_eq!(stamp_state(&old), Stamp::Absent);
        let repo = Repo::new();
        repo.write(layout.file, &old);

        let refused = install(&repo, layout.args, &["--upgrade"]);
        assert_eq!(refused.code, 1, "{}: {}", layout.file, refused.stdout);
        assert!(
            refused.stdout.contains("carries no digest")
                && refused.stdout.contains("--force")
                && refused.stdout.contains("@@ "),
            "{}: {}",
            layout.file,
            refused.stdout
        );
        assert_eq!(read(&repo, layout.file), old, "{}", layout.file);

        let forced = install(&repo, layout.args, &["--upgrade", "--force"]);
        assert_eq!(forced.code, 0, "{}: {}", layout.file, forced.stdout);
        assert_eq!(read(&repo, layout.file), now, "{}", layout.file);
    }
}

/// A file without a digest line that is otherwise what this release writes is provably
/// unedited: the upgrade only adds the digest, so nothing is asked. One more line in it and
/// it is the "cannot tell" file of the test above.
#[test]
fn a_file_without_a_digest_that_is_otherwise_current_upgrades_silently() {
    for layout in LAYOUTS {
        let now = current(layout);
        let old = unstamped(&now);
        assert_eq!(stamp_state(&old), Stamp::Absent);
        let repo = Repo::new();
        repo.write(layout.file, &old);

        let plain = install(&repo, layout.args, &[]);
        assert_eq!(plain.code, 0, "{}", plain.stdout);
        assert!(
            plain
                .stdout
                .contains("was written by an earlier discipline release"),
            "{}: {}",
            layout.file,
            plain.stdout
        );
        assert_eq!(read(&repo, layout.file), old);

        let up = install(&repo, layout.args, &["--upgrade"]);
        assert_eq!(up.code, 0, "{}: {}", layout.file, up.stdout);
        assert!(
            up.stdout.contains("upgraded") && !up.stdout.contains("@@"),
            "{}: {}",
            layout.file,
            up.stdout
        );
        assert_eq!(read(&repo, layout.file), now, "{}", layout.file);

        // Control: the same file with one line of its own is not provable.
        let edited = format!("{old}{}\n", layout.comment);
        repo.write(layout.file, &edited);
        let refused = install(&repo, layout.args, &["--upgrade"]);
        assert_eq!(refused.code, 1, "{}: {}", layout.file, refused.stdout);
        assert_eq!(read(&repo, layout.file), edited, "{}", layout.file);
    }
    // An observe-mode plugin without a digest keeps its mode when none is asked for.
    let observe = current(&LAYOUTS[3]);
    let repo = Repo::new();
    repo.write(LAYOUTS[3].file, &unstamped(&observe));
    let up = install(&repo, &["--agent", "opencode"], &["--upgrade"]);
    assert_eq!(up.code, 0, "{}", up.stdout);
    assert_eq!(read(&repo, LAYOUTS[3].file), observe);
}

/// Observe and enforcing are two things `hook install` writes: moving a current plugin to
/// observe mode is not an edit, and an earlier release's observe-mode plugin stays in
/// observe mode.
#[test]
fn switching_mode_is_not_a_local_edit() {
    let file = ".opencode/plugins/discipline.js";
    let enforcing = current(&LAYOUTS[2]);
    let observe = current(&LAYOUTS[3]);
    assert_ne!(enforcing, observe);
    let repo = Repo::new();
    repo.write(file, &enforcing);
    let up = install(&repo, &["--agent", "opencode", "--observe"], &["--upgrade"]);
    assert_eq!(up.code, 0, "{}", up.stdout);
    assert_eq!(read(&repo, file), observe);

    let old = Repo::new();
    old.write(file, &earlier(&observe));
    let up = install(&old, &["--agent", "opencode"], &["--upgrade"]);
    assert_eq!(up.code, 0, "{}", up.stdout);
    assert!(up.stdout.contains("in observe mode"), "{}", up.stdout);
    assert_eq!(read(&old, file), observe);
}

/// A pinned bootstrap is never replaced by an unpinned one, `--force` included; with the
/// new release's sums, an edited one follows the same rule as any other file.
#[test]
fn force_does_not_unpin_a_bootstrap() {
    let layout = &LAYOUTS[0];
    let sums_dir = tempfile::tempdir().unwrap();
    let sums = sums_dir.path().join("SHA256SUMS");
    std::fs::write(
        &sums,
        format!(
            "{}  discipline-x86_64-unknown-linux-musl.tar.gz\n{}  discipline-aarch64-unknown-linux-musl.tar.gz\n",
            "a".repeat(64),
            "b".repeat(64)
        ),
    )
    .unwrap();
    let pin = ["--pin-sums", sums.to_str().unwrap()];
    let repo = Repo::new();
    let run = install(&repo, layout.args, &pin);
    assert_eq!(run.code, 0, "{}", run.stdout);
    let pinned = read(&repo, layout.file);
    assert_eq!(stamp_state(&pinned), Stamp::Unedited);
    let edited = format!("{pinned}{}\n", layout.comment);
    repo.write(layout.file, &edited);

    let kept = install(&repo, layout.args, &["--upgrade", "--force"]);
    assert_eq!(kept.code, 0, "{}", kept.stdout);
    assert!(kept.stdout.contains("--pin-sums"), "{}", kept.stdout);
    assert_eq!(read(&repo, layout.file), edited);

    let refused = install(&repo, layout.args, &[pin[0], pin[1], "--upgrade"]);
    assert_eq!(refused.code, 1, "{}", refused.stdout);
    assert!(
        refused.stdout.contains(&format!("-{}", layout.comment)),
        "{}",
        refused.stdout
    );
    assert_eq!(read(&repo, layout.file), edited);
    let forced = install(
        &repo,
        layout.args,
        &[pin[0], pin[1], "--upgrade", "--force"],
    );
    assert_eq!(forced.code, 0, "{}", forced.stdout);
    assert_eq!(read(&repo, layout.file), pinned);
}

#[test]
fn force_is_only_for_an_upgrade() {
    let repo = Repo::new();
    let run = install(&repo, &["--agent", "opencode"], &["--force"]);
    assert_eq!(run.code, 2, "{}", run.stdout);
    assert!(run.stderr.contains("--upgrade"), "{}", run.stderr);
    assert!(!repo.file(".opencode/plugins/discipline.js").exists());
}

/// The agents whose hook file is JSON, with the table its events are under and one event
/// this release writes a check into.
const JSON_AGENTS: &[(Agent, &str, &str)] = &[
    (Agent::ClaudeCode, "hooks", "Stop"),
    (Agent::Codex, "hooks", "Stop"),
    (Agent::Cursor, "hooks", "stop"),
    (Agent::Copilot, "hooks", "agentStop"),
    (Agent::Agy, "discipline", "Stop"),
    (Agent::Qwen, "hooks", "Stop"),
];

/// The hook file of `agent` as this release writes it, with a setting and a hook that are
/// the repository's own beside it.
fn with_own_content(agent: Agent, table: &str, event: &str) -> (&'static str, serde_json::Value) {
    let (file, generated) = discipline::hook::config_for_mode(agent, false);
    let mut json: serde_json::Value = serde_json::from_str(&generated).unwrap();
    json["ourTeamSetting"] = serde_json::json!({ "model": "local", "keep": [1, 2, 3] });
    json[table]["OurOwnEvent"] =
        serde_json::json!([{ "type": "command", "command": "./scripts/notify.sh" }]);
    json[table][event]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({ "type": "command", "command": "./scripts/after-stop.sh" }));
    (file, json)
}

fn pretty(json: &serde_json::Value) -> String {
    serde_json::to_string_pretty(json).unwrap() + "\n"
}

/// Every string in `json`, in place, through `edit`.
fn edit_strings(json: &mut serde_json::Value, edit: &dyn Fn(&str) -> Option<String>) {
    match json {
        serde_json::Value::String(s) => {
            if let Some(new) = edit(s) {
                *s = new;
            }
        }
        serde_json::Value::Array(a) => a.iter_mut().for_each(|v| edit_strings(v, edit)),
        serde_json::Value::Object(m) => m.values_mut().for_each(|v| edit_strings(v, edit)),
        _ => {}
    }
}

#[test]
fn a_json_file_with_content_of_its_own_and_current_entries_is_left_alone() {
    for (agent, table, event) in JSON_AGENTS {
        let (file, json) = with_own_content(*agent, table, event);
        let text = pretty(&json);
        let repo = Repo::new();
        repo.write(file, &text);
        let up = install(&repo, &["--agent", agent.id()], &["--upgrade"]);
        assert_eq!(up.code, 0, "{file}: {}", up.stdout);
        assert!(
            up.stdout.contains("already runs discipline"),
            "{}",
            up.stdout
        );
        assert_eq!(read(&repo, file), text, "{file}");
    }
}

#[test]
fn an_edited_entry_in_a_json_file_is_refused_and_force_keeps_what_is_not_ours() {
    for (agent, table, event) in JSON_AGENTS {
        let (file, current_json) = with_own_content(*agent, table, event);
        // A guard somebody put in front of the check command.
        let mut json = current_json.clone();
        let check = format!("discipline hook run --agent {}", agent.id());
        edit_strings(&mut json, &|s| {
            s.ends_with(&check)
                .then(|| s.replace(&check, &format!("test -f .skip-discipline || {check}")))
        });
        assert_ne!(json, current_json, "{file}");
        let edited = pretty(&json);
        let repo = Repo::new();
        repo.write(file, &edited);

        let refused = install(&repo, &["--agent", agent.id()], &["--upgrade"]);
        assert_eq!(refused.code, 1, "{file}: {}", refused.stdout);
        assert!(
            refused.stdout.contains(file)
                && refused.stdout.contains("--force")
                && refused
                    .stdout
                    .lines()
                    .any(|l| l.starts_with('-') && l.contains("test -f .skip-discipline ||")),
            "{file}: {}",
            refused.stdout
        );
        assert_eq!(read(&repo, file), edited, "{file}: left byte for byte");

        let forced = install(&repo, &["--agent", agent.id()], &["--upgrade", "--force"]);
        assert_eq!(forced.code, 0, "{file}: {}", forced.stdout);
        assert!(
            forced.stdout.contains("test -f .skip-discipline ||"),
            "the discarded edit is shown: {}",
            forced.stdout
        );
        let now: serde_json::Value = serde_json::from_str(&read(&repo, file)).unwrap();
        assert_eq!(now, current_json, "{file}: only our entries changed");

        // Control: nothing left to do.
        let again = install(&repo, &["--agent", agent.id()], &["--upgrade"]);
        assert_eq!(again.code, 0, "{file}: {}", again.stdout);
    }
}

/// A file that holds only what a release writes is rewritten without a question, as
/// before; one whose check timeout is below the default is what an edit looks like.
#[test]
fn a_json_file_only_a_release_wrote_upgrades_and_a_shortened_timeout_does_not() {
    for (agent, table, event) in JSON_AGENTS {
        let (file, generated) = discipline::hook::config_for_mode(*agent, false);
        let mut old: serde_json::Value = serde_json::from_str(&generated).unwrap();
        // An earlier release: no pre-tool entry yet (Cursor's file has only its check).
        let events = old[*table].as_object_mut().unwrap();
        events.retain(|name, _| !name.eq_ignore_ascii_case("PreToolUse"));
        let repo = Repo::new();
        repo.write(file, &pretty(&old));
        let up = install(&repo, &["--agent", agent.id()], &["--upgrade"]);
        assert_eq!(up.code, 0, "{file}: {}", up.stdout);
        assert!(!up.stdout.contains("@@"), "no diff: {}", up.stdout);
        assert_eq!(read(&repo, file), generated, "{file}");

        let Some(default) = discipline::hook::default_timeout(*agent) else {
            continue;
        };
        let mut short: serde_json::Value = serde_json::from_str(&generated).unwrap();
        fn shorten(json: &mut serde_json::Value, default: u32) {
            match json {
                serde_json::Value::Number(n) if n.as_u64() == Some(u64::from(default)) => {
                    *json = serde_json::json!(7);
                }
                serde_json::Value::Array(a) => a.iter_mut().for_each(|v| shorten(v, default)),
                serde_json::Value::Object(m) => m.values_mut().for_each(|v| shorten(v, default)),
                _ => {}
            }
        }
        shorten(&mut short, default);
        let short = pretty(&short);
        assert_ne!(short, generated, "{file}: {event}");
        repo.write(file, &short);
        let refused = install(&repo, &["--agent", agent.id()], &["--upgrade"]);
        assert_eq!(refused.code, 1, "{file}: {}", refused.stdout);
        assert!(
            refused.stdout.contains("--force")
                && refused.stdout.contains("--timeout")
                && refused
                    .stdout
                    .lines()
                    .any(|l| l.starts_with('-') && l.contains("\": 7")),
            "{file}: {}",
            refused.stdout
        );
        assert_eq!(read(&repo, file), short, "{file}");
        // Saying the timeout is what makes it an option rather than an edit.
        let kept = install(
            &repo,
            &["--agent", agent.id()],
            &["--upgrade", "--timeout", "7"],
        );
        assert_eq!(kept.code, 0, "{file}: {}", kept.stdout);
        assert_eq!(read(&repo, file), short, "{file}");
        let forced = install(&repo, &["--agent", agent.id()], &["--upgrade", "--force"]);
        assert_eq!(forced.code, 0, "{file}: {}", forced.stdout);
        assert_eq!(read(&repo, file), generated, "{file}");
    }
}

#[test]
fn the_user_level_file_follows_the_same_rule() {
    let repo = Repo::new();
    let home = tempfile::tempdir().unwrap();
    let env = [("COPILOT_HOME", home.path().to_str().unwrap())];
    let args = ["hook", "install", "--agent", "copilot", "--user"];
    let run = repo.run(&args, &env);
    assert_eq!(run.code, 0, "{}", run.stdout);
    let path = home.path().join("hooks/discipline.json");
    let generated: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let mut json = generated.clone();
    json["hooks"]["agentStop"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({ "type": "command", "bash": "./mine.sh", "timeoutSec": 5 }));
    let own = json.clone();
    edit_strings(&mut json, &|s| {
        s.ends_with("--if-configured")
            .then(|| format!("{s} --base origin/release"))
    });
    assert_ne!(json, own);
    let edited = pretty(&json);
    std::fs::write(&path, &edited).unwrap();

    let refused = repo.run(&[&args[..], &["--upgrade"]].concat(), &env);
    assert_eq!(refused.code, 1, "{}", refused.stdout);
    assert!(
        refused.stdout.contains("--base origin/release") && refused.stdout.contains("--force"),
        "{}",
        refused.stdout
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);
    let forced = repo.run(&[&args[..], &["--upgrade", "--force"]].concat(), &env);
    assert_eq!(forced.code, 0, "{}", forced.stdout);
    let now: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(now, own, "the hook that is not ours is kept");
}

/// A hook file is repository content: a control sequence in it does not reach the
/// terminal through the printed difference.
#[test]
fn the_printed_difference_carries_no_control_sequence() {
    let layout = &LAYOUTS[2];
    let now = current(layout);
    let edited = format!("{now}// \u{1b}[2J\u{1b}[1;31mok: upgraded\u{1b}[0m\n");
    let repo = Repo::new();
    repo.write(layout.file, &edited);
    let refused = install(&repo, layout.args, &["--upgrade"]);
    assert_eq!(refused.code, 1, "{}", refused.stdout);
    assert!(
        refused.stdout.contains("ok: upgraded"),
        "{}",
        refused.stdout
    );
    assert!(!refused.stdout.contains('\u{1b}'), "{:?}", refused.stdout);
}
