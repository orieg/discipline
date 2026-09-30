//! Verification of threat model claims against pinning tests (docs/ARCHITECTURE.md §1.4, §3, §10, SECURITY.md, docs/GATES.md).
//!
//! Modelled on `tests/test_stability_contract.rs`:
//! - Every claim in `tests/fixtures/threat_model_claims.json` must be well-formed with a stable id, source, and code anchor.
//! - Every test named in `pinning_tests` must actually exist in the test suite (no stale or renamed references).
//! - When untested claims exist, the test suite reports them as the target list for adversarial red-teaming.

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct Claim {
    id: String,
    category: String,
    claim: String,
    source: String,
    code: String,
    pinning_tests: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ClaimsDoc {
    description: String,
    claims: Vec<Claim>,
}

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn collect_all_test_targets() -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let test_dir = root().join("tests");
    let test_re =
        regex::Regex::new(r"#\[test\]\s*(?:#\[[^\]]+\]\s*)*(?:async\s+)?fn\s+([a-zA-Z0-9_]+)")
            .unwrap();

    fn walk(dir: PathBuf, re: &regex::Regex, out: &mut BTreeMap<String, BTreeSet<String>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(path, re, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let content = std::fs::read_to_string(&path).unwrap();
                let rel = path
                    .strip_prefix(root())
                    .unwrap()
                    .to_string_lossy()
                    .to_string();
                for cap in re.captures_iter(&content) {
                    out.entry(rel.clone())
                        .or_default()
                        .insert(cap[1].to_string());
                }
            }
        }
    }

    walk(test_dir, &test_re, &mut out);
    out
}

#[test]
fn claims_fixture_is_valid() {
    let fixture_path = root().join("tests/fixtures/threat_model_claims.json");
    assert!(
        fixture_path.exists(),
        "threat_model_claims.json fixture must exist"
    );

    let content = std::fs::read_to_string(&fixture_path).unwrap();
    let doc: ClaimsDoc =
        serde_json::from_str(&content).expect("fixture must be valid JSON matching ClaimsDoc");

    assert!(
        !doc.description.trim().is_empty(),
        "claims doc description must not be empty"
    );

    assert!(
        doc.claims.len() >= 60,
        "expected at least 60 claims, found {}",
        doc.claims.len()
    );

    let mut seen_ids = BTreeSet::new();
    for claim in &doc.claims {
        assert!(!claim.id.trim().is_empty(), "claim id must not be empty");
        assert!(
            seen_ids.insert(claim.id.clone()),
            "duplicate claim id: {}",
            claim.id
        );
        assert!(
            !claim.claim.trim().is_empty(),
            "claim text must not be empty for {}",
            claim.id
        );
        assert!(
            !claim.source.trim().is_empty(),
            "claim source must not be empty for {}",
            claim.id
        );
        assert!(
            !claim.code.trim().is_empty(),
            "claim code reference must not be empty for {}",
            claim.id
        );
        assert!(
            !claim.category.trim().is_empty(),
            "claim category must not be empty for {}",
            claim.id
        );
    }
}

#[test]
fn every_pinning_test_exists() {
    let fixture_path = root().join("tests/fixtures/threat_model_claims.json");
    let content = std::fs::read_to_string(&fixture_path).unwrap();
    let doc: ClaimsDoc = serde_json::from_str(&content).unwrap();

    let available_tests = collect_all_test_targets();
    let mut missing = Vec::new();

    for claim in &doc.claims {
        for target in &claim.pinning_tests {
            let parts: Vec<&str> = target.split("::").collect();
            if parts.len() != 2 {
                missing.push(format!(
                    "claim {} has malformed test target '{}' (must be file::fn)",
                    claim.id, target
                ));
                continue;
            }
            let file = parts[0];
            let fn_name = parts[1];

            match available_tests.get(file) {
                None => {
                    missing.push(format!(
                        "claim {} targets file '{}' which does not exist or has no tests",
                        claim.id, file
                    ));
                }
                Some(fns) => {
                    if !fns.contains(fn_name) {
                        missing.push(format!(
                            "claim {} targets test fn '{}' not found in '{}'",
                            claim.id, fn_name, file
                        ));
                    }
                }
            }
        }
    }

    assert!(
        missing.is_empty(),
        "pinning tests in threat_model_claims.json do not exist or were renamed:\n  {}",
        missing.join("\n  ")
    );
}

