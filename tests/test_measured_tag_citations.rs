//! `provenance-tags` checks what a `(measured: <host>, <commit>)` tag cites (#534), behind
//! keys that are off by default: `verify_measured_commit` (the tag names a host and a
//! commit the repository holds), `record_paths` / `record_commit_key` (a result record's
//! commit is a full id that resolves) and `verify_cited_figures` / `figure_tolerance_pct`
//! (a tagged figure is a value of the artifact its paragraph cites).
//!
//! Every case drives the real binary over a temporary repository with real commits, so an
//! id resolves, or does not, through the object database. Each rule has a control beside
//! it that pins the other verdict.

mod common;
use common::{git_command, Repo, Run, CONFIG_HEAD};

const GATE: &str = "provenance-tags";
const PLACEHOLDER: &str = "provenance-tags/placeholder-provenance-tag";
const UNRESOLVABLE_TAG: &str = "provenance-tags/unresolvable-measured-commit";
const UNRESOLVABLE_RECORD: &str = "provenance-tags/unresolvable-record-commit";
const DISAGREES: &str = "provenance-tags/figure-disagrees-with-artifact";

/// An object id no test repository holds.
const ABSENT: &str = "0123456789abcdef0123456789abcdef01234567";

/// The gate with its other rules off, so a case shows one rule, and `keys` added.
fn config(keys: &str) -> String {
    format!(
        "{CONFIG_HEAD}[gates.provenance-tags]\nenabled = true\ncheck_tables = false\n\
         check_mechanisms = false\ncheck_intervals = false\ncheck_paired_figures = false\n{keys}"
    )
}

/// A repository whose base holds the configuration, and the id of its base commit.
fn repo_with(keys: &str) -> (Repo, String) {
    let repo = Repo::new();
    repo.commit_base("discipline.toml", &config(keys), "chore: config");
    let base = repo.git_output(&["rev-parse", "main"]);
    (repo, base)
}

/// Writes `files`, commits them on the work branch and checks the change.
fn check(repo: &Repo, files: &[(&str, &str)]) -> Run {
    for (path, content) in files {
        repo.write(path, content);
    }
    repo.commit("docs: figures");
    repo.check(&[])
}

fn codes(run: &Run) -> Vec<String> {
    run.violations(GATE)
        .iter()
        .map(|v| v["code"].as_str().unwrap().to_string())
        .collect()
}

fn notes(run: &Run) -> String {
    run.outcome(GATE)["notes"].to_string()
}

fn doc(tag: &str) -> String {
    format!("# Perf\n\nPoint lookup takes 12.38 ns {tag}.\n")
}

// ---- Step 1: the tag names a host and a commit ---------------------------------------

#[test]
fn a_placeholder_tag_is_reported_and_a_tag_naming_a_real_commit_is_not() {
    let (repo, _) = repo_with("verify_measured_commit = true\n");
    let run = check(&repo, &[("docs/perf.md", &doc("(measured: host, commit)"))]);
    assert_eq!(run.code, 1, "{}", run.stdout);
    assert_eq!(codes(&run), [PLACEHOLDER, PLACEHOLDER]);
    let v = &run.violations(GATE)[0];
    assert_eq!(
        (v["file"].as_str(), v["line"].as_u64()),
        (Some("docs/perf.md"), Some(3))
    );

    // Control: the same document naming a host and the base commit.
    let (repo, base) = repo_with("verify_measured_commit = true\n");
    let run = check(
        &repo,
        &[(
            "docs/perf.md",
            &doc(&format!("(measured: bench-box, {base})")),
        )],
    );
    assert_eq!(run.code, 0, "{}", run.stdout);
    assert!(codes(&run).is_empty());
    assert!(
        notes(&run).contains("verify_measured_commit: 1 measured tag(s)"),
        "{}",
        notes(&run)
    );

    // A tag that does not have the form names neither.
    for tag in [
        "(measured)",
        "(measured on the reference host)",
        "(measured: bench-box)",
    ] {
        let (repo, _) = repo_with("verify_measured_commit = true\n");
        let run = check(&repo, &[("docs/perf.md", &doc(tag))]);
        assert_eq!(codes(&run), [PLACEHOLDER], "{tag}");
    }
}

