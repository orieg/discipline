//! Issue 575: `hook install --upgrade` keeps the mode (observe or enforcing) of every
//! generated hook file, the OpenCode plugin included, and says which mode it wrote.
//! Every run is in a throwaway repository with the home directories pointed at a
//! throwaway directory.

mod common;

use common::{Repo, Run};
use discipline::hook::{config_for_mode, Agent, GENERATED_HEADER, OPENCODE_OBSERVE_NEVER_BLOCKS};
use discipline::hookfile::{stamp, unstamped};
use std::path::Path;

const PLUGIN: &str = ".opencode/plugins/discipline.js";

/// A repository and a home directory, both temporary.
fn sandbox() -> (Repo, tempfile::TempDir) {
    (Repo::new(), tempfile::tempdir().unwrap())
}

/// `discipline hook install <args>` in `repo`, with every home directory under `home`.
fn install(repo: &Repo, home: &Path, args: &[&str]) -> Run {
    let h = home.to_str().unwrap();
    let copilot = home.join(".copilot");
    let mut all = vec!["hook", "install"];
    all.extend_from_slice(args);
    repo.run(
        &all,
        &[
            ("HOME", h),
            ("USERPROFILE", h),
            ("XDG_CONFIG_HOME", h),
            ("COPILOT_HOME", copilot.to_str().unwrap()),
        ],
    )
}

fn read(repo: &Repo, rel: &str) -> String {
    std::fs::read_to_string(repo.file(rel)).unwrap()
}

/// `text`, a generated file whose body was altered, with the digest an earlier release
/// would have written for that body: without it the alteration is an edit.
fn restamped(text: &str) -> String {
    stamp(&unstamped(text), GENERATED_HEADER, &[])
}

/// This release's plugin with one comment line of the template worded differently, and
/// the digest of the plugin as it was before: what a hand edit leaves.
fn altered_plugin(observe: bool) -> String {
    let now = config_for_mode(Agent::Opencode, observe).1;
    let old = now.replace(
        "// the model reads it and repairs the change.",
        "// the model reads it.",
    );
    assert_ne!(old, now, "the template still has the line this replaces");
    old
}

/// `--upgrade` on `text`, the plugin with an alteration its digest does not cover: refused
/// with the difference, and the file is left byte for byte.
fn assert_upgrade_refuses_plugin(text: &str, extra: &[&str]) {
    let (repo, home) = sandbox();
    repo.write(PLUGIN, text);
    let mut args = vec!["--agent", "opencode", "--upgrade"];
    args.extend_from_slice(extra);
    let refused = install(&repo, home.path(), &args);
    assert_eq!(refused.code, 1, "{}{}", refused.stdout, refused.stderr);
    assert!(
        refused.stdout.contains("@@ ") && refused.stdout.contains("--force"),
        "{}",
        refused.stdout
    );
    assert_eq!(read(&repo, PLUGIN), text);
}

/// What this release writes for `agent`, as an earlier release would have left it: the
/// same file with one comment line of the template worded differently and that body's
/// digest (the plugin), or the v0.15.0 fixture (a JSON hook file).
fn earlier_release(agent: Agent, observe: bool) -> String {
    if agent == Agent::Opencode {
        return restamped(&altered_plugin(observe));
    }
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/hook_install/v0.15.0/{}.{}.json",
        env!("CARGO_MANIFEST_DIR"),
        agent.id(),
        if observe { "observe" } else { "enforce" }
    ))
    .unwrap()
}

/// Whether the plugin text is an observe-mode one: the marker line, `--observe` on the
/// check and pre-tool commands, and no statement that refuses a call.
fn plugin_is_observe(text: &str) -> bool {
    text.lines()
        .filter(|l| *l == OPENCODE_OBSERVE_NEVER_BLOCKS)
        .count()
        == 1
        && text.contains("discipline hook run --agent opencode --observe")
        && text.contains("--event pre-tool --observe")
        && !text.contains("throw new Error")
}

/// Whether the plugin text is an enforcing one: no marker, no `--observe`, and the
/// pre-tool handlers refuse.
fn plugin_is_enforcing(text: &str) -> bool {
    !text.contains(OPENCODE_OBSERVE_NEVER_BLOCKS)
        && !text.contains("--observe")
        && text.matches("throw new Error").count() == 2
}