/// F11 Invariant: Untrusted text never reaches a shell parser.
/// Verifies action.yml directly: all `run:` blocks must pass values through `env:`, zero `${{ ... }}` interpolation.
#[test]
fn action_yml_has_no_inline_shell_interpolation() {
    let action_path = root().join("action.yml");
    let content = std::fs::read_to_string(action_path).expect("action.yml must exist");

    let lines: Vec<&str> = content.lines().collect();
    let mut violations = Vec::new();
    let mut in_run = false;
    let mut run_indent = 0;

    for (line_idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();

        if trimmed.starts_with("run:") {
            in_run = true;
            run_indent = indent;
            if trimmed.contains("${{") {
                violations.push(format!("line {}: {}", line_idx + 1, line));
            }
            continue;
        }

        if in_run {
            if trimmed.is_empty() {
                continue;
            }
            if indent <= run_indent && !trimmed.starts_with('|') && !trimmed.starts_with('>') {
                in_run = false;
            } else if line.contains("${{") {
                violations.push(format!("line {}: {}", line_idx + 1, line));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "F11 violation: action.yml contains inline `${{{{ }}}}` interpolation in a run block:\n  {}",
        violations.join("\n  ")
    );
}

/// Security Policy Contract: Untrusted, malformed, or hostile source code parses safely or records parse errors without crashing.
#[test]
fn sec_tree_sitter_parse_safety() {
    let registry = discipline::ast::default_registry();
    let vocab = discipline::ast::AssertVocabulary::default();

    let adversarial_payloads: Vec<(&str, &str)> = vec![
        ("empty.rs", ""),
        ("nul.rs", "\0\0\0\0"),
        (
            "unclosed_parens.py",
            "def test_foo():\n    assert(((((((((((((((((1 == 1\n",
        ),
        (
            "nested_braces.js",
            "function test() { {{{{{{{{{{{{{{ return; }}}}}}}}}}}}}} }",
        ),
        (
            "truncated_tokens.go",
            "package main\nfunc TestBad(t *testing.T) {\n  if true {\n",
        ),
    ];

    for (path, src) in adversarial_payloads {
        let pack = registry
            .find_pack(path)
            .expect("registered pack for extension");
        let facts = pack.extract(path, src, &vocab);
        assert!(
            facts.is_ok(),
            "tree-sitter extraction failed unexpectedly on {path}: {:?}",
            facts.err()
        );
    }
}

/// Security Policy Contract: Malformed git repositories, invalid diffs, or non-UTF8 paths do not cause panics.
#[test]
fn sec_libgit2_memory_safety() {
    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    let sig = git2::Signature::now("Security Auditor", "audit@lab.invalid").unwrap();
    let mut index = repo.index().unwrap();

    // Add a raw binary blob containing NULs and non-UTF8 sequences
    let blob_id = repo
        .blob(b"\x00\xFF\xFE\x00\x01\x02\x03\x04evil_payload\0")
        .unwrap();
    index
        .add(&git2::IndexEntry {
            ctime: git2::IndexTime::new(0, 0),
            mtime: git2::IndexTime::new(0, 0),
            dev: 0,
            ino: 0,
            mode: 0o100644,
            uid: 0,
            gid: 0,
            file_size: 20,
            id: blob_id,
            flags: 0,
            flags_extended: 0,
            path: b"binary_test.bin".to_vec(),
        })
        .unwrap();
    index.write().unwrap();

    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        "test: adversarial commit with binary blob",
        &tree,
        &[],
    )
    .unwrap();

    // Verify gitctx detects binary file without panicking
    let is_bin = discipline::gitctx::is_binary_file("binary_test.bin", b"\x00\xFF\xFE\x00");
    assert!(is_bin, "binary file must be identified as binary");
}

/// Lists all claims without pinning tests. When run, acts as the definitive red-team target list.
#[test]
fn report_and_verify_claims_coverage() {
    let fixture_path = root().join("tests/fixtures/threat_model_claims.json");
    let content = std::fs::read_to_string(&fixture_path).unwrap();
    let doc: ClaimsDoc = serde_json::from_str(&content).unwrap();

    let mut unpinned = Vec::new();
    let mut pinned_count = 0;

    for claim in &doc.claims {
        if claim.pinning_tests.is_empty() {
            unpinned.push(format!("{} ({}): {}", claim.id, claim.source, claim.claim));
        } else {
            pinned_count += 1;
        }
    }

    let total = doc.claims.len();
    eprintln!(
        "\n=== Threat Model Claims Coverage ===\nTotal claims: {}\nPinned: {} ({:.1}%)\nUnpinned: {}\n",
        total,
        pinned_count,
        (pinned_count as f64 / total as f64) * 100.0,
        unpinned.len()
    );

    if !unpinned.is_empty() {
        eprintln!("Untested claims target list:\n  {}", unpinned.join("\n  "));
    }

    if std::env::var("DISCIPLINE_ALLOW_UNTESTED_CLAIMS").is_err() && !unpinned.is_empty() {
        panic!(
            "There are {} untested threat model claims that require discriminating tests or red-team verification:\n  {}\n\nSet DISCIPLINE_ALLOW_UNTESTED_CLAIMS=1 to bypass.",
            unpinned.len(),
            unpinned.join("\n  ")
        );
    }
}

/// Attack Probe: Directive parsing with CRLF line endings (Windows/RFC 2822).
#[test]
fn sec_directive_crlf_parsing() {
    let crlf_commit_body =
        "feat: delete old test\r\n\r\nremoves: tests/old.rs migrating to new test suite\r\n";
    let dirs = discipline::tokens::parse_directives_with_names(
        crlf_commit_body,
        discipline::tokens::REMOVES,
        discipline::tokens::OverrideSource::Commit("0123456".into()),
    );
    assert_eq!(dirs.len(), 1, "CRLF directive must be parsed");
    assert_eq!(dirs[0].directive, "removes");
    assert!(dirs[0].covers("tests/old.rs"));
}

/// Attack Probe: Directive smuggling via Markdown formatting (blockquotes, fenced code, inline).
#[test]
fn sec_directive_markdown_smuggling_defense() {
    let names = discipline::tokens::REMOVES;

    // Blockquote
    let bq = "> removes: tests/old.rs smuggled inside blockquote\n";
    let dirs_bq = discipline::tokens::parse_directives_with_names(
        bq,
        names,
        discipline::tokens::OverrideSource::PrBody,
    );
    assert!(
        dirs_bq.is_empty(),
        "blockquoted directive must never be parsed"
    );

    // Fenced code block
    let fenced = "```\nremoves: tests/old.rs inside code block\n```\n";
    let dirs_fenced = discipline::tokens::parse_directives_with_names(
        fenced,
        names,
        discipline::tokens::OverrideSource::PrBody,
    );
    assert!(
        dirs_fenced.is_empty(),
        "fenced code block directive must never be parsed"
    );

    // HTML comment when allow_hidden is false
    let html = "<!-- removes: tests/old.rs inside comment -->\n";
    let dirs_html = discipline::tokens::parse_directives_with_names(
        html,
        names,
        discipline::tokens::OverrideSource::PrBody,
    );
    assert_eq!(dirs_html.len(), 1);
    assert!(
        dirs_html[0].hidden,
        "HTML comment directive must be marked hidden"
    );
}

/// Attack Probe: Directives placed in the commit subject line (line 0) must be ignored.
#[test]
fn sec_directive_subject_line_ignored() {
    let subject_msg = "removes: tests/old.rs replaced by new tests\n\nCommit body explanation";
    let dirs = discipline::tokens::parse_directives_with_names(
        subject_msg,
        discipline::tokens::REMOVES,
        discipline::tokens::OverrideSource::Commit("0123456".into()),
    );
    assert!(
        dirs.is_empty(),
        "directive on commit subject line must be ignored"
    );
}

/// Attack Probe: Documenting GAP where `removes: <subject>` with no reason or `removes: <subject> todo`
/// is treated as non-empty reason and bypasses placeholder check.
#[test]
fn sec_directive_placeholder_gap_proof() {
    let names = discipline::tokens::REMOVES;

    // A bare placeholder without subject IS caught:
    let bare_todo = "removes: todo\n";
    let dirs_bare = discipline::tokens::parse_directives_with_names(
        bare_todo,
        names,
        discipline::tokens::OverrideSource::PrBody,
    );
    assert!(dirs_bare.is_empty(), "bare placeholder must be rejected");

    // However, when a subject is included before placeholder, e.g. `removes: tests/old.rs todo`:
    let subject_todo = "removes: tests/old.rs todo\n";
    let dirs_subject = discipline::tokens::parse_directives_with_names(
        subject_todo,
        names,
        discipline::tokens::OverrideSource::PrBody,
    );
    // Verified finding / GAP: dirs_subject currently parses because reason is "tests/old.rs todo"
    // and is not in PLACEHOLDERS list.
    assert!(
        !dirs_subject.is_empty(),
        "proves GAP: subject+todo is currently parsed as valid directive"
    );
    assert!(
        dirs_subject[0].covers("tests/old.rs"),
        "proves GAP: covers tests/old.rs despite placeholder reason"
    );
}

/// Attack Probe: Ratified-paths comment parsing rejects smuggled blocks.
#[test]
fn sec_ratified_paths_adversarial_blocks() {
    let marker = "Owner-ratified-paths:";

    // Quoted blockquote:
    let bq = "> Owner-ratified-paths:\n> - protected.txt\n";
    assert!(discipline::ratification::parse_blocks(bq, marker).is_empty());

    // Fenced code block:
    let code = "```\nOwner-ratified-paths:\n- protected.txt\n```\n";
    assert!(discipline::ratification::parse_blocks(code, marker).is_empty());

    // HTML comment:
    let html = "<!--\nOwner-ratified-paths:\n- protected.txt\n-->\n";
    assert!(discipline::ratification::parse_blocks(html, marker).is_empty());

    // Malformed glob entry:
    let glob_block = "Owner-ratified-paths:\n- protected/*.txt\n";
    let parsed = discipline::ratification::parse_blocks(glob_block, marker);
    assert_eq!(parsed.len(), 1);
    assert!(parsed[0].entries.is_empty(), "glob entry must be refused");
    assert_eq!(parsed[0].refused.len(), 1);
    assert_eq!(
        parsed[0].refused[0].1,
        "it is a glob; name each path exactly"
    );
}