#[test]
fn a_host_name_is_not_quoted_in_a_finding() {
    let (repo, _) = repo_with("verify_measured_commit = true\n");
    let run = check(
        &repo,
        &[(
            "docs/perf.md",
            &doc("(measured: build-07.corp.example, tbd)"),
        )],
    );
    assert_eq!(codes(&run), [PLACEHOLDER]);
    assert!(!run.stdout.contains("build-07"), "{}", run.stdout);
}

#[test]
fn a_tag_commit_must_resolve_to_a_commit_object() {
    let (repo, base) = repo_with("verify_measured_commit = true\n");
    let tree = repo.git_output(&["rev-parse", "main^{tree}"]);
    repo.git(&["tag", "-a", "v1", "-m", "v1", "main"]);
    let tag_object = repo.git_output(&["rev-parse", "v1"]);
    let text = format!(
        "# Perf\n\nA 12.38 ns (measured: bench-box, {ABSENT}).\n\nB 12.38 ns (measured: bench-box, {tree}).\n\n\
         C 12.38 ns (measured: bench-box, main).\n\nD 12.38 ns (measured: bench-box, {tag_object}).\n\n\
         E 12.38 ns (measured: bench-box, {}).\n\nF 12.38 ns (measured: bench-box, {}).\n",
        &base[..6],
        &base[..7],
    );
    let run = check(&repo, &[("docs/perf.md", &text)]);
    assert_eq!(run.code, 1);
    let lines: Vec<(String, u64)> = run
        .violations(GATE)
        .iter()
        .map(|v| {
            (
                v["code"].as_str().unwrap().to_string(),
                v["line"].as_u64().unwrap(),
            )
        })
        .collect();
    // An absent id, a tree, a branch name, a tag object and a six-digit abbreviation are
    // reported; the seven-digit abbreviation of a commit (line 13) is not.
    let expected: Vec<(String, u64)> = [3, 5, 7, 9, 11]
        .iter()
        .map(|l| (UNRESOLVABLE_TAG.to_string(), *l))
        .collect();
    assert_eq!(lines, expected, "{}", run.stdout);
    let messages = run.stdout.clone();
    assert!(
        messages.contains(ABSENT),
        "the id that does not resolve is named"
    );
}

#[test]
fn a_tag_the_change_did_not_write_is_not_judged() {
    let (repo, base) = repo_with("verify_measured_commit = true\n");
    repo.commit_base(
        "docs/perf.md",
        &doc("(measured: host, commit)"),
        "docs: old figure",
    );
    let appended = format!(
        "{}\nRange scan takes 40.5 ns (measured: bench-box, {base}).\n",
        doc("(measured: host, commit)")
    );
    let run = check(&repo, &[("docs/perf.md", &appended)]);
    assert_eq!(run.code, 0, "{}", run.stdout);
    assert!(codes(&run).is_empty());
    assert!(notes(&run).contains("1 measured tag(s)"), "{}", notes(&run));

    // Control: the change rewrites the old line, and the tag on it is judged.
    let rewritten = appended.replace("Point lookup", "A point lookup");
    let run = check(&repo, &[("docs/perf.md", &rewritten)]);
    assert_eq!(codes(&run), [PLACEHOLDER, PLACEHOLDER]);
}