/// The agents whose hook file is JSON, with the file.
const JSON_AGENTS: [(Agent, &str); 6] = [
    (Agent::ClaudeCode, ".claude/settings.json"),
    (Agent::Codex, ".codex/hooks.json"),
    (Agent::Cursor, ".cursor/hooks.json"),
    (Agent::Copilot, ".github/hooks/discipline.json"),
    (Agent::Agy, ".agents/hooks.json"),
    (Agent::Qwen, ".qwen/settings.json"),
];

/// The issue: an observe-mode plugin of this release, upgraded without `--observe`, is
/// still the observe-mode plugin.
#[test]
fn an_observe_plugin_of_this_release_is_left_in_observe_mode_by_upgrade() {
    let (repo, home) = sandbox();
    let first = install(&repo, home.path(), &["--agent", "opencode", "--observe"]);
    assert_eq!(first.code, 0, "{}{}", first.stdout, first.stderr);
    let before = read(&repo, PLUGIN);
    assert!(plugin_is_observe(&before), "{before}");

    let up = install(&repo, home.path(), &["--agent", "opencode", "--upgrade"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let after = read(&repo, PLUGIN);
    assert!(
        plugin_is_observe(&after),
        "the upgrade left observe mode:\n{}\n{after}",
        up.stdout
    );
    assert_eq!(after, before, "nothing to rewrite: {}", up.stdout);
    assert!(
        up.stdout
            .contains("already runs discipline for opencode, in observe mode"),
        "{}",
        up.stdout
    );
}

/// An observe-mode plugin an earlier release wrote is rewritten to this release's
/// observe-mode plugin.
#[test]
fn an_earlier_observe_plugin_is_upgraded_to_this_releases_observe_plugin() {
    let (repo, home) = sandbox();
    repo.write(PLUGIN, &earlier_release(Agent::Opencode, true));
    let up = install(&repo, home.path(), &["--agent", "opencode", "--upgrade"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let after = read(&repo, PLUGIN);
    assert!(plugin_is_observe(&after), "{}\n{after}", up.stdout);
    assert_eq!(after, config_for_mode(Agent::Opencode, true).1);

    // The same alteration under the digest of the unaltered plugin is an edit.
    assert_upgrade_refuses_plugin(&altered_plugin(true), &[]);
}

/// Control: an enforcing plugin stays enforcing through an upgrade, from this release
/// and from an earlier one.
#[test]
fn an_enforcing_plugin_stays_enforcing_through_upgrade() {
    let (repo, home) = sandbox();
    let first = install(&repo, home.path(), &["--agent", "opencode"]);
    assert_eq!(first.code, 0, "{}{}", first.stdout, first.stderr);
    let up = install(&repo, home.path(), &["--agent", "opencode", "--upgrade"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert!(plugin_is_enforcing(&read(&repo, PLUGIN)), "{}", up.stdout);

    let (repo, home) = sandbox();
    repo.write(PLUGIN, &earlier_release(Agent::Opencode, false));
    let up = install(&repo, home.path(), &["--agent", "opencode", "--upgrade"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert_eq!(
        read(&repo, PLUGIN),
        config_for_mode(Agent::Opencode, false).1
    );

    // The same alteration under the digest of the unaltered plugin is an edit.
    assert_upgrade_refuses_plugin(&altered_plugin(false), &[]);
}

/// Control: `--upgrade --observe` turns observe mode on, as it does for a JSON file.
#[test]
fn upgrade_with_observe_turns_an_enforcing_plugin_to_observe() {
    let (repo, home) = sandbox();
    let first = install(&repo, home.path(), &["--agent", "opencode"]);
    assert_eq!(first.code, 0, "{}{}", first.stdout, first.stderr);
    let up = install(
        &repo,
        home.path(),
        &["--agent", "opencode", "--upgrade", "--observe"],
    );
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert!(up.stdout.contains("upgraded"), "{}", up.stdout);
    assert_eq!(
        read(&repo, PLUGIN),
        config_for_mode(Agent::Opencode, true).1
    );
}

/// A plain install on a plugin this release wrote in the other mode says which mode the
/// file is in and what to run; it does not call the file an earlier release's.
#[test]
fn a_plain_install_on_a_plugin_in_the_other_mode_names_both_modes() {
    let (repo, home) = sandbox();
    install(&repo, home.path(), &["--agent", "opencode", "--observe"]);
    let before = read(&repo, PLUGIN);
    let plain = install(&repo, home.path(), &["--agent", "opencode"]);
    assert_eq!(plain.code, 0, "{}{}", plain.stdout, plain.stderr);
    assert!(
        !plain.stdout.contains("earlier discipline release"),
        "{}",
        plain.stdout
    );
    assert!(
        plain
            .stdout
            .contains("is in observe mode and this command asked for enforcing mode")
            && plain.stdout.contains("it was not changed")
            && plain.stdout.contains("delete"),
        "{}",
        plain.stdout
    );
    assert_eq!(read(&repo, PLUGIN), before);

    let (repo, home) = sandbox();
    install(&repo, home.path(), &["--agent", "opencode"]);
    let before = read(&repo, PLUGIN);
    let plain = install(&repo, home.path(), &["--agent", "opencode", "--observe"]);
    assert_eq!(plain.code, 0, "{}{}", plain.stdout, plain.stderr);
    assert!(
        !plain.stdout.contains("earlier discipline release"),
        "{}",
        plain.stdout
    );
    assert!(
        plain
            .stdout
            .contains("is in enforcing mode and this command asked for observe mode")
            && plain.stdout.contains("it was not changed")
            && plain.stdout.contains("`--upgrade`"),
        "{}",
        plain.stdout
    );
    assert_eq!(read(&repo, PLUGIN), before);
}

/// The same for a JSON hook file.
#[test]
fn a_plain_install_on_a_json_file_in_the_other_mode_names_both_modes() {
    let qwen = ".qwen/settings.json";
    let (repo, home) = sandbox();
    install(&repo, home.path(), &["--agent", "qwen", "--observe"]);
    let before = read(&repo, qwen);
    let plain = install(&repo, home.path(), &["--agent", "qwen"]);
    assert_eq!(plain.code, 0, "{}{}", plain.stdout, plain.stderr);
    assert!(
        plain
            .stdout
            .contains("is in observe mode and this command asked for enforcing mode"),
        "{}",
        plain.stdout
    );
    assert_eq!(read(&repo, qwen), before);

    let (repo, home) = sandbox();
    install(&repo, home.path(), &["--agent", "qwen"]);
    let before = read(&repo, qwen);
    let plain = install(&repo, home.path(), &["--agent", "qwen", "--observe"]);
    assert_eq!(plain.code, 0, "{}{}", plain.stdout, plain.stderr);
    assert!(
        !plain.stdout.contains("earlier discipline release")
            && plain
                .stdout
                .contains("is in enforcing mode and this command asked for observe mode"),
        "{}",
        plain.stdout
    );
    assert_eq!(read(&repo, qwen), before);
}

/// Control: a file that really is an earlier release's is still called that.
#[test]
fn a_plain_install_on_an_earlier_releases_file_still_says_so() {
    for (agent, rel) in [
        (Agent::Opencode, PLUGIN),
        (Agent::Qwen, ".qwen/settings.json"),
    ] {
        let (repo, home) = sandbox();
        let old = earlier_release(agent, true);
        repo.write(rel, &old);
        let plain = install(&repo, home.path(), &["--agent", agent.id(), "--observe"]);
        assert_eq!(plain.code, 0, "{}{}", plain.stdout, plain.stderr);
        assert!(
            plain
                .stdout
                .contains("was written by an earlier discipline release"),
            "{agent:?}: {}",
            plain.stdout
        );
        assert_eq!(read(&repo, rel), old, "{agent:?}");
    }

    // The plugin with the same alteration under the digest of the unaltered one is not
    // called an earlier release's: it was changed after it was written.
    let (repo, home) = sandbox();
    let edited = altered_plugin(true);
    repo.write(PLUGIN, &edited);
    let plain = install(&repo, home.path(), &["--agent", "opencode", "--observe"]);
    assert_eq!(plain.code, 0, "{}{}", plain.stdout, plain.stderr);
    assert!(
        plain.stdout.contains("was changed after")
            && !plain
                .stdout
                .contains("was written by an earlier discipline release"),
        "{}",
        plain.stdout
    );
    assert_eq!(read(&repo, PLUGIN), edited);
    assert_upgrade_refuses_plugin(&edited, &["--observe"]);
}

/// A plugin with the generated header whose marker line and commands disagree has no
/// mode that can be read: `--upgrade` refuses it and leaves it byte for byte, whichever
/// way it was damaged. It is never taken for enforcing, and never for observe.
#[test]
fn a_plugin_whose_mode_cannot_be_read_is_refused_by_upgrade_and_left_unchanged() {
    let observe = config_for_mode(Agent::Opencode, true).1;
    let enforcing = config_for_mode(Agent::Opencode, false).1;
    let marker = format!("{OPENCODE_OBSERVE_NEVER_BLOCKS}\n");
    let cases = [
        ("marker removed", observe.replace(&marker, "")),
        (
            "marker garbled",
            observe.replace(
                "// Observe mode: no hook below",
                "// Observe mode: hooks below",
            ),
        ),
        (
            "marker added to an enforcing plugin",
            enforcing.replace("import {", &format!("{marker}import {{")),
        ),
        (
            "one command without --observe",
            observe.replacen("--event pre-tool --observe", "--event pre-tool", 1),
        ),
    ];
    for (what, damaged) in cases {
        assert!(damaged != observe && damaged != enforcing, "{what}");
        let (repo, home) = sandbox();
        repo.write(PLUGIN, &damaged);
        let up = install(&repo, home.path(), &["--agent", "opencode", "--upgrade"]);
        assert_ne!(up.code, 0, "{what}: {}", up.stdout);
        assert!(
            up.stdout.contains("discipline.js") && up.stdout.contains("mode cannot be read"),
            "{what}: {}",
            up.stdout
        );
        assert!(!up.stdout.contains("upgraded"), "{what}: {}", up.stdout);
        assert_eq!(read(&repo, PLUGIN), damaged, "{what}: rewritten");
    }
}

/// What the refusal says to run works: with `--observe` given, the mode is asked for and
/// need not be read. A plugin whose marker line was removed was changed after it was
/// written, so the rewrite also takes `--force`; without it the difference is printed and
/// the file is left as it is.
#[test]
fn a_plugin_whose_mode_cannot_be_read_is_rewritten_with_observe_and_force() {
    let observe = config_for_mode(Agent::Opencode, true).1;
    let damaged = observe.replace(&format!("{OPENCODE_OBSERVE_NEVER_BLOCKS}\n"), "");
    assert_upgrade_refuses_plugin(&damaged, &["--observe"]);

    let (repo, home) = sandbox();
    repo.write(PLUGIN, &damaged);
    let up = install(
        &repo,
        home.path(),
        &["--agent", "opencode", "--upgrade", "--observe", "--force"],
    );
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert_eq!(read(&repo, PLUGIN), observe);
}

/// Pin: a plugin file without the generated header is a person's and is never rewritten.
/// One that does not run discipline is refused with the plugin to merge; one that does is
/// reported as present.
#[test]
fn a_hand_written_plugin_without_the_header_is_never_rewritten() {
    let own = "// our own plugin\nexport const Ours = async () => ({})\n";
    let (repo, home) = sandbox();
    repo.write(PLUGIN, own);
    for args in [
        &["--agent", "opencode"][..],
        &["--agent", "opencode", "--upgrade"][..],
        &["--agent", "opencode", "--upgrade", "--observe"][..],
    ] {
        let run = install(&repo, home.path(), args);
        assert_eq!(run.code, 1, "{args:?}: {}", run.stdout);
        assert!(
            run.stdout.contains("was not changed. Merge this into it"),
            "{args:?}: {}",
            run.stdout
        );
        assert_eq!(read(&repo, PLUGIN), own, "{args:?}");
    }

    let runs = "// our own plugin\n// calls discipline hook run --agent opencode --observe itself\nexport const Ours = async () => ({})\n";
    let (repo, home) = sandbox();
    repo.write(PLUGIN, runs);
    for args in [
        &["--agent", "opencode"][..],
        &["--agent", "opencode", "--upgrade"][..],
    ] {
        let run = install(&repo, home.path(), args);
        assert_eq!(run.code, 0, "{args:?}: {}", run.stdout);
        assert!(
            run.stdout.contains("already runs discipline for opencode"),
            "{args:?}: {}",
            run.stdout
        );
        assert!(!run.stdout.contains(" mode"), "{args:?}: {}", run.stdout);
        assert_eq!(read(&repo, PLUGIN), runs, "{args:?}");
    }
}

/// Control: the JSON hook files of this release keep their mode through `--upgrade`, and
/// `--upgrade --observe` turns observe mode on.
#[test]
fn json_hook_files_of_this_release_keep_their_mode_through_upgrade() {
    for (agent, rel) in JSON_AGENTS {
        for observe in [true, false] {
            let (repo, home) = sandbox();
            let mut args = vec!["--agent", agent.id()];
            if observe {
                args.push("--observe");
            }
            let first = install(&repo, home.path(), &args);
            assert_eq!(first.code, 0, "{agent:?}: {}{}", first.stdout, first.stderr);
            let before = read(&repo, rel);
            assert_eq!(before, config_for_mode(agent, observe).1, "{agent:?}");

            let up = install(&repo, home.path(), &["--agent", agent.id(), "--upgrade"]);
            assert_eq!(up.code, 0, "{agent:?}: {}{}", up.stdout, up.stderr);
            assert_eq!(read(&repo, rel), before, "{agent:?} observe={observe}");

            let on = install(
                &repo,
                home.path(),
                &["--agent", agent.id(), "--upgrade", "--observe"],
            );
            assert_eq!(on.code, 0, "{agent:?}: {}{}", on.stdout, on.stderr);
            assert_eq!(
                read(&repo, rel),
                config_for_mode(agent, true).1,
                "{agent:?}"
            );
        }
    }
}

/// Control: the JSON hook files v0.15.0 wrote are rewritten in their own mode.
#[test]
fn json_hook_files_of_an_earlier_release_are_upgraded_in_their_own_mode() {
    for (agent, rel) in JSON_AGENTS {
        for observe in [true, false] {
            let (repo, home) = sandbox();
            repo.write(rel, &earlier_release(agent, observe));
            let up = install(&repo, home.path(), &["--agent", agent.id(), "--upgrade"]);
            assert_eq!(up.code, 0, "{agent:?}: {}{}", up.stdout, up.stderr);
            assert_eq!(
                read(&repo, rel),
                config_for_mode(agent, observe).1,
                "{agent:?} observe={observe}"
            );
        }
    }
}

/// Observe mode survives `--upgrade` without `--observe` for every agent the installer
/// supports, whatever kind of file it writes.
#[test]
fn observe_mode_survives_upgrade_for_every_agent() {
    let agents = <Agent as clap::ValueEnum>::value_variants();
    assert_eq!(agents.len(), 8, "a new agent needs a row here: {agents:?}");
    for agent in agents {
        let (repo, home) = sandbox();
        let first = install(&repo, home.path(), &["--agent", agent.id(), "--observe"]);
        assert_eq!(first.code, 0, "{agent:?}: {}{}", first.stdout, first.stderr);
        let (rel, observe) = config_for_mode(*agent, true);
        assert_eq!(read(&repo, rel), observe, "{agent:?}");
        assert!(observe.contains("--observe"), "{agent:?}");

        let up = install(&repo, home.path(), &["--agent", agent.id(), "--upgrade"]);
        assert_eq!(up.code, 0, "{agent:?}: {}{}", up.stdout, up.stderr);
        assert_eq!(
            read(&repo, rel),
            observe,
            "{agent:?}: the upgrade changed the mode: {}",
            up.stdout
        );
    }
}

/// Control: the user-level Copilot file keeps observe mode through `--upgrade` too.
#[test]
fn the_user_level_copilot_file_keeps_observe_mode_through_upgrade() {
    let (repo, home) = sandbox();
    let file = home.path().join(".copilot/hooks/discipline.json");
    let first = install(
        &repo,
        home.path(),
        &["--agent", "copilot", "--user", "--observe"],
    );
    assert_eq!(first.code, 0, "{}{}", first.stdout, first.stderr);
    let before = std::fs::read_to_string(&file).unwrap();
    assert!(before.contains("--if-configured --observe"), "{before}");
    let up = install(
        &repo,
        home.path(),
        &["--agent", "copilot", "--user", "--upgrade"],
    );
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
}

/// The upgrade says which mode it wrote, in the same words for the plugin and for a JSON
/// hook file; a file without a mode (the Claude Code bootstrap) gets no such words.
#[test]
fn the_upgrade_output_names_the_mode_it_wrote() {
    let v = env!("CARGO_PKG_VERSION");
    for (agent, rel) in [
        (Agent::Opencode, PLUGIN),
        (Agent::Qwen, ".qwen/settings.json"),
        (Agent::Codex, ".codex/hooks.json"),
    ] {
        for (observe, mode) in [(true, "observe"), (false, "enforcing")] {
            let (repo, home) = sandbox();
            repo.write(rel, &earlier_release(agent, observe));
            let up = install(&repo, home.path(), &["--agent", agent.id(), "--upgrade"]);
            assert_eq!(up.code, 0, "{agent:?}: {}{}", up.stdout, up.stderr);
            let line = up
                .stdout
                .lines()
                .find(|l| l.contains("upgraded") && l.contains(rel))
                .unwrap_or_else(|| panic!("{agent:?}: no upgrade line: {}", up.stdout));
            assert!(
                line.ends_with(&format!("to discipline {v}, in {mode} mode")),
                "{agent:?} observe={observe}: {line}"
            );
        }
    }

    // The bootstrap has no mode.
    let (repo, home) = sandbox();
    install(&repo, home.path(), &["--agent", "claude-code"]);
    let boot = ".claude/hooks/discipline-bootstrap.sh";
    let altered = read(&repo, boot).replace(&format!("v{v}"), "v0.0.1");
    let old = restamped(&altered);
    repo.write(boot, &old);
    let up = install(&repo, home.path(), &["--agent", "claude-code", "--upgrade"]);
    assert_eq!(up.code, 0, "{}{}", up.stdout, up.stderr);
    let line = up
        .stdout
        .lines()
        .find(|l| l.contains("upgraded") && l.contains(boot))
        .unwrap_or_else(|| panic!("no upgrade line: {}", up.stdout));
    assert!(line.ends_with(&format!("to discipline {v}")), "{line}");

    // The same alteration under the digest of the unaltered script is an edit: no
    // upgrade line, the difference, and the script as it was.
    repo.write(boot, &altered);
    let refused = install(&repo, home.path(), &["--agent", "claude-code", "--upgrade"]);
    assert_eq!(refused.code, 1, "{}{}", refused.stdout, refused.stderr);
    assert!(
        refused.stdout.contains("-version=\"v0.0.1\"")
            && !refused
                .stdout
                .lines()
                .any(|l| l.contains("upgraded") && l.contains(boot)),
        "{}",
        refused.stdout
    );
    assert_eq!(read(&repo, boot), altered);
}

/// `--upgrade --observe` on an enforcing file says it wrote observe mode.
#[test]
fn turning_observe_on_by_upgrade_says_observe_mode() {
    for (agent, rel) in [
        (Agent::Opencode, PLUGIN),
        (Agent::Qwen, ".qwen/settings.json"),
    ] {
        let (repo, home) = sandbox();
        install(&repo, home.path(), &["--agent", agent.id()]);
        let up = install(
            &repo,
            home.path(),
            &["--agent", agent.id(), "--upgrade", "--observe"],
        );
        assert_eq!(up.code, 0, "{agent:?}: {}{}", up.stdout, up.stderr);
        assert!(
            up.stdout.contains("upgraded") && up.stdout.contains(", in observe mode"),
            "{agent:?}: {}",
            up.stdout
        );
        assert_eq!(read(&repo, rel), config_for_mode(agent, true).1);
    }
}
