//! `unsafe-budget` fails closed on a head side it cannot parse: with `max_unsafe = 0` and
//! every other gate off, a change that adds an `unsafe` block to a file nested past the
//! parse-depth limit is a could-not-check (exit 2), never a pass.

mod common;
use common::{Repo, CONFIG_HEAD};

/// A Rust file with one new `unsafe` block, and an expression nested `depth` levels deep.
fn source(depth: usize) -> String {
    format!(
        "pub fn f(p: *const u8) -> u8 {{\n    unsafe {{ *p }}\n}}\n\npub fn g() -> u32 {{\n    {}1{}\n}}\n",
        "(".repeat(depth),
        ")".repeat(depth)
    )
}

/// A repository whose configuration enables `unsafe-budget` with a cap of zero.
fn repo() -> Repo {
    let repo = Repo::new();
    repo.write(
        "discipline.toml",
        &format!("{CONFIG_HEAD}\n[gates.unsafe-budget]\nenabled = true\nmax_unsafe = 0\n"),
    );
    repo.commit("chore: configuration");
    repo.git(&["branch", "-f", "main"]);
    repo
}

/// Every gate the run reports except `unsafe-budget`, read from a run's own report so
/// that a gate added later is switched off too.
fn every_other_gate(repo: &Repo) -> String {
    let run = repo.check(&[]);
    let gates: Vec<String> = run.json()["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|o| o["gate"].as_str())
        .filter(|g| *g != "unsafe-budget")
        .map(str::to_string)
        .collect();
    assert!(!gates.is_empty(), "{}", run.stdout);
    gates.join(",")
}

#[test]
fn an_unparseable_head_with_new_unsafe_is_could_not_check_with_only_this_gate_on() {
    // The control: the same change nested within the limit is read, and the new
    // `unsafe` block is reported against the cap.
    let r = repo();
    r.write("src/lib.rs", &source(10));
    r.commit("feat: add a module");
    let others = every_other_gate(&r);
    let run = r.check(&["--disable", &others]);
    assert_eq!(run.code, 1, "{}\n{}", run.stdout, run.stderr);
    assert!(
        !run.titles("unsafe-budget").is_empty(),
        "the new unsafe block is reported: {}",
        run.stdout
    );
    let report = run.json();
    let enabled: Vec<&str> = report["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["enabled"].as_bool() == Some(true))
        .filter_map(|o| o["gate"].as_str())
        .collect();
    // `config-integrity` reads only the configuration and is not switched off by
    // `--disable`; every gate that reads source is off but this one.
    assert_eq!(
        enabled,
        ["config-integrity", "unsafe-budget"],
        "{}",
        run.stdout
    );

    // Nested far past the 4096-level limit, the file has no facts: the run cannot say
    // how many `unsafe` sites it holds, so it stops with exit 2 under this gate.
    let r = repo();
    r.write("src/lib.rs", &source(10));
    r.commit("feat: add a module");
    let others = every_other_gate(&r);
    r.write("src/lib.rs", &source(6000));
    r.commit("feat: nest the module");
    let run = r.check(&["--disable", &others]);
    assert_eq!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    let (reason, gate) = run.could_not_check();
    assert_eq!(reason, "gate");
    assert_eq!(gate.as_deref(), Some("unsafe-budget"));
    let detail = run.json()["could_not_check"]["detail"].to_string();
    assert!(detail.contains("could not parse `src/lib.rs`"), "{detail}");
}