#[test]
fn a_tag_in_the_pull_request_description_is_judged() {
    let (repo, base) = repo_with("verify_measured_commit = true\n");
    repo.write("docs/notes.md", "# Notes\n\nNothing measured here.\n");
    repo.commit("docs: notes");
    let run = repo.check_with_pr(
        &[],
        "Lookup takes 12.38 ns (measured: host, commit).\n\nno-issue: fixture",
    );
    let found: Vec<_> = run
        .violations(GATE)
        .iter()
        .map(|v| {
            (
                v["code"].as_str().unwrap().to_string(),
                v["file"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            (PLACEHOLDER.to_string(), "PR body".to_string()),
            (PLACEHOLDER.to_string(), "PR body".to_string())
        ]
    );
    let good = format!("Lookup takes 12.38 ns (measured: bench-box, {base}).\n\nno-issue: fixture");
    assert!(codes(&repo.check_with_pr(&[], &good)).is_empty());
}

/// Two blob contents whose object ids share their first seven hexadecimal digits.
fn blobs_sharing_a_prefix() -> (String, String, String) {
    let mut seen = std::collections::HashMap::new();
    for i in 0u32.. {
        let content = format!("blob {i}\n");
        let id = git2::Oid::hash_object(git2::ObjectType::Blob, content.as_bytes())
            .unwrap()
            .to_string();
        if let Some(other) = seen.insert(id[..7].to_string(), content.clone()) {
            return (id[..7].to_string(), other, content);
        }
    }
    unreachable!()
}

#[test]
fn an_abbreviation_two_objects_share_cannot_be_checked() {
    let (prefix, one, two) = blobs_sharing_a_prefix();
    let (repo, _) = repo_with("verify_measured_commit = true\n");
    repo.write("data/one.txt", &one);
    let text = doc(&format!("(measured: bench-box, {prefix})"));
    // Control: one object under the prefix. It is a blob, so the tag is a finding.
    let run = check(&repo, &[("docs/perf.md", &text)]);
    assert_eq!(run.code, 1, "{}", run.stdout);
    assert_eq!(codes(&run), [UNRESOLVABLE_TAG]);

    let run = check(&repo, &[("data/two.txt", &two)]);
    assert_eq!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    assert_eq!(
        run.could_not_check(),
        ("repository".to_string(), Some(GATE.to_string()))
    );
    assert!(
        run.stderr.contains("matches more than one object"),
        "{}",
        run.stderr
    );
    assert!(run.stderr.contains("docs/perf.md:3"), "{}", run.stderr);
}

/// A clone of `origin`'s `main` holding its newest commit only, on a `work` branch.
fn shallow_clone(origin: &Repo) -> Repo {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("file://{}", origin.path().display());
    let out = git_command()
        .args(["clone", "-q", "--depth", "1", "--branch", "main", &url, "."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let clone = Repo { dir };
    clone.git(&["checkout", "-q", "-b", "work"]);
    clone
}

#[test]
fn an_id_absent_from_a_shallow_clone_cannot_be_checked() {
    let (origin, first) = repo_with("verify_measured_commit = true\n");
    origin.commit_base("docs/plan.md", "# Plan\n\nPhase 1.\n", "docs: plan");
    let newest = origin.git_output(&["rev-parse", "main"]);
    assert_ne!(first, newest);

    let clone = shallow_clone(&origin);
    assert_eq!(
        clone.git_output(&["rev-parse", "--is-shallow-repository"]),
        "true"
    );
    // The clone does not hold the first commit, and cannot say that nobody does.
    let run = check(
        &clone,
        &[(
            "docs/perf.md",
            &doc(&format!("(measured: bench-box, {first})")),
        )],
    );
    assert_eq!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    assert_eq!(
        run.could_not_check(),
        ("repository".to_string(), Some(GATE.to_string()))
    );
    assert!(run.stderr.contains("shallow clone"), "{}", run.stderr);

    // Control: a commit the shallow clone holds is checked there as anywhere.
    let clone = shallow_clone(&origin);
    let run = check(
        &clone,
        &[(
            "docs/perf.md",
            &doc(&format!("(measured: bench-box, {newest})")),
        )],
    );
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    assert!(codes(&run).is_empty());

    // Control: in the full repository the first commit resolves, and an id nobody holds
    // is a finding, not a could-not-check.
    origin.git(&["checkout", "-q", "-B", "work", "main"]);
    let run = check(
        &origin,
        &[(
            "docs/perf.md",
            &doc(&format!("(measured: bench-box, {first})")),
        )],
    );
    assert_eq!(run.code, 0, "{}", run.stdout);
    let run = check(
        &origin,
        &[(
            "docs/perf.md",
            &doc(&format!("(measured: bench-box, {ABSENT})")),
        )],
    );
    assert_eq!(run.code, 1);
    assert_eq!(codes(&run), [UNRESOLVABLE_TAG]);
}

// ---- Step 2: result records ---------------------------------------------------------

const RECORDS: &str = "record_paths = [\"results/**\"]\n";

#[test]
fn an_added_record_must_carry_a_full_commit_id_that_resolves() {
    let (repo, base) = repo_with(RECORDS);
    let jsonl = format!(
        "{{\"arm\": \"get\", \"commit\": \"{base}\", \"ns\": 12.4}}\n{{\"arm\": \"put\", \"commit\": \"unknown\", \"ns\": 40.5}}\n\
         {{\"arm\": \"scan\", \"ns\": 3}}\n{{\"arm\": \"del\", \"commit\": \"{}\"}}\n{{\"arm\": \"old\", \"commit\": \"{ABSENT}\"}}\n",
        &base[..12]
    );
    let run = check(&repo, &[("results/run.jsonl", &jsonl)]);
    assert_eq!(run.code, 1, "{}", run.stdout);
    let found: Vec<(String, u64)> = run
        .violations(GATE)
        .iter()
        .map(|v| {
            (
                v["code"].as_str().unwrap().to_string(),
                v["line"].as_u64().unwrap(),
            )
        })
        .collect();
    let expected: Vec<(String, u64)> = [2, 3, 4, 5]
        .iter()
        .map(|l| (UNRESOLVABLE_RECORD.to_string(), *l))
        .collect();
    assert_eq!(found, expected, "{}", run.stdout);
    // A value that is not an id is whatever the harness wrote: it is not quoted.
    let messages: String = run
        .violations(GATE)
        .iter()
        .map(|v| v["message"].to_string())
        .collect();
    assert!(!messages.contains("unknown"), "{messages}");
    assert!(messages.contains(ABSENT), "{messages}");
    assert!(notes(&run).contains("record_paths: 5 added or changed record(s) judged in 1 file(s)"));

    // Control: the same file outside the configured paths is not read.
    let (repo, _) = repo_with(RECORDS);
    let run = check(&repo, &[("other/run.jsonl", &jsonl)]);
    assert_eq!(run.code, 0, "{}", run.stdout);
    // Control: with no `record_paths` nothing is read either.
    let (repo, _) = repo_with("");
    let run = check(&repo, &[("results/run.jsonl", &jsonl)]);
    assert_eq!(run.code, 0, "{}", run.stdout);
    assert!(!notes(&run).contains("record_paths"));
}

#[test]
fn a_record_the_change_did_not_add_is_not_judged() {
    let (repo, base) = repo_with(RECORDS);
    repo.commit_base_files(
        &[
            (
                "results/run.jsonl",
                "{\"commit\": \"unknown\", \"ns\": 1}\n",
            ),
            (
                "results/all.json",
                "[{\"commit\": \"unknown\", \"ns\": 1}]\n",
            ),
        ],
        "chore: old records",
    );
    let good = format!("{{\"commit\": \"{base}\", \"ns\": 2}}");
    let run = check(
        &repo,
        &[
            (
                "results/run.jsonl",
                &format!("{{\"commit\": \"unknown\", \"ns\": 1}}\n{good}\n"),
            ),
            (
                "results/all.json",
                &format!("[{{\"commit\": \"unknown\", \"ns\": 1}}, {good}]\n"),
            ),
        ],
    );
    assert_eq!(run.code, 0, "{}", run.stdout);
    assert!(
        notes(&run).contains("2 added or changed record(s) judged in 2 file(s)"),
        "{}",
        notes(&run)
    );

    // Control: a new record with a stand-in beside the old ones is reported, by its place.
    let bad = "{\"commit\": \"\", \"ns\": 3}";
    let run = check(
        &repo,
        &[(
            "results/all.json",
            &format!("[{{\"commit\": \"unknown\", \"ns\": 1}}, {good}, {bad}]\n"),
        )],
    );
    assert_eq!(codes(&run), [UNRESOLVABLE_RECORD]);
    assert!(run.violations(GATE)[0]["message"]
        .as_str()
        .unwrap()
        .contains("record 2"));
}

#[test]
fn another_commit_key_and_a_ratio_run_file_are_read() {
    let (repo, base) = repo_with("record_paths = [\"results/**\"]\nrecord_commit_key = \"rev\"\n");
    let run_file = |commit: &str| {
        format!(
            "{{\"schema\": \"discipline-bench-ratio/v1\", \"rev\": \"{base}\", \"provenance\": {{\"commit\": \"{commit}\"}}, \"axes\": {{}}}}\n"
        )
    };
    let run = check(
        &repo,
        &[
            (
                "results/keyed.json",
                &format!("{{\"rev\": \"{base}\", \"commit\": \"unknown\"}}\n"),
            ),
            ("results/ratio.json", &run_file(&base)),
        ],
    );
    assert_eq!(run.code, 0, "{}", run.stdout);

    let run = check(
        &repo,
        &[
            (
                "results/keyed.json",
                &format!("{{\"rev\": \"unknown\", \"commit\": \"{base}\"}}\n"),
            ),
            ("results/ratio.json", &run_file("abc123")),
        ],
    );
    let messages: Vec<String> = run
        .violations(GATE)
        .iter()
        .map(|v| {
            format!(
                "{} {}",
                v["file"].as_str().unwrap(),
                v["message"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(codes(&run), [UNRESOLVABLE_RECORD, UNRESOLVABLE_RECORD]);
    assert!(
        messages[0].starts_with("results/keyed.json") && messages[0].contains("`rev`"),
        "{messages:?}"
    );
    assert!(
        messages[1].starts_with("results/ratio.json")
            && messages[1].contains("`provenance.commit`"),
        "{messages:?}"
    );
}

#[test]
fn a_record_file_that_does_not_parse_cannot_be_checked() {
    for (path, content) in [
        ("results/run.json", "{\"commit\": "),
        ("results/run.jsonl", "{\"commit\": \"x\"}\nnot json\n"),
    ] {
        let (repo, _) = repo_with(RECORDS);
        let run = check(&repo, &[(path, content)]);
        assert_eq!(run.code, 2, "{path}: {}\n{}", run.stdout, run.stderr);
        assert_eq!(run.could_not_check().1, Some(GATE.to_string()));
        assert!(
            run.stderr.contains(path) && run.stderr.contains("does not parse"),
            "{}",
            run.stderr
        );
    }
    // A file the globs match that is neither format is named in a note, not passed over.
    let (repo, _) = repo_with(RECORDS);
    let run = check(&repo, &[("results/run.yaml", "commit: unknown\n")]);
    assert_eq!(run.code, 0, "{}", run.stdout);
    assert!(
        notes(&run).contains("`results/run.yaml` is neither"),
        "{}",
        notes(&run)
    );
}

// ---- Step 3: a tagged figure is a value of the cited artifact --------------------------

const FIGURES: &str = "verify_cited_figures = true\n";
const ARTIFACT: &str = "{\"get\": {\"median_ns\": 12.3849, \"ratio\": 2.9}}\n";

fn figure_doc(body: &str) -> String {
    format!("# Perf\n\n{body}\n")
}

#[test]
fn a_tagged_figure_must_match_a_value_of_the_cited_artifact() {
    let good = figure_doc(
        "Point lookup takes 12.38 ns, 2.9x the old one (measured: bench-box, abc1234; `results/get.json`).",
    );
    let (repo, _) = repo_with(FIGURES);
    let run = check(
        &repo,
        &[("results/get.json", ARTIFACT), ("docs/perf.md", &good)],
    );
    assert_eq!(run.code, 0, "{}", run.stdout);
    assert!(
        notes(&run).contains("figures compared in 1 tagged paragraph(s)"),
        "{}",
        notes(&run)
    );

    // The same paragraph quoting another number.
    let stale = good.replace("12.38 ns", "11.9 ns");
    let (repo, _) = repo_with(FIGURES);
    let run = check(
        &repo,
        &[("results/get.json", ARTIFACT), ("docs/perf.md", &stale)],
    );
    assert_eq!(run.code, 1, "{}", run.stdout);
    assert_eq!(codes(&run), [DISAGREES]);
    let message = run.violations(GATE)[0]["message"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        message.contains("`11.9 ns`") && message.contains("`results/get.json`"),
        "{message}"
    );

    // A tolerance the repository configures admits it; a smaller one does not.
    let (repo, _) = repo_with("verify_cited_figures = true\nfigure_tolerance_pct = 5.0\n");
    let run = check(
        &repo,
        &[("results/get.json", ARTIFACT), ("docs/perf.md", &stale)],
    );
    assert_eq!(run.code, 0, "{}", run.stdout);
    let (repo, _) = repo_with("verify_cited_figures = true\nfigure_tolerance_pct = 1.0\n");
    let run = check(
        &repo,
        &[("results/get.json", ARTIFACT), ("docs/perf.md", &stale)],
    );
    assert_eq!(codes(&run), [DISAGREES]);
}

#[test]
fn tagged_figures_with_thousands_separators_and_percent_match_artifact() {
    let doc = figure_doc(
        "Throughput is 1,234,567 ops/s (or 1_234_567 ops/s) with 99.5% cache hit (measured: bench-box, abc1234; `results/perf.json`).",
    );
    let artifact = "{\"throughput\": 1234567.0, \"cache_hit_pct\": 99.5}\n";
    let (repo, _) = repo_with(FIGURES);
    let run = check(
        &repo,
        &[("results/perf.json", artifact), ("docs/perf.md", &doc)],
    );
    assert_eq!(run.code, 0, "{}", run.stdout);
    assert!(
        notes(&run).contains("figures compared in 1 tagged paragraph(s)"),
        "{}",
        notes(&run)
    );

    // Negative control: a figure with thousands separators that disagrees with the artifact.
    let stale = doc.replace("1,234,567", "1,234,568");
    let run = check(
        &repo,
        &[("results/perf.json", artifact), ("docs/perf.md", &stale)],
    );
    assert_eq!(run.code, 1, "{}", run.stdout);
    assert_eq!(codes(&run), [DISAGREES]);
}

#[test]
fn a_caption_tag_covers_its_table_and_csv_cells_are_values() {
    let table = |ns: &str| {
        figure_doc(&format!(
            "*(measured: bench-box, abc1234; results/arms.csv)*\n\n| arm | time |\n|---|---|\n| get | {ns} ns |\n| put | 40.5 ns |"
        ))
    };
    let csv = "arm,ns\nget,12.3849\nput,40.5\n";
    let (repo, _) = repo_with(FIGURES);
    let run = check(
        &repo,
        &[("results/arms.csv", csv), ("docs/perf.md", &table("12.38"))],
    );
    assert_eq!(run.code, 0, "{}", run.stdout);
    let run = check(&repo, &[("docs/perf.md", &table("14.00"))]);
    assert_eq!(codes(&run), [DISAGREES]);
    assert_eq!(run.violations(GATE)[0]["line"].as_u64(), Some(7));
}

#[test]
fn the_cited_artifact_must_be_tracked_and_must_parse() {
    let text =
        figure_doc("Point lookup takes 12.38 ns (measured: bench-box, abc1234; results/get.json).");
    // Not in the repository.
    let (repo, _) = repo_with(FIGURES);
    let run = check(&repo, &[("docs/perf.md", &text)]);
    assert_eq!(run.code, 1, "{}", run.stdout);
    assert_eq!(codes(&run), [DISAGREES]);
    assert!(run.violations(GATE)[0]["message"]
        .as_str()
        .unwrap()
        .contains("not a tracked file"));

    // On disk and ignored: still not tracked.
    let (repo, _) = repo_with(FIGURES);
    repo.write(".gitignore", "results/\n");
    let run = check(
        &repo,
        &[("results/get.json", ARTIFACT), ("docs/perf.md", &text)],
    );
    assert_eq!(codes(&run), [DISAGREES], "{}", run.stdout);

    // Tracked and not JSON.
    let (repo, _) = repo_with(FIGURES);
    let run = check(
        &repo,
        &[("results/get.json", "{\"get\": "), ("docs/perf.md", &text)],
    );
    assert_eq!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    assert_eq!(run.could_not_check().1, Some(GATE.to_string()));
    assert!(run.stderr.contains("results/get.json"), "{}", run.stderr);
}

#[test]
fn a_tagged_paragraph_that_cites_nothing_is_counted_in_a_note() {
    let (repo, _) = repo_with(FIGURES);
    let text = figure_doc("Point lookup takes 11.9 ns (measured: bench-box, abc1234).");
    let run = check(&repo, &[("docs/perf.md", &text)]);
    assert_eq!(run.code, 0, "{}", run.stdout);
    assert!(
        notes(&run).contains("1 tagged paragraph(s) cite no data artifact"),
        "{}",
        notes(&run)
    );
}

// ---- lifting, defaults and configuration ------------------------------------------------

#[test]
fn allow_provenance_lifts_each_of_the_new_findings() {
    let keys = "verify_measured_commit = true\nverify_cited_figures = true\nrecord_paths = [\"results/**\"]\n";
    let (repo, _) = repo_with(keys);
    let files = [
        (
            "results/get.json",
            "{\"commit\": \"unknown\", \"median_ns\": 12.3849}\n",
        ),
        (
            "docs/perf.md",
            &figure_doc(&format!(
                "Point lookup takes 11.9 ns (measured: host, {ABSENT}; results/get.json)."
            )) as &str,
        ),
    ];
    let run = check(&repo, &files);
    assert_eq!(run.code, 1);
    let mut found = codes(&run);
    found.sort();
    assert_eq!(
        found,
        [
            DISAGREES,
            PLACEHOLDER,
            UNRESOLVABLE_TAG,
            UNRESOLVABLE_RECORD
        ]
    );

    let body = "allow-provenance: docs/perf.md figures restated from the archived run, re-measured in #12\n\
                allow-provenance: results/get.json archived record predates revision capture, see #12\n\nno-issue: fixture";
    let lifted = repo.check_with_pr(&[], body);
    assert!(codes(&lifted).is_empty(), "{}", lifted.stdout);
    assert_eq!(
        lifted.outcome(GATE)["overrides"].as_array().unwrap().len(),
        4
    );
}

#[test]
fn with_every_key_off_the_gate_reports_what_it_did_before() {
    // Everything the new rules would report, under the gate's default configuration.
    let files = [
        (
            "results/get.json",
            "{\"commit\": \"unknown\", \"median_ns\": 12.3849}\n",
        ),
        (
            "docs/perf.md",
            "# Perf\n\nPoint lookup takes 11.9 ns (measured: host, commit; results/get.json).\n\n\
             Lookups are 2.9x faster at 1M keys.\n\n| arm | time |\n|---|---|\n| get | 12.4 ns |\n",
        ),
    ];
    let outcome = |keys: &str| {
        let repo = Repo::new();
        repo.commit_base(
            "discipline.toml",
            &format!("{CONFIG_HEAD}[gates.provenance-tags]\nenabled = true\n{keys}"),
            "chore: config",
        );
        let run = check(&repo, &files);
        assert_eq!(run.code, 1, "{}", run.stdout);
        let mut o = run.outcome(GATE);
        // A fingerprint is the same for the same finding; nothing else differs by run.
        o.as_object_mut().unwrap().remove("duration_ms");
        o
    };
    let default = outcome("");
    let found: Vec<&str> = default["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["code"].as_str().unwrap())
        .collect();
    assert_eq!(found, ["provenance-tags/wall-clock-ratio-without-interval"]);
    assert_eq!(default["notes"], serde_json::json!([]));
    assert_eq!(default["examined"], 1);
    // Written out as off, the keys change nothing.
    let off = outcome(
        "verify_measured_commit = false\nrecord_paths = []\nrecord_commit_key = \"commit\"\n\
         verify_cited_figures = false\nfigure_tolerance_pct = 0.0\n",
    );
    assert_eq!(default, off);
    // Control: switched on, the same change is reported by all three rules.
    let on = outcome("verify_measured_commit = true\nrecord_paths = [\"results/**\"]\nverify_cited_figures = true\n");
    assert_ne!(default, on);
    assert_eq!(on["violations"].as_array().unwrap().len(), 5, "{on}");
}

#[test]
fn turning_a_key_off_narrowing_the_paths_or_raising_the_tolerance_is_a_weakening() {
    let strict =
        "verify_measured_commit = true\nverify_cited_figures = true\nfigure_tolerance_pct = 1.0\n\
                  record_paths = [\"results/**\", \"bench/**\"]\n";
    let said = |head: &str| -> Vec<String> {
        let (repo, _) = repo_with(strict);
        repo.write("discipline.toml", &config(head));
        repo.commit("chore: tune");
        repo.check(&[])
            .violations("config-integrity")
            .iter()
            .map(|v| v["message"].as_str().unwrap().to_string())
            .collect()
    };
    let loose = said(
        "verify_measured_commit = false\nverify_cited_figures = false\nfigure_tolerance_pct = 5.0\n\
         record_paths = [\"results/**\"]\nrecord_commit_key = \"rev\"\n",
    );
    for needle in [
        "`verify_measured_commit` changed from true to false",
        "`verify_cited_figures` changed from true to false",
        "`figure_tolerance_pct` increased from 1.0 to 5.0",
        "`record_paths` lost 1",
        "`record_commit_key` changed",
    ] {
        assert!(
            loose.iter().any(|m| m.contains(needle)),
            "{needle}: {loose:?}"
        );
    }
    assert_eq!(loose.len(), 5, "{loose:?}");
    // Control: tightening each of them is not reported.
    let tight = said(
        "verify_measured_commit = true\nverify_cited_figures = true\nfigure_tolerance_pct = 0.5\n\
         record_paths = [\"results/**\", \"bench/**\", \"runs/**\"]\n",
    );
    assert!(tight.is_empty(), "{tight:?}");
}

#[test]
fn a_tolerance_below_zero_is_a_configuration_error() {
    let (repo, _) = repo_with("verify_cited_figures = true\nfigure_tolerance_pct = -1.0\n");
    let run = check(&repo, &[("docs/perf.md", "# Perf\n\nNothing.\n")]);
    assert_eq!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    assert!(
        run.stderr.contains("figure_tolerance_pct"),
        "{}",
        run.stderr
    );
}
